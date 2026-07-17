//! Obtaining the source archive to rebuild (SEP-58 verification steps 2-4).

use std::collections::BTreeSet;
use std::io::Read;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{Result, VerifyError};
use crate::sha256_hex;

/// Where a verification job's source comes from.
///
/// SEP-58 speaks only of `source_uri` + `source_sha256` (a content-addressed
/// archive). `Git` is our addition: the ecosystem's pre-SEP-58 contracts have
/// no `source_uri`, and a repo+commit is what a developer actually has to hand.
/// It feeds the same pipeline — a commit pins a tree just as a sha256 pins an
/// archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceRef {
    /// Public git repository at an exact revision.
    Git { repo: String, rev: String },
    /// Archive downloaded from `uri`, checked against `source_sha256`.
    Archive { uri: String, source_sha256: String },
}

/// uid/gid of the non-root `builder` user in the build image. Source is staged
/// under this ownership so the build (which runs as `builder`) can write its
/// `target/` directory into the tree.
pub const BUILDER_UID: u64 = 1000;

/// A token unique within this process, for naming scratch dirs and volumes.
///
/// The pid alone is not enough: Day2 runs verification jobs concurrently inside
/// one process, and two jobs sharing a scratch directory or a CARGO_HOME volume
/// would corrupt each other's build.
pub(crate) fn unique_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// A source tree staged for the container, as an uncompressed tar.
pub struct SourceArchive {
    /// Uncompressed tar bytes, ready for `docker cp -`.
    pub tar: Vec<u8>,
    /// The archive's single top-level directory (SEP-58 step 4).
    pub top_dir: String,
    /// sha256 of the bytes we fetched, as fetched.
    ///
    /// For `Archive` this is the checked `source_sha256`. For `Git` it is the
    /// digest of the tar we produced from the commit — recorded for the
    /// verification record, not compared against anything.
    pub sha256: String,
}

/// Cap on downloaded archive bytes. A verifier accepts URIs from strangers, so
/// the decompression/read path needs a bound that does not depend on the
/// server's honesty about Content-Length.
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

impl SourceRef {
    pub fn fetch(&self) -> Result<SourceArchive> {
        match self {
            SourceRef::Git { repo, rev } => fetch_git(repo, rev),
            SourceRef::Archive { uri, source_sha256 } => fetch_archive(uri, source_sha256),
        }
    }
}

/// Clone at `rev` and export the tree as a tar via `git archive`.
///
/// `git archive` writes the commit's tree and nothing else — no `.git`, no
/// untracked files, no local state — so the tar is a function of the commit
/// alone. `--prefix` gives us the single top-level directory SEP-58 wants.
fn fetch_git(repo: &str, rev: &str) -> Result<SourceArchive> {
    let tmp = std::env::temp_dir().join(format!("verify-src-{}", unique_token()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let guard = DirGuard(tmp.clone());

    let clone = Command::new("git")
        .args(["clone", "--quiet", "--no-checkout", repo])
        .arg(&tmp)
        .output()
        .map_err(|e| VerifyError::SourceFetch(format!("could not run git: {e}")))?;
    if !clone.status.success() {
        return Err(VerifyError::SourceFetch(format!(
            "git clone of `{repo}` failed: {}",
            String::from_utf8_lossy(&clone.stderr).trim()
        )));
    }

    let archive = Command::new("git")
        .arg("-C")
        .arg(&tmp)
        .args(["archive", "--format=tar", "--prefix=source/", rev])
        .output()
        .map_err(|e| VerifyError::SourceFetch(format!("could not run git archive: {e}")))?;
    if !archive.status.success() {
        return Err(VerifyError::SourceFetch(format!(
            "git archive of rev `{rev}` failed: {}",
            String::from_utf8_lossy(&archive.stderr).trim()
        )));
    }
    drop(guard);

    // Digest the tar as `git archive` produced it: the identity of the source
    // is the commit's tree, not our staging fixups.
    let sha256 = sha256_hex(&archive.stdout);
    let tar = normalize_ownership(&archive.stdout)?;
    Ok(SourceArchive { tar, top_dir: "source".into(), sha256 })
}

/// Download `uri`, check its digest, and normalise it to an uncompressed tar.
fn fetch_archive(uri: &str, expected_sha256: &str) -> Result<SourceArchive> {
    let resp = ureq::get(uri)
        .call()
        .map_err(|e| VerifyError::SourceFetch(format!("GET {uri} failed: {e}")))?;

    let mut bytes = Vec::new();
    resp.into_reader()
        .take(MAX_ARCHIVE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| VerifyError::SourceFetch(format!("reading {uri}: {e}")))?;
    if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(VerifyError::SourceFetch(format!(
            "source archive exceeds the {MAX_ARCHIVE_BYTES} byte limit"
        )));
    }

    // SEP-58 step 3: the digest covers the archive's bytes *as downloaded*, so
    // it must be checked before anything decompresses or unpacks them.
    let actual = sha256_hex(&bytes);
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(VerifyError::SourceIntegrity {
            expected: expected_sha256.to_lowercase(),
            actual,
        });
    }

    let tar = if is_gzip(&bytes) { gunzip(&bytes)? } else { bytes };
    let top_dir = single_top_dir(&tar)?;
    let tar = normalize_ownership(&tar)?;
    Ok(SourceArchive { tar, top_dir, sha256: actual })
}

fn is_gzip(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1f, 0x8b])
}

/// True for pax extended/global header pseudo-entries (tar types `x`/`g`).
///
/// These carry metadata for the archive or the following entry, not a file of
/// their own. The `tar` crate folds local (`x`) extensions into the entry they
/// precede but still surfaces the global (`g`) header — which GitHub codeload
/// tarballs always include — as a standalone entry named `pax_global_header`.
/// Every walk over the entries must skip them or it will mistake that header
/// for a real path.
fn is_pax_meta<R: Read>(entry: &tar::Entry<'_, R>) -> bool {
    let ty = entry.header().entry_type();
    ty.is_pax_global_extensions() || ty.is_pax_local_extensions()
}

/// Decompress with the `gzip` CLI rather than linking a decompressor.
///
/// The bytes are already digest-checked at this point, and this keeps the
/// dependency surface small for the MVP.
fn gunzip(bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new("gzip")
        .args(["-dc"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| VerifyError::SourceFetch(format!("could not run gzip: {e}")))?;
    child.stdin.take().expect("stdin was piped").write_all(bytes)?;
    let out = child
        .wait_with_output()
        .map_err(|e| VerifyError::SourceFetch(format!("gzip failed: {e}")))?;
    if !out.status.success() {
        return Err(VerifyError::SourceFetch(format!(
            "gunzip of source archive failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// Rewrite every entry's ownership to the build image's `builder` user.
///
/// `docker cp` restores the uid/gid recorded in the tar, and both of our source
/// paths produce root-owned entries (`git archive` hardcodes uid 0; a published
/// tarball carries whatever its author's machine had). Staged as-is, the tree
/// would be unwritable by the non-root build. Rewriting here keeps the build
/// itself unprivileged, rather than fixing it up by running the build as root.
///
/// This changes no file *content*, so it cannot affect the resulting WASM.
fn normalize_ownership(tar: &[u8]) -> Result<Vec<u8>> {
    let mut archive = tar::Archive::new(tar);
    let mut builder = tar::Builder::new(Vec::new());
    for entry in archive
        .entries()
        .map_err(|e| VerifyError::SourceFetch(format!("source archive is not a valid tar: {e}")))?
    {
        let mut entry =
            entry.map_err(|e| VerifyError::SourceFetch(format!("bad tar entry: {e}")))?;
        // Skip pax metadata pseudo-entries (e.g. the `pax_global_header` that
        // GitHub codeload tarballs carry). They are not files; the tar crate has
        // already folded any local extensions into the entries they precede.
        if is_pax_meta(&entry) {
            continue;
        }
        let mut header = entry.header().clone();
        header.set_uid(BUILDER_UID);
        header.set_gid(BUILDER_UID);

        let path = entry
            .path()
            .map_err(|e| VerifyError::SourceFetch(format!("bad tar entry path: {e}")))?
            .into_owned();
        let link = entry
            .link_name()
            .map_err(|e| VerifyError::SourceFetch(format!("bad tar link name: {e}")))?
            .map(|l| l.into_owned());

        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;

        if let Some(link) = link {
            // Link targets live in the header, not the body; set_cksum happens
            // inside append_link.
            builder
                .append_link(&mut header, &path, &link)
                .map_err(|e| VerifyError::SourceFetch(format!("rewriting tar link: {e}")))?;
        } else {
            header.set_cksum();
            builder
                .append_data(&mut header, &path, &data[..])
                .map_err(|e| VerifyError::SourceFetch(format!("rewriting tar entry: {e}")))?;
        }
    }
    builder
        .into_inner()
        .map_err(|e| VerifyError::SourceFetch(format!("finishing rewritten tar: {e}")))
}

/// SEP-58 step 4: the archive must contain exactly one top-level directory.
pub fn single_top_dir(tar: &[u8]) -> Result<String> {
    let mut archive = tar::Archive::new(tar);
    let mut tops = BTreeSet::new();
    for entry in archive
        .entries()
        .map_err(|e| VerifyError::SourceFetch(format!("source archive is not a valid tar: {e}")))?
    {
        let entry =
            entry.map_err(|e| VerifyError::SourceFetch(format!("bad tar entry: {e}")))?;
        // A `pax_global_header` (present in every GitHub codeload tarball) is
        // metadata, not a top-level directory — counting it would spuriously
        // trip the "exactly one top-level directory" check below.
        if is_pax_meta(&entry) {
            continue;
        }
        let path = entry
            .path()
            .map_err(|e| VerifyError::SourceFetch(format!("bad tar entry path: {e}")))?;
        // Reject traversal: `docker cp` unpacks this, and `..` or an absolute
        // path would let a crafted archive write outside the staging directory.
        for component in path.components() {
            use std::path::Component;
            if matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_))
            {
                return Err(VerifyError::SourceFetch(format!(
                    "source archive contains an unsafe path: {}",
                    path.display()
                )));
            }
        }
        if let Some(first) = path.components().next() {
            tops.insert(first.as_os_str().to_string_lossy().into_owned());
        }
    }
    if tops.len() != 1 {
        return Err(VerifyError::SourceLayout(tops.len()));
    }
    Ok(tops.into_iter().next().expect("checked len == 1"))
}

/// Best-effort cleanup of the clone scratch directory.
struct DirGuard(std::path::PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
