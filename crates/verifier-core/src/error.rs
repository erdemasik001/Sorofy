//! Failure modes of a reproduction job.
//!
//! These map onto what the Day2 API reports back to a caller, so the split is
//! by *who is at fault*: the submitted source/metadata, the build itself, or
//! our own infrastructure.

use std::time::Duration;

pub type Result<T> = std::result::Result<T, VerifyError>;

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// Could not obtain the source archive (clone failed, download failed, 404).
    #[error("source fetch failed: {0}")]
    SourceFetch(String),

    /// Downloaded bytes did not match the declared `source_sha256` (SEP-58 step 3).
    #[error("source integrity check failed: declared source_sha256 {expected}, got {actual}")]
    SourceIntegrity { expected: String, actual: String },

    /// SEP-58 requires the archive to hold exactly one top-level directory.
    #[error("source archive must contain exactly one top-level directory, found {0}")]
    SourceLayout(usize),

    /// SEP-58 requires `bldimg` to be pinned by digest; a tag can be moved.
    #[error("build image must be digest-pinned (`image@sha256:...`), got `{0}`")]
    UnpinnedImage(String),

    /// The build command exited non-zero.
    #[error("build failed (exit code {code})")]
    BuildFailed { code: i32, log: String },

    /// The build exceeded the job's wall-clock budget.
    #[error("build timed out after {0:?}")]
    Timeout(Duration),

    /// Build succeeded but emitted no `.wasm`.
    #[error("build produced no .wasm artifact")]
    NoWasmProduced,

    /// Build emitted several `.wasm` files, so the target is ambiguous; the
    /// submitter must narrow it with a `bldopt` such as `--package=<name>`.
    #[error("build produced {count} .wasm artifacts ({names:?}); narrow it with a bldopt like --package=<name>")]
    AmbiguousWasm { count: usize, names: Vec<String> },

    /// Docker itself misbehaved (not installed, daemon down, bad args).
    #[error("docker: {0}")]
    Docker(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
