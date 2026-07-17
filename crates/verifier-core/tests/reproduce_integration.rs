//! End-to-end reproduction tests: the same discrimination table Day1 ran by
//! hand (`docs/day1-build-engine.md`), turned into automated checks.
//!
//! These drive real Docker containers and clone the published fixture, so they
//! are `#[ignore]` by default and do not run under a plain `cargo test`:
//!
//! ```text
//! cargo test -p verifier-core -- --ignored
//! ```
//!
//! Requirements:
//! - A working Docker daemon. On the Linux deploy target this is native Docker;
//!   on the Windows dev box it is Docker Engine inside WSL2 — `Docker::autodetect`
//!   shells into it. Override with `VERIFY_DOCKER`.
//! - The pinned build image present locally (see README, "Development").
//!
//! Overridable via env vars (defaults point at the published fixture):
//! - `FIXTURE_REPO` — git URL or local path of the hello-world fixture
//! - `FIXTURE_REV`  — the commit that reproduces the on-chain WASM
//! - `BLDIMG`       — the build image tag/digest

use std::path::Path;
use std::process::Command;

use verifier_core::{
    reproduce, Docker, ReproductionRequest, SourceRef, VerificationResult,
};

/// The published reproduction fixture (item 1). Its release build is
/// byte-identical to the Day0 on-chain WASM below.
const DEFAULT_REPO: &str = "https://github.com/erdemasik001/stellar-verify-fixture-hello-world";
const DEFAULT_REV: &str = "c08333e9924bfb45ee221f3edeb8ded4d4840397";
const DEFAULT_BLDIMG: &str = "stellar-verify/build-image:rust1.91.1-cli23.2.1";

/// The hash and size Day0 recorded and Day1 reproduces.
const EXPECTED_WASM: &str = "b68602842d3a1d169d54fe3e57c0511a774df4710553d6d4d22e653d62bf5f5b";
const EXPECTED_SIZE: usize = 660;

fn repo() -> String {
    std::env::var("FIXTURE_REPO").unwrap_or_else(|_| DEFAULT_REPO.into())
}
fn rev() -> String {
    std::env::var("FIXTURE_REV").unwrap_or_else(|_| DEFAULT_REV.into())
}
fn bldimg() -> String {
    std::env::var("BLDIMG").unwrap_or_else(|_| DEFAULT_BLDIMG.into())
}

/// A request against the fixture. The build image is local (no registry digest
/// yet), so unpinned images are allowed here — the same footing Day1 ran on.
fn request(source: SourceRef, expected: &str) -> ReproductionRequest {
    let mut req = ReproductionRequest::new(source, bldimg(), expected);
    req.allow_unpinned_image = true;
    req
}

// --- The four cases from docs/day1-build-engine.md ---

/// Correct source + correct hash → `verified`, exit 0.
#[test]
#[ignore = "requires Docker and the pinned build image"]
fn verified_correct_source_and_hash() {
    let report = reproduce(
        &Docker::autodetect(),
        &request(SourceRef::Git { repo: repo(), rev: rev() }, EXPECTED_WASM),
    )
    .expect("reproduction should succeed");

    assert_eq!(report.result, VerificationResult::Verified);
    assert_eq!(report.rebuilt_wasm_sha256, EXPECTED_WASM);
    assert_eq!(report.rebuilt_wasm_size, EXPECTED_SIZE);
}

/// Correct source, wrong expected hash → `mismatch`. The build is identical; only
/// the comparison differs, so the rebuilt hash is still the real one.
#[test]
#[ignore = "requires Docker and the pinned build image"]
fn mismatch_wrong_expected_hash() {
    let wrong = "0000000000000000000000000000000000000000000000000000000000000000";
    let report = reproduce(
        &Docker::autodetect(),
        &request(SourceRef::Git { repo: repo(), rev: rev() }, wrong),
    )
    .expect("build succeeds; only the hash comparison fails");

    assert_eq!(report.result, VerificationResult::Mismatch);
    assert_eq!(report.rebuilt_wasm_sha256, EXPECTED_WASM);
}

/// Tampered source (one word, committed) → `mismatch`. The engine builds the
/// altered tree, so the WASM genuinely differs from the on-chain hash.
#[test]
#[ignore = "requires Docker and the pinned build image"]
fn mismatch_tampered_source() {
    let clone = clone_fixture();
    let lib = clone.path().join("contracts/hello-world/src/lib.rs");
    let original = std::fs::read_to_string(&lib).expect("read lib.rs");
    let tampered = original.replace("\"Hello\"", "\"Howdy\"");
    assert_ne!(original, tampered, "expected a `Hello` literal to tamper with");
    std::fs::write(&lib, tampered).expect("write tampered lib.rs");
    git(clone.path(), &["commit", "--quiet", "-am", "tamper: Hello -> Howdy"]);
    let head = git_stdout(clone.path(), &["rev-parse", "HEAD"]);

    let report = reproduce(
        &Docker::autodetect(),
        &request(SourceRef::Git { repo: path_str(clone.path()), rev: head }, EXPECTED_WASM),
    )
    .expect("tampered source still builds");

    assert_eq!(report.result, VerificationResult::Mismatch);
    assert_ne!(report.rebuilt_wasm_sha256, EXPECTED_WASM);
}

/// Original rev, tampered *working tree* → `verified`. `git archive` exports the
/// commit, never the dirty working tree, so an uncommitted change cannot move
/// the hash. This is the row that proves determinism, not just sensitivity.
#[test]
#[ignore = "requires Docker and the pinned build image"]
fn verified_original_rev_despite_dirty_working_tree() {
    let clone = clone_fixture();
    let original = git_stdout(clone.path(), &["rev-parse", "HEAD"]);
    // Dirty the working tree WITHOUT committing it.
    let lib = clone.path().join("contracts/hello-world/src/lib.rs");
    let src = std::fs::read_to_string(&lib).expect("read lib.rs");
    std::fs::write(&lib, src.replace("\"Hello\"", "\"Howdy\"")).expect("dirty lib.rs");

    let report = reproduce(
        &Docker::autodetect(),
        &request(SourceRef::Git { repo: path_str(clone.path()), rev: original }, EXPECTED_WASM),
    )
    .expect("reproduction should succeed");

    assert_eq!(report.result, VerificationResult::Verified);
    assert_eq!(report.rebuilt_wasm_sha256, EXPECTED_WASM);
    assert_eq!(report.rebuilt_wasm_size, EXPECTED_SIZE);
}

// --- helpers ---

/// Clone the fixture into a throwaway directory the caller can tamper with.
///
/// The engine re-clones from this path internally, and a local clone carries the
/// commit objects independently of its working tree — which is exactly what lets
/// the "dirty working tree" case be set up here and still build the commit.
fn clone_fixture() -> TempDir {
    let dir = TempDir::new();
    let out = Command::new("git")
        .args(["clone", "--quiet"])
        .arg(repo())
        .arg(dir.path())
        .output()
        .expect("run git clone");
    assert!(
        out.status.success(),
        "cloning fixture failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    // Identity so the tamper case can commit.
    git(dir.path(), &["config", "user.email", "test@example.invalid"]);
    git(dir.path(), &["config", "user.name", "reproduce-integration"]);
    dir
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
}

fn git_stdout(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// `git clone` accepts forward slashes on every platform; backslashes it does not.
fn path_str(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// A temp directory removed on drop; avoids a `tempfile` dev-dependency.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("verify-it-{}-{}", std::process::id(), nanos));
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
