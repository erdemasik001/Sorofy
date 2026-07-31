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
    let tar = normalize_ownership(&archive.stdout)?;
    Ok(SourceArchive {
        tar,
        top_dir: "source".into(),
        sha256,
    })
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
    let top_dir = single_top_dir(&tar)?;
    let tar = normalize_ownership(&tar)?;
    Ok(SourceArchive {
        tar,
        top_dir,
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
}
