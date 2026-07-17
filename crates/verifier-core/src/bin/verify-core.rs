//! `verify-core` — rebuild a contract from source and compare it to a WASM hash.
//!
//! The Day1 deliverable: the verification engine, driven from the command line
//! with no API in front of it.
//!
//! ```text
//! verify-core --repo https://github.com/user/contract --rev <sha> \
//!             --bldimg <image@sha256:...> --wasm-hash <sha256>
//! ```

use std::process::ExitCode;
use std::time::Duration;

use clap::{ArgGroup, Parser};
use verifier_core::{
    reproduce, Docker, ReproductionRequest, SourceRef, VerificationResult, VerifyError,
};

#[derive(Parser, Debug)]
#[command(
    name = "verify-core",
    about = "Rebuild a Soroban contract from source in a pinned container and compare its WASM hash"
)]
// Exactly one source: a git repo+rev, or a SEP-58 source_uri+source_sha256.
#[command(group(ArgGroup::new("src").required(true).args(["repo", "source_uri"])))]
struct Args {
    /// Git repository holding the contract source.
    #[arg(long, group = "src")]
    repo: Option<String>,

    /// Commit to build (used with --repo).
    #[arg(long, requires = "repo")]
    rev: Option<String>,

    /// Source archive URI (SEP-58 `source_uri`), as an alternative to --repo.
    #[arg(long, group = "src", requires = "source_sha256")]
    source_uri: Option<String>,

    /// SEP-58 `source_sha256`: expected sha256 of the archive at --source-uri.
    #[arg(long)]
    source_sha256: Option<String>,

    /// SEP-58 `bldimg`: build image, digest-pinned.
    #[arg(long)]
    bldimg: String,

    /// SEP-58 `bldopt`: flag passed verbatim to `stellar contract build`. Repeatable.
    #[arg(long = "bldopt")]
    bldopt: Vec<String>,

    /// The on-chain WASM hash to reproduce.
    #[arg(long = "wasm-hash")]
    wasm_hash: String,

    /// Build timeout in seconds.
    #[arg(long, default_value = "900")]
    timeout: u64,

    /// Accept a --bldimg without a digest. For locally-built images only;
    /// SEP-58 requires a digest because tags can move.
    #[arg(long)]
    allow_unpinned_image: bool,

    /// Print the full report as JSON.
    #[arg(long)]
    json: bool,
}

/// `verified` -> 0, `mismatch` -> 1, error -> 2, so CI can branch on it.
fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();

    let args = Args::parse();

    let source = match (&args.repo, &args.source_uri) {
        (Some(repo), None) => SourceRef::Git {
            repo: repo.clone(),
            rev: args.rev.clone().unwrap_or_else(|| "HEAD".into()),
        },
        (None, Some(uri)) => SourceRef::Archive {
            uri: uri.clone(),
            source_sha256: args.source_sha256.clone().expect("clap requires it with --source-uri"),
        },
        _ => {
            eprintln!("error: pass exactly one of --repo or --source-uri");
            return ExitCode::from(2);
        }
    };

    let request = ReproductionRequest {
        source,
        bldimg: args.bldimg,
        bldopt: args.bldopt,
        expected_wasm_sha256: args.wasm_hash,
        timeout: Duration::from_secs(args.timeout),
        allow_unpinned_image: args.allow_unpinned_image,
    };

    match reproduce(&Docker::autodetect(), &request) {
        Ok(report) => {
            if args.json {
                println!("{}", serde_json::to_string_pretty(&report).expect("report is serializable"));
            } else {
                print_report(&report);
            }
            match report.result {
                VerificationResult::Verified => ExitCode::SUCCESS,
                _ => ExitCode::from(1),
            }
        }
        Err(err) => {
            report_error(&err);
            ExitCode::from(2)
        }
    }
}

fn print_report(report: &verifier_core::ReproductionReport) {
    let verdict = match report.result {
        VerificationResult::Verified => "VERIFIED",
        VerificationResult::Mismatch => "MISMATCH",
        other => {
            println!("{other:?}");
            return;
        }
    };
    println!();
    println!("  result:    {verdict}");
    println!("  expected:  {}", report.expected_wasm_sha256);
    println!("  rebuilt:   {}", report.rebuilt_wasm_sha256);
    println!("  artifact:  {} ({} bytes)", report.artifact, report.rebuilt_wasm_size);
    println!("  bldimg:    {}", report.bldimg);
    if let Some(digest) = &report.bldimg_digest {
        println!("  digest:    {digest}");
    }
    if !report.bldopt.is_empty() {
        println!("  bldopt:    {}", report.bldopt.join(" "));
    }
    println!("  source:    sha256:{}", report.source_sha256);
    println!("  trust:     {:?}", report.trust_level);
    println!("  build:     {:.1}s", report.build_seconds);
    println!();
}

/// Print the failure plus what the operator can actually do about it.
fn report_error(err: &VerifyError) {
    eprintln!("\nerror: {err}");
    match err {
        VerifyError::BuildFailed { log, .. } => {
            eprintln!("\n--- build log ---\n{}", log.trim_end());
        }
        VerifyError::Docker(_) => {
            eprintln!(
                "\nhint: is the daemon running? On this Windows dev box Docker Engine lives \
                 inside WSL2 (`wsl -d Ubuntu -- sudo service docker start`).\n\
                 Override the docker command with VERIFY_DOCKER, e.g. \
                 VERIFY_DOCKER=\"wsl -d Ubuntu -- docker\"."
            );
        }
        VerifyError::UnpinnedImage(_) => {
            eprintln!(
                "\nhint: resolve the tag to a digest with `docker image inspect <img> \
                 --format '{{{{index .RepoDigests 0}}}}'`, or pass --allow-unpinned-image \
                 for a local image."
            );
        }
        _ => {}
    }
}
