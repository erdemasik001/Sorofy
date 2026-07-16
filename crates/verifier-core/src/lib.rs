//! Core verification logic for the Soroban Contract Verification Service.
//!
//! Day0 scaffold. The real reproduction pipeline (SEP-58 metadata -> pinned
//! Docker build -> sha256 compare) lands in Day1. See `docs/sep-58-notes.md`.

use serde::{Deserialize, Serialize};

/// SEP-58 build-reproducibility metadata needed to rebuild a contract's WASM.
///
/// Field names follow the SEP-58 spec (not the older brief names). See
/// `docs/sep-58-notes.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildMetadata {
    /// Fully-qualified container image, pinned by digest (single-arch). Required.
    pub bldimg: String,
    /// Shell-style flags passed verbatim to the build command. Repeatable.
    #[serde(default)]
    pub bldopt: Vec<String>,
    /// URI to download the source archive (https:// expected). Optional.
    #[serde(default)]
    pub source_uri: Option<String>,
    /// SHA-256 of the source archive bytes. Required.
    pub source_sha256: String,
}

/// Trust level of the build image used for a verification.
///
/// Backs the multi-dimensional trust model (brief §5). MVP returns a fixed
/// value; real allowlist logic comes later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrustLevel {
    /// Arbitrary digest, not on any allowlist.
    Arbitrary,
    /// Image is publicly auditable (SBOM / reproducible build / attestations).
    PubliclyAuditable,
    /// Image is maintained by SDF.
    SdfMaintained,
}

/// Outcome of a verification job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationResult {
    Verified,
    Mismatch,
    Pending,
    Error,
}

/// Compute the lowercase hex sha256 of a byte slice (used for WASM comparison).
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    hex_encode(&digest)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_of_empty_is_known_vector() {
        // Well-known sha256("") value; also the SEP-58 example source_sha256.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
