//! The reproduction pipeline: source + build metadata -> WASM -> hash compare.
//!
//! This is SEP-58's verification algorithm (`docs/sep-58-notes.md`), minus the
//! metadata retrieval step, which is the Day2 API's job.

use std::io::Read;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::docker::Docker;
use crate::docker::{ContainerSpec, Network, Volume};
use crate::error::{Result, VerifyError};
use crate::source::{SourceArchive, SourceRef};
use crate::{sha256_hex, TrustLevel, VerificationResult};

/// Where the source tree is staged inside the container.
const STAGE_DIR: &str = "/build";

/// Where the shared `CARGO_HOME` volume is mounted in both phases.
const CARGO_HOME: &str = "/cargo-home";

/// Only target we build for; SEP-58 contracts are `wasm32v1-none`.
const WASM_TARGET: &str = "wasm32v1-none";

/// Default wall-clock budget for one build.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Share of the job's budget the dependency fetch may use before we call it.
const FETCH_TIMEOUT_FRACTION: u32 = 3;

/// cgroup caps applied to the fetch and build containers (docs/security.md, G1).
///
/// A build compiles and runs untrusted code (`build.rs`, proc macros); without
/// these a hostile source could OOM the host, fork-bomb it, or peg every core.
/// Generous enough for a real-size contract build; a deploy target with less RAM
/// gets the tighter of these and its own VM limit. Tune here if a legitimately
/// large contract needs more headroom.
const BUILD_LIMITS: crate::docker::ResourceLimits<'static> = crate::docker::ResourceLimits {
    memory: Some("3g"),
    // Equal to `memory` so swap cannot lift the effective ceiling to ~2× on a
    // swap-enabled host.
    memory_swap: Some("3g"),
    cpus: Some("2"),
    pids: Some(2048),
    // No disk quota: `--storage-opt size=` needs a quota-capable storage driver
    // (see `ResourceLimits::storage_opt_size`), which the deploy target is not
    // guaranteed to have. Enable per-deploy once the driver is known to support
    // it; a size-bounded volume/tmpfs is the driver-independent alternative.
    storage_opt_size: None,
};

/// Privilege hardening applied to the fetch and build containers (docs/security.md, G6).
///
/// A compile needs no Linux capabilities and the build already runs non-root, so
/// dropping all caps and blocking setuid escalation costs nothing and shrinks the
/// escape surface. Applied to both phases: fetch resolves an attacker-controlled
/// `Cargo.toml`, build runs its `build.rs`.
const BUILD_SECURITY: crate::docker::SecurityOpts = crate::docker::SecurityOpts {
    cap_drop_all: true,
    no_new_privileges: true,
};

/// Which docker network the fetch phase runs on (docs/adr/0001-fetch-egress-control.md).
///
/// `VERIFY_FETCH_NETWORK` names a pre-created, egress-filtered network; unset
/// keeps the default bridge, i.e. today's behaviour. Opt-in because the control
/// that makes it *mean* anything — host firewall rules dropping traffic from that
/// network to internal ranges — is deploy config (roadmap 0.7): defaulting it on
/// would name a network that does not exist and break every build, and silently
/// creating one would give unfiltered egress under a name that implies filtering.
///
/// The build phase is unaffected: it is `--network=none` and reaches nothing.
fn fetch_network() -> Network<'static> {
    static NETWORK: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let configured = NETWORK.get_or_init(|| {
        std::env::var("VERIFY_FETCH_NETWORK")
            .ok()
            .filter(|s| !s.trim().is_empty())
    });
    network_for(configured.as_deref())
}

/// Pure selection behind [`fetch_network`], so the mapping is unit-testable
/// without touching process environment.
fn network_for(configured: Option<&str>) -> Network<'_> {
    match configured {
        Some(name) if !name.trim().is_empty() => Network::Named(name),
        _ => Network::Bridge,
    }
}

/// One reproduction job.
#[derive(Debug, Clone)]
pub struct ReproductionRequest {
    pub source: SourceRef,
    /// SEP-58 `bldimg`: the build image, pinned by digest.
    pub bldimg: String,
    /// SEP-58 `bldopt`: flags passed verbatim to `stellar contract build`.
    pub bldopt: Vec<String>,
    /// The on-chain WASM hash to reproduce.
    pub expected_wasm_sha256: String,
    pub timeout: Duration,
    /// Allow a `bldimg` that is not digest-pinned.
    ///
    /// Off by default: SEP-58 requires a digest because a tag can be moved,
    /// which would silently invalidate every past verification made against it.
    /// We only relax this to build against a locally-built image that has no
    /// registry digest yet.
    pub allow_unpinned_image: bool,
    /// Write the rebuilt `.wasm` bytes to this path after extraction.
    ///
    /// The report only carries the hash; the bytes themselves are needed when
    /// the rebuilt artifact is the deliverable — e.g. deploying exactly what a
    /// reproduction produced, so the on-chain hash is *by construction* one the
    /// engine can land on again.
    pub emit_wasm: Option<std::path::PathBuf>,
}

impl ReproductionRequest {
    pub fn new(
        source: SourceRef,
        bldimg: impl Into<String>,
        expected_wasm_sha256: impl Into<String>,
    ) -> Self {
        ReproductionRequest {
            source,
            bldimg: bldimg.into(),
            bldopt: Vec::new(),
            expected_wasm_sha256: expected_wasm_sha256.into(),
            timeout: DEFAULT_TIMEOUT,
            allow_unpinned_image: false,
            emit_wasm: None,
        }
    }
}

/// The outcome of a reproduction, verified or not.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReproductionReport {
    pub result: VerificationResult,
    pub expected_wasm_sha256: String,
    pub rebuilt_wasm_sha256: String,
    pub rebuilt_wasm_size: usize,
    /// The `.wasm` path inside the container, relative to the crate root.
    pub artifact: String,
    pub bldimg: String,
    /// The `repo@sha256:...` digest the daemon actually resolved `bldimg` to.
    ///
    /// This is the image the WASM was really built from — the honest pin for the
    /// verification record, even when `bldimg` was passed as a movable tag under
    /// `--allow-unpinned-image`. `None` for a locally-built image that was never
    /// pushed to a registry, so it has no digest to resolve.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bldimg_digest: Option<String>,
    pub bldopt: Vec<String>,
    /// sha256 of the source archive actually built.
    pub source_sha256: String,
    pub trust_level: TrustLevel,
    pub build_seconds: f64,
    /// Build log, kept for `mismatch` so a developer can see what differed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_log: Option<String>,
}

/// Rebuild `request.source` in `request.bldimg` and compare hashes.
pub fn reproduce(docker: &Docker, request: &ReproductionRequest) -> Result<ReproductionReport> {
    if !request.allow_unpinned_image && !request.bldimg.contains("@sha256:") {
        return Err(VerifyError::UnpinnedImage(request.bldimg.clone()));
    }
    docker.preflight()?;

    tracing::info!(source = ?request.source, "fetching source");
    let source = request.source.fetch()?;
    // A constant, not a property of the fetched source: `normalize_for_staging`
    // re-roots every tree onto STAGED_TOP_DIR so the git and archive paths build
    // at the same absolute path (see that constant — Phase 3 prerequisite).
    let workdir = format!("{STAGE_DIR}/{}", crate::source::STAGED_TOP_DIR);

    // A contract's dependencies live on crates.io, but the build must not have
    // network access — so the job is split in two, sharing one CARGO_HOME:
    //
    //   1. `cargo fetch --locked` WITH network. Downloads exactly the versions
    //      the committed Cargo.lock names, and runs none of their code.
    //   2. `stellar contract build` with `--network=none`, resolving offline
    //      from what phase 1 left in the volume.
    //
    // The split is what makes isolation possible at all: a build compiles and
    // runs untrusted code (build.rs, proc macros), and that is the phase that
    // must not reach the network. Fetching is not.
    let cargo_home =
        docker.create_volume(&format!("verify-cargo-{}", crate::source::unique_token()))?;

    let fetch_timeout = request.timeout / FETCH_TIMEOUT_FRACTION;
    let started = Instant::now();
    fetch_dependencies(
        docker,
        request,
        &source,
        &workdir,
        &cargo_home,
        fetch_timeout,
    )?;

    // `stellar contract build` runs against the image's baked-in toolchain. It
    // has no --locked of its own; lockfile adherence is already guaranteed from
    // both sides: phase 1's `cargo fetch --locked` fails on a missing or stale
    // Cargo.lock, and this phase can only resolve from what that left behind.
    let mut argv: Vec<String> = ["contract", "build"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    argv.extend(request.bldopt.iter().cloned());

    let container = docker.create(&ContainerSpec {
        image: &request.bldimg,
        entrypoint: None,
        argv: &argv,
        workdir: &workdir,
        env: &[("CARGO_HOME", CARGO_HOME), ("CARGO_NET_OFFLINE", "true")],
        volumes: &[(cargo_home.name(), CARGO_HOME)],
        network: Network::None,
        limits: BUILD_LIMITS,
        security: BUILD_SECURITY,
    })?;
    tracing::info!(container = container.id(), image = %request.bldimg, "created build container");

    container.put_archive(STAGE_DIR, &source.tar)?;

    tracing::info!(?argv, "building (network-isolated)");
    let run = container.run_to_completion(request.timeout.saturating_sub(started.elapsed()))?;
    let build_seconds = started.elapsed().as_secs_f64();

    if run.exit_code != 0 {
        tracing::warn!(exit_code = run.exit_code, log = %run.log, "build failed");
        return Err(VerifyError::BuildFailed {
            code: run.exit_code,
            log: run.log,
        });
    }

    let (artifact, wasm) = extract_wasm(&container, &workdir)?;
    if let Some(path) = &request.emit_wasm {
        std::fs::write(path, &wasm)?;
        tracing::info!(path = %path.display(), bytes = wasm.len(), "wrote rebuilt wasm");
    }
    let rebuilt = sha256_hex(&wasm);
    let expected = request.expected_wasm_sha256.to_lowercase();
    let result = if rebuilt == expected {
        VerificationResult::Verified
    } else {
        VerificationResult::Mismatch
    };

    // Record the digest the image actually resolved to. The fetch and build
    // phases already pulled it, so it is present locally now. This is
    // best-effort metadata for the record, not a gate: a failure to resolve it
    // (or a local image that has no registry digest) must not fail a build that
    // otherwise reproduced.
    let bldimg_digest = match docker.image_digest(&request.bldimg) {
        Ok(digest) => digest,
        Err(e) => {
            tracing::warn!(error = %e, "could not resolve bldimg digest");
            None
        }
    };
    tracing::info!(?result, %rebuilt, %expected, digest = ?bldimg_digest, "reproduction complete");

    Ok(ReproductionReport {
        result,
        expected_wasm_sha256: expected,
        rebuilt_wasm_sha256: rebuilt,
        rebuilt_wasm_size: wasm.len(),
        artifact,
        bldimg: request.bldimg.clone(),
        bldimg_digest,
        bldopt: request.bldopt.clone(),
        source_sha256: source.sha256,
        // MVP: every image is untrusted until the allowlist lands. The field is
        // wired end-to-end now so the API's schema does not change later.
        trust_level: TrustLevel::Arbitrary,
        build_seconds,
        build_log: (result == VerificationResult::Mismatch).then_some(run.log),
    })
}

/// Phase 1: populate the shared `CARGO_HOME` volume with the exact dependency
/// versions `Cargo.lock` names.
///
/// Network is on here and only here. `cargo fetch` downloads and unpacks crates
/// without compiling or executing any of them, so nothing untrusted runs while
/// there is a route out.
fn fetch_dependencies(
    docker: &Docker,
    request: &ReproductionRequest,
    source: &SourceArchive,
    workdir: &str,
    cargo_home: &Volume<'_>,
    timeout: Duration,
) -> Result<()> {
    // No --target: it would narrow the fetch to the wasm target and leave out
    // host-side dependencies (proc macros, build scripts), which the offline
    // build phase then could not download. Fetching for all targets is the
    // superset the build is guaranteed to be satisfiable from.
    let argv: Vec<String> = ["fetch", "--locked"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    let container = docker.create(&ContainerSpec {
        image: &request.bldimg,
        // The image entrypoints to `stellar`; this phase needs cargo directly.
        entrypoint: Some("cargo"),
        argv: &argv,
        workdir,
        env: &[("CARGO_HOME", CARGO_HOME)],
        volumes: &[(cargo_home.name(), CARGO_HOME)],
        network: fetch_network(),
        limits: BUILD_LIMITS,
        security: BUILD_SECURITY,
    })?;
    container.put_archive(STAGE_DIR, &source.tar)?;

    tracing::info!(network = ?fetch_network(), "fetching dependencies (Cargo.lock, network on)");
    let run = container.run_to_completion(timeout)?;
    if run.exit_code != 0 {
        // Almost always a missing/stale Cargo.lock: --locked refuses to invent
        // versions, and a source tree without one cannot be reproduced anyway.
        tracing::warn!(exit_code = run.exit_code, log = %run.log, "dependency fetch failed");
        return Err(VerifyError::BuildFailed {
            code: run.exit_code,
            log: run.log,
        });
    }
    Ok(())
}

/// Whether a `docker cp` error is "the source path does not exist" rather than
/// an infrastructure failure. `docker cp` phrases a missing path differently
/// across versions, so match the known shapes; anything else stays a real
/// [`VerifyError::Docker`] attributed to our side, not the submitter's.
fn is_missing_path(docker_error: &str) -> bool {
    let msg = docker_error.to_ascii_lowercase();
    msg.contains("no such file or directory")
        || msg.contains("could not find the file")
        || msg.contains("no such container:path")
}

/// Pull the built `.wasm` out of the container.
///
/// Copies the release directory out as a tar, then hands the bytes to
/// [`select_wasm_from_tar`] to pick the contract artifact. The container I/O is
/// here; the selection *rules* live in that pure function so they can be tested
/// without a running daemon.
fn extract_wasm(
    container: &crate::docker::Container<'_>,
    workdir: &str,
) -> Result<(String, Vec<u8>)> {
    let release_dir = format!("{workdir}/target/{WASM_TARGET}/release");
    let tar = match container.get_archive(&release_dir) {
        Ok(tar) => tar,
        // A missing release dir means the build produced no wasm32 artifact
        // (e.g. a bldopt pointed the build at the wrong crate) — the submitter's
        // problem. Any *other* docker failure (daemon down, killed container) is
        // ours, and must not be misreported as "your source built nothing".
        Err(VerifyError::Docker(msg)) if is_missing_path(&msg) => {
            return Err(VerifyError::NoWasmProduced)
        }
        Err(e) => return Err(e),
    };
    select_wasm_from_tar(&tar)
}

/// Pick the contract's `.wasm` from the release directory's tar bytes.
///
/// Split out from [`extract_wasm`] so the selection rules can be unit-tested
/// without a container. Cargo writes finished artifacts at the root of the
/// profile directory and uses subdirectories (`deps/`, `build/`, `incremental/`)
/// for intermediates — the copy under `deps/` is the same contract, not a second
/// candidate — so only root-level `.wasm` files count. `docker cp <dir>` roots
/// the tar at the directory itself, so a finished artifact is exactly
/// `release/<name>.wasm` (depth 2); anything deeper is an intermediate.
///
/// More than one artifact at the root means a workspace with several contracts,
/// where "the" WASM is genuinely ambiguous — except the `--optimize` case, which
/// emits `<name>.optimized.wasm` next to the original; that pair resolves to the
/// optimized one, since that is what gets deployed and whose hash is on chain.
fn select_wasm_from_tar(tar: &[u8]) -> Result<(String, Vec<u8>)> {
    let mut archive = tar::Archive::new(tar);
    let mut found: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in archive
        .entries()
        .map_err(|e| VerifyError::Docker(format!("build output is not a valid tar: {e}")))?
    {
        let mut entry =
            entry.map_err(|e| VerifyError::Docker(format!("bad build-output entry: {e}")))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|e| VerifyError::Docker(format!("bad build-output path: {e}")))?
            .to_string_lossy()
            .into_owned();
        if !path.ends_with(".wasm") {
            continue;
        }
        // Root-level only: `release/<name>.wasm` is depth 2; deeper is a cargo
        // intermediate (`deps/`, `build/`, `incremental/`).
        if path.split('/').filter(|s| !s.is_empty()).count() != 2 {
            continue;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        found.push((path, bytes));
    }

    match found.len() {
        0 => Err(VerifyError::NoWasmProduced),
        1 => Ok(found.pop().expect("checked len == 1")),
        _ => {
            // `--optimize` emits `<name>.optimized.wasm` beside the original. The
            // optimized one is what gets deployed, so it is the on-chain artifact.
            let optimized: Vec<usize> = found
                .iter()
                .enumerate()
                .filter(|(_, (p, _))| p.ends_with(".optimized.wasm"))
                .map(|(i, _)| i)
                .collect();
            if optimized.len() == 1 && found.len() == 2 {
                return Ok(found.swap_remove(optimized[0]));
            }
            let names = found.iter().map(|(p, _)| p.clone()).collect();
            Err(VerifyError::AmbiguousWasm {
                count: found.len(),
                names,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an in-memory tar from `(path, bytes)` entries — the shape
    /// `docker cp <release_dir> -` produces, rooted at `release/`.
    fn tar_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for &(path, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, path, data)
                .expect("append tar entry");
        }
        builder.into_inner().expect("finish tar")
    }

    #[test]
    fn fetch_network_defaults_to_bridge_and_honours_a_configured_name() {
        // Unset (or blank/whitespace) keeps today's behaviour: the default bridge.
        assert_eq!(network_for(None), Network::Bridge);
        assert_eq!(network_for(Some("")), Network::Bridge);
        assert_eq!(network_for(Some("   ")), Network::Bridge);
        // A configured name pins the fetch phase to that network.
        assert_eq!(
            network_for(Some("sorofy-fetch")),
            Network::Named("sorofy-fetch")
        );
    }

    #[test]
    fn single_root_wasm_is_selected() {
        let tar = tar_with(&[("release/hello.wasm", b"WASM")]);
        let (name, bytes) = select_wasm_from_tar(&tar).expect("one artifact");
        assert_eq!(name, "release/hello.wasm");
        assert_eq!(bytes, b"WASM");
    }

    #[test]
    fn no_wasm_is_no_wasm_produced() {
        // Non-wasm outputs only → the build produced no contract artifact.
        let tar = tar_with(&[("release/hello.d", b"x"), ("release/libhello.rlib", b"y")]);
        assert!(matches!(
            select_wasm_from_tar(&tar),
            Err(VerifyError::NoWasmProduced)
        ));
    }

    #[test]
    fn intermediate_wasm_under_deps_is_ignored() {
        // The `deps/` copy is a cargo intermediate, not a second candidate.
        let tar = tar_with(&[
            ("release/hello.wasm", b"ROOT"),
            ("release/deps/hello.wasm", b"DEP"),
        ]);
        let (name, bytes) = select_wasm_from_tar(&tar).expect("root artifact wins");
        assert_eq!(name, "release/hello.wasm");
        assert_eq!(bytes, b"ROOT");
    }

    #[test]
    fn optimized_artifact_wins_over_its_unoptimized_sibling() {
        let tar = tar_with(&[
            ("release/hello.wasm", b"PLAIN"),
            ("release/hello.optimized.wasm", b"OPT"),
        ]);
        let (name, bytes) = select_wasm_from_tar(&tar).expect("optimized wins");
        assert_eq!(name, "release/hello.optimized.wasm");
        assert_eq!(bytes, b"OPT");
    }

    #[test]
    fn two_unrelated_contracts_are_ambiguous_not_auto_picked() {
        // Two roots, neither optimized → the optimize shortcut must NOT fire.
        let tar = tar_with(&[("release/one.wasm", b"1"), ("release/two.wasm", b"2")]);
        assert!(matches!(
            select_wasm_from_tar(&tar),
            Err(VerifyError::AmbiguousWasm { count: 2, .. })
        ));
    }

    #[test]
    fn three_contracts_are_ambiguous_with_all_names() {
        let tar = tar_with(&[
            ("release/one.wasm", b"1"),
            ("release/two.wasm", b"2"),
            ("release/three.wasm", b"3"),
        ]);
        match select_wasm_from_tar(&tar) {
            Err(VerifyError::AmbiguousWasm { count, names }) => {
                assert_eq!(count, 3);
                assert_eq!(names.len(), 3);
            }
            other => panic!("expected AmbiguousWasm, got {other:?}"),
        }
    }
}
