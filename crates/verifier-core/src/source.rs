//! Obtaining the source archive to rebuild (SEP-58 verification steps 2-4).

use std::collections::BTreeSet;
use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::process::Command;

use serde::{Deserialize, Serialize};
use url::Url;

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

/// The top-level directory every staged source tree is rewritten to.
///
/// Both source shapes must build at the *same* absolute path inside the
/// container. They did not: `git archive --prefix=source/` staged at
/// `/build/source`, while an archive kept whatever top directory its author
/// chose (GitHub's tarballs use `<repo>-<sha>`), staging at `/build/<repo>-<sha>`.
///
/// That difference is a correctness problem for multi-verifier agreement
/// (roadmap Phase 3), not a tidiness one. `--remap-path-prefix` only covers
/// `$CARGO_HOME/registry/src`, so nothing normalises the *workdir* path. If a
/// build ever embeds its absolute path — a `build.rs` writing `env!("PWD")`, a
/// panic message, a debug section — then two honest verifiers handed the same
/// commit in different shapes (one as a repo, one as a tarball) would produce
/// different bytes and report `disagreement` about an honest contract. Staging
/// both at a constant removes the variable.
pub const STAGED_TOP_DIR: &str = "source";

/// A token unique within this process, for naming scratch dirs and volumes.
///
/// The pid alone is not enough: Day2 runs verification jobs concurrently inside
/// one process, and two jobs sharing a scratch directory or a CARGO_HOME volume
/// would corrupt each other's build.
pub(crate) fn unique_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// A source tree staged for the container, as an uncompressed tar.
///
/// There is deliberately no `top_dir` field: every staged tar is re-rooted onto
/// [`STAGED_TOP_DIR`], so the staging path is a constant rather than something a
/// caller could get wrong or a submitter could influence.
pub struct SourceArchive {
    /// Uncompressed tar bytes, ready for `docker cp -`, rooted at
    /// [`STAGED_TOP_DIR`].
    pub tar: Vec<u8>,
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

/// Redirect hops to follow on the archive path before giving up. Matches ureq's
/// historical default — ample for the one legitimate hop (github.com → codeload)
/// with margin, and bounded so a redirect loop cannot spin.
const MAX_REDIRECTS: u32 = 5;

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
    // Block SSRF for URL repos (docs/security.md, G4). Local paths and ssh
    // remotes (used by the CLI and tests) carry no `http(s)://` scheme and are
    // left alone — the service accepts https repos from strangers, a shell does
    // not.
    if repo.starts_with("http://") || repo.starts_with("https://") {
        guard_public_url(repo)?;
    }

    let tmp = std::env::temp_dir().join(format!("verify-src-{}", unique_token()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let guard = DirGuard(tmp.clone());

    let clone = Command::new("git")
        .args([
            // Do not follow a redirect to a host we never validated. We checked
            // `repo`'s host, and a smart-HTTP git host serves the repo there
            // directly, so a cross-host 3xx here would be an SSRF hop past the
            // guard (docs/security.md, G4). git's default is `initial` (follow the
            // first request's redirect), which is exactly that hole. The archive
            // path re-validates each hop instead, because it has a legitimate
            // github.com → codeload redirect; git clone has no such need.
            "-c",
            "http.followRedirects=false",
            "clone",
            "--quiet",
            "--no-checkout",
            repo,
        ])
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
    // `--prefix=source/` above already matches STAGED_TOP_DIR, so the re-rooting
    // is a no-op here; it runs anyway so the invariant has exactly one enforcer.
    let tar = normalize_for_staging(&archive.stdout)?;
    Ok(SourceArchive { tar, sha256 })
}

/// Download `uri`, check its digest, and normalise it to an uncompressed tar.
fn fetch_archive(uri: &str, expected_sha256: &str) -> Result<SourceArchive> {
    // Follow redirects ourselves so every hop is SSRF-checked, not just the
    // first URL (docs/security.md, G4).
    let resp = get_with_guarded_redirects(uri)?;

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

    let tar = if is_gzip(&bytes) {
        gunzip(&bytes)?
    } else {
        bytes
    };
    // SEP-58 step 4 plus the traversal check. Its verdict is what makes the
    // re-rooting below well-defined: exactly one top-level component, and no
    // entry that could escape it.
    single_top_dir(&tar)?;
    let tar = normalize_for_staging(&tar)?;
    Ok(SourceArchive {
        tar,
        sha256: actual,
    })
}

/// GET `url`, following redirects manually so **every** hop is SSRF-checked.
///
/// [`guard_public_url`] only ever saw the first URL, but ureq follows up to 5
/// redirects on its own — so a public URL could `302` to `169.254.169.254` (or
/// any RFC-1918 address) and reach it before we ever looked (docs/security.md,
/// G4). We disable ureq's auto-follow and re-validate each `Location` before
/// dialing it.
///
/// Redirects are re-validated, not disabled: GitHub's `/archive/<sha>.tar.gz`
/// legitimately `302`s `github.com` → `codeload.github.com`, and the archive
/// (`source_uri`) path depends on that hop.
fn get_with_guarded_redirects(url: &str) -> Result<ureq::Response> {
    let agent = ureq::builder().redirects(0).build();
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        // Validate the host we are *about* to dial — the initial URL on the first
        // pass, each redirect target on later passes.
        guard_public_url(&current)?;
        let resp = agent
            .get(&current)
            .call()
            .map_err(|e| VerifyError::SourceFetch(format!("GET {current} failed: {e}")))?;
        if !(300..400).contains(&resp.status()) {
            return Ok(resp);
        }
        let location = resp.header("location").ok_or_else(|| {
            VerifyError::SourceFetch(format!(
                "redirect from `{current}` (status {}) had no Location header",
                resp.status()
            ))
        })?;
        current = resolve_redirect(&current, location)?;
    }
    Err(VerifyError::SourceFetch(format!(
        "source URL redirected more than {MAX_REDIRECTS} times; refusing"
    )))
}

/// Resolve a redirect `Location` — absolute or relative — against the URL that
/// produced it, yielding the absolute URL of the next hop.
fn resolve_redirect(current: &str, location: &str) -> Result<String> {
    let base = Url::parse(current)
        .map_err(|e| VerifyError::SourceFetch(format!("invalid redirect base `{current}`: {e}")))?;
    let next = base.join(location).map_err(|e| {
        VerifyError::SourceFetch(format!("invalid redirect target `{location}`: {e}"))
    })?;
    Ok(next.to_string())
}

/// Reject a submitter-supplied URL that points back at the host's own network
/// (docs/security.md, G4 — SSRF).
///
/// A verifier fetches source from URLs strangers choose. Without this, a request
/// could name `http://169.254.169.254/…` (cloud metadata), `http://localhost:…`,
/// or an RFC-1918 address to probe or exfiltrate from inside the deploy network.
/// We require `http`/`https`, resolve the host, and refuse if *any* resolved
/// address is non-public.
///
/// Residual: this is a check-then-connect gap — DNS rebinding could return a
/// public address here and an internal one when the fetch actually dials. Closing
/// that needs IP-pinned dialing or an egress proxy; tracked in docs/security.md.
fn guard_public_url(raw: &str) -> Result<()> {
    let url = Url::parse(raw)
        .map_err(|e| VerifyError::SourceFetch(format!("invalid source URL `{raw}`: {e}")))?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(VerifyError::SourceFetch(format!(
                "source URL scheme `{other}` is not allowed; use https"
            )))
        }
    }
    let host = url
        .host_str()
        .ok_or_else(|| VerifyError::SourceFetch(format!("source URL `{raw}` has no host")))?;
    let port = url.port_or_known_default().unwrap_or(443);

    // Resolve and inspect every address the host maps to; a hostname that
    // resolves to an internal address is refused just like a literal one.
    let mut resolved = false;
    for addr in (host, port).to_socket_addrs().map_err(|e| {
        VerifyError::SourceFetch(format!("cannot resolve source host `{host}`: {e}"))
    })? {
        resolved = true;
        if is_internal(addr.ip()) {
            return Err(VerifyError::SourceFetch(format!(
                "source host `{host}` resolves to a non-public address ({}); refusing (SSRF guard)",
                addr.ip()
            )));
        }
    }
    if !resolved {
        return Err(VerifyError::SourceFetch(format!(
            "source host `{host}` did not resolve to any address"
        )));
    }
    Ok(())
}

/// Whether `ip` is one a public fetch has no business reaching: loopback,
/// link-local (incl. the `169.254.169.254` cloud-metadata endpoint), private,
/// carrier-grade NAT, or unspecified. IPv6 ranges are matched on the raw
/// segments to avoid depending on still-unstable `Ipv6Addr` helpers.
fn is_internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.octets()[0] == 0
                // 100.64.0.0/10, carrier-grade NAT
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 0x40)
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return true;
            }
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_internal(IpAddr::V4(mapped));
            }
            let head = v6.segments()[0];
            // fc00::/7 unique-local, or fe80::/10 link-local.
            (head & 0xfe00) == 0xfc00 || (head & 0xffc0) == 0xfe80
        }
    }
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
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(bytes)?;
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

/// Normalise a fetched tar into the shape we stage: `builder`-owned, rooted at
/// [`STAGED_TOP_DIR`].
///
/// **Ownership.** `docker cp` restores the uid/gid recorded in the tar, and both
/// of our source paths produce root-owned entries (`git archive` hardcodes uid 0;
/// a published tarball carries whatever its author's machine had). Staged as-is,
/// the tree would be unwritable by the non-root build. Rewriting here keeps the
/// build itself unprivileged, rather than fixing it up by running the build as
/// root.
///
/// **Top directory.** Every entry is re-rooted onto [`STAGED_TOP_DIR`] so the git
/// and archive paths build at one absolute path — see that constant for why.
///
/// Neither rewrite touches file *content*. The path rewrite does change the
/// build's working directory for the archive path, which is precisely the point;
/// that it does not perturb the output is proven live, not assumed, by
/// `reproduce_integration::verified_archive_source_uri` reproducing the fixture's
/// on-chain hash byte-for-byte.
fn normalize_for_staging(tar: &[u8]) -> Result<Vec<u8>> {
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
        // Build a fresh header instead of cloning the fetched one. tar headers
        // carry several fixed-width name fields — `name`, `linkname`, and ustar's
        // `prefix`, which holds the leading directories of a path too long for
        // `name`. Writing a shorter value into one of them leaves the old bytes
        // in the others, and GitHub's tarballs do use the `prefix` split: a
        // cloned header re-rooted to `source/…` extracted as
        // `<old prefix>/source/…`, so nested files landed off-tree and the build
        // failed to read its own workspace members. Starting from zero cannot
        // inherit a stale field, and it makes every staged tar one canonical
        // format regardless of what shape we fetched.
        let source_header = entry.header();
        let entry_type = source_header.entry_type();
        // Carried over, with a default when the field is unreadable. The previous
        // implementation copied the header wholesale and so never parsed these;
        // rejecting an archive over a malformed mode would be a new strictness
        // this change has no business introducing. Mode still matters (an
        // executable bit on a build script), so the default follows the entry
        // type rather than being uniform.
        let mode = source_header
            .mode()
            .unwrap_or(if entry_type.is_dir() { 0o755 } else { 0o644 });
        let mtime = source_header.mtime().unwrap_or(0);

        let path = restage_path(&entry.path_bytes())?;
        let link = entry.link_name_bytes().map(|l| l.into_owned());

        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;

        let mut header = tar::Header::new_gnu();
        header.set_entry_type(entry_type);
        header.set_mode(mode);
        header.set_mtime(mtime);
        header.set_size(data.len() as u64);
        header.set_uid(BUILDER_UID);
        header.set_gid(BUILDER_UID);

        if let Some(link) = link {
            // A *hard* link's target is a path within the archive, so it carries
            // the old top directory and has to be re-rooted with everything else.
            // A *symlink*'s target is resolved at extraction relative to the link,
            // so renaming the top directory leaves it correct — rewriting it would
            // break it. (Absolute or escaping symlink targets are neither created
            // nor validated here; that residual is docs/security.md S5.)
            let link = if entry_type == tar::EntryType::Link {
                restage_path(&link)?
            } else {
                entry_name(&link)?.to_owned()
            };
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

/// Replace a tar entry path's first component with [`STAGED_TOP_DIR`].
///
/// Works on the raw stored bytes rather than a `Path`: tar names are always
/// `/`-separated, while `Path` semantics differ per platform (on Windows `\` is
/// also a separator, and rebuilding a path there would emit the wrong bytes).
/// Everything after the first component is copied verbatim, including the
/// trailing `/` that marks a directory entry.
///
/// Callers must have established that there *is* exactly one top-level component
/// and no traversal — [`single_top_dir`] for the archive path, `git archive
/// --prefix` for the git one.
fn restage_path(raw: &[u8]) -> Result<String> {
    let rest = match raw.iter().position(|&b| b == b'/') {
        Some(slash) => entry_name(&raw[slash..])?,
        // No separator: this is the top-level directory entry itself.
        None => "",
    };
    Ok(format!("{STAGED_TOP_DIR}{rest}"))
}

/// A tar entry name as UTF-8.
///
/// tar stores names as bytes, so this is a real (if narrow) restriction: an
/// archive with a non-UTF-8 filename is refused rather than staged. Cargo already
/// requires UTF-8 paths, and an untrusted archive carrying names that cannot be
/// round-tripped is a smell — refusing beats guessing at an encoding.
fn entry_name(raw: &[u8]) -> Result<&str> {
    std::str::from_utf8(raw)
        .map_err(|_| VerifyError::SourceFetch("source archive has a non-UTF-8 entry name".into()))
}

/// SEP-58 step 4: the archive must contain exactly one top-level directory.
pub fn single_top_dir(tar: &[u8]) -> Result<String> {
    let mut archive = tar::Archive::new(tar);
    let mut tops = BTreeSet::new();
    for entry in archive
        .entries()
        .map_err(|e| VerifyError::SourceFetch(format!("source archive is not a valid tar: {e}")))?
    {
        let entry = entry.map_err(|e| VerifyError::SourceFetch(format!("bad tar entry: {e}")))?;
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
            if matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            ) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn internal_addresses_are_flagged() {
        let internal = [
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),       // loopback
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)), // cloud metadata (link-local)
            IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)),        // private
            IpAddr::V4(Ipv4Addr::new(192, 168, 0, 5)),     // private
            IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)),      // private
            IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),      // CGNAT
            IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),         // unspecified
            IpAddr::V6(Ipv6Addr::LOCALHOST),               // ::1
            IpAddr::V6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1)), // unique-local
            IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)), // link-local
            IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 0x0001)), // ::ffff:127.0.0.1
        ];
        for ip in internal {
            assert!(is_internal(ip), "{ip} should be flagged internal");
        }
    }

    #[test]
    fn public_addresses_are_allowed() {
        for ip in [
            IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            IpAddr::V4(Ipv4Addr::new(140, 82, 121, 3)), // github.com range
            IpAddr::V6(Ipv6Addr::new(0x2606, 0x4700, 0, 0, 0, 0, 0, 1)),
        ] {
            assert!(!is_internal(ip), "{ip} should be allowed");
        }
    }

    #[test]
    fn guard_rejects_internal_and_non_http_urls() {
        // Literal internal addresses — parsed, not DNS-resolved, so offline.
        for url in [
            "http://127.0.0.1/x",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.5/repo.tar.gz",
            "http://[::1]/x",
            "https://localhost/x", // resolves via hosts file, still internal
        ] {
            assert!(guard_public_url(url).is_err(), "{url} must be refused");
        }
        // Non-http schemes are refused before any lookup.
        assert!(guard_public_url("file:///etc/passwd").is_err());
        assert!(guard_public_url("gopher://example.com/").is_err());
        // Garbage is a fetch error, not a panic.
        assert!(guard_public_url("not a url").is_err());
    }

    #[test]
    fn resolve_redirect_handles_absolute_and_relative_targets() {
        // Absolute Location replaces the whole URL (the github.com → codeload hop).
        assert_eq!(
            resolve_redirect(
                "https://github.com/u/r/archive/abc.tar.gz",
                "https://codeload.github.com/u/r/tar.gz/abc"
            )
            .unwrap(),
            "https://codeload.github.com/u/r/tar.gz/abc"
        );
        // Relative Location resolves against the current URL's origin.
        assert_eq!(
            resolve_redirect("https://example.com/a/b", "/c/d").unwrap(),
            "https://example.com/c/d"
        );
        // A garbage Location is a fetch error, not a panic.
        assert!(resolve_redirect("https://example.com/", "http://[bad").is_err());
    }

    #[test]
    fn a_redirect_target_to_an_internal_host_is_refused() {
        // Per-hop safety is `guard_public_url` applied to the *resolved* target:
        // a 302 → cloud metadata is caught before the next dial, even though the
        // first host (github.com) was public.
        let next =
            resolve_redirect("https://github.com/u/r", "http://169.254.169.254/latest/").unwrap();
        assert!(
            guard_public_url(&next).is_err(),
            "a redirect to the metadata endpoint must be refused"
        );
    }

    // --- Staging normalisation (STAGED_TOP_DIR) ---

    /// A source tree under `top`, in the shape a fetched tar arrives in.
    fn tar_under(top: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut dir = tar::Header::new_gnu();
        dir.set_entry_type(tar::EntryType::Directory);
        dir.set_size(0);
        dir.set_mode(0o755);
        dir.set_cksum();
        builder
            .append_data(&mut dir, format!("{top}/"), &b""[..])
            .unwrap();
        for (name, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("{top}/{name}"), *data)
                .unwrap();
        }
        builder.into_inner().unwrap()
    }

    /// Every entry's path, and its link target when it has one.
    fn entries_of(tar: &[u8]) -> Vec<(String, Option<String>)> {
        tar::Archive::new(tar)
            .entries()
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.path().unwrap().to_string_lossy().replace('\\', "/"),
                    e.link_name()
                        .unwrap()
                        .map(|l| l.to_string_lossy().replace('\\', "/")),
                )
            })
            .collect()
    }

    #[test]
    fn staging_reroots_an_archive_onto_the_constant_top_dir() {
        // GitHub's tarballs are named <repo>-<sha>; that name must not reach the
        // container, or the archive path builds somewhere the git path never does.
        let tar = tar_under(
            "sorofy-fixture-token-cd68767",
            &[("Cargo.toml", b"[package]"), ("src/lib.rs", b"// x")],
        );
        let staged = normalize_for_staging(&tar).unwrap();

        let paths: Vec<String> = entries_of(&staged).into_iter().map(|(p, _)| p).collect();
        // The directory entry keeps its trailing `/`: only the first component is
        // replaced, everything after it is copied verbatim.
        assert_eq!(paths, ["source/", "source/Cargo.toml", "source/src/lib.rs"]);
    }

    #[test]
    fn the_git_and_archive_shapes_stage_to_identical_bytes() {
        // The invariant the whole change exists for: the same tree fetched as a
        // repo and as a tarball must be indistinguishable once staged, so two
        // verifiers handed different shapes cannot manufacture a `disagreement`.
        let files: &[(&str, &[u8])] = &[("Cargo.toml", b"[package]"), ("src/lib.rs", b"// x")];
        let from_git = normalize_for_staging(&tar_under("source", files)).unwrap();
        let from_archive = normalize_for_staging(&tar_under("mycrate-abc123", files)).unwrap();

        assert_eq!(
            from_git, from_archive,
            "staged tars must be byte-identical regardless of the fetched shape"
        );
    }

    #[test]
    fn a_path_too_long_for_the_name_field_survives_restaging() {
        // GitHub roots its tarballs at <repo>-<40-char sha>, so any nested path
        // overflows tar's 100-byte name field and travels in a long-name/pax
        // extension entry. Re-rooting has to produce a tree the build can
        // actually find: this is the case the live archive reproduction failed
        // on with "failed to read /build/source/contracts/hello-world/Cargo.toml".
        let top = "stellar-verify-fixture-hello-world-c08333e9924bfb45ee221f3edeb8ded4d4840397";
        let tar = tar_under(top, &[("contracts/hello-world/Cargo.toml", b"[package]")]);

        let staged = normalize_for_staging(&tar).unwrap();
        let paths: Vec<String> = entries_of(&staged).into_iter().map(|(p, _)| p).collect();
        assert_eq!(
            paths,
            ["source/", "source/contracts/hello-world/Cargo.toml"]
        );
    }

    #[test]
    fn ownership_is_rewritten_to_the_builder_user() {
        // The other half of staging: `git archive` hardcodes uid 0, and the build
        // runs as `builder` and must be able to write `target/` into the tree.
        let staged = normalize_for_staging(&tar_under("x", &[("Cargo.toml", b"")])).unwrap();
        for entry in tar::Archive::new(&staged[..]).entries().unwrap() {
            let entry = entry.unwrap();
            assert_eq!(entry.header().uid().unwrap(), BUILDER_UID);
            assert_eq!(entry.header().gid().unwrap(), BUILDER_UID);
        }
    }

    #[test]
    fn a_hard_link_target_is_rerooted_but_a_symlink_target_is_not() {
        // Hard-link targets are archive-internal paths, so they carry the old top
        // dir; symlink targets resolve relative to the link and must survive
        // untouched. Getting this backwards yields a dangling link at extraction.
        let mut builder = tar::Builder::new(Vec::new());
        let mut file = tar::Header::new_gnu();
        file.set_size(4);
        file.set_mode(0o644);
        file.set_cksum();
        builder
            .append_data(&mut file, "pkg-1a2b/real.txt", &b"data"[..])
            .unwrap();

        let mut hard = tar::Header::new_gnu();
        hard.set_entry_type(tar::EntryType::Link);
        hard.set_size(0);
        hard.set_mode(0o644);
        builder
            .append_link(&mut hard, "pkg-1a2b/hard.txt", "pkg-1a2b/real.txt")
            .unwrap();

        let mut sym = tar::Header::new_gnu();
        sym.set_entry_type(tar::EntryType::Symlink);
        sym.set_size(0);
        sym.set_mode(0o777);
        builder
            .append_link(&mut sym, "pkg-1a2b/soft.txt", "real.txt")
            .unwrap();

        let staged = normalize_for_staging(&builder.into_inner().unwrap()).unwrap();
        let entries = entries_of(&staged);

        assert_eq!(entries[0].0, "source/real.txt");
        assert_eq!(
            entries[1],
            ("source/hard.txt".into(), Some("source/real.txt".into()))
        );
        assert_eq!(
            entries[2],
            ("source/soft.txt".into(), Some("real.txt".into()))
        );
    }

    #[test]
    fn a_non_utf8_entry_name_is_refused() {
        // Staging rewrites names, so they must round-trip as text. An archive that
        // cannot is refused rather than staged under a guessed encoding.
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        let name = b"pkg/\xff.rs";
        header.as_old_mut().name[..name.len()].copy_from_slice(name);
        header.set_cksum();

        let mut builder = tar::Builder::new(Vec::new());
        builder.append(&header, &b""[..]).unwrap();
        let tar = builder.into_inner().unwrap();

        assert!(matches!(
            normalize_for_staging(&tar),
            Err(VerifyError::SourceFetch(_))
        ));
    }
}
