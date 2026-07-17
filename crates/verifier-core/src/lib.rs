//! Core verification logic for the Soroban Contract Verification Service.
//!
//! Implements SEP-58's reproduction algorithm: take build metadata + a source
//! reference, rebuild the WASM inside a digest-pinned, network-isolated
//! container, and compare its sha256 against the on-chain hash. See
//! `docs/sep-58-notes.md`.

pub mod docker;
pub mod error;
pub mod reproduce;
pub mod source;

pub use docker::Docker;
pub use error::{Result, VerifyError};
pub use reproduce::{reproduce, ReproductionReport, ReproductionRequest, DEFAULT_TIMEOUT};
pub use source::SourceRef;

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

impl BuildMetadata {
    /// The source reference these fields describe, if they carry one.
    ///
    /// `source_uri` is optional in SEP-58: a contract may declare only a
    /// `source_sha256`, leaving the archive to be found by content address.
    /// Until we have such a lookup, those contracts cannot be fetched.
    pub fn source_ref(&self) -> Option<SourceRef> {
        self.source_uri.as_ref().map(|uri| SourceRef::Archive {
            uri: uri.clone(),
            source_sha256: self.source_sha256.clone(),
        })
    }
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
    hex::encode(Sha256::digest(bytes))
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

    #[test]
    fn unpinned_bldimg_is_rejected_before_any_build() {
        // SEP-58 mandates a digest: a tag can be repointed at different bytes,
        // which would silently invalidate earlier verifications.
        let request = ReproductionRequest::new(
            SourceRef::Git { repo: "https://example.com/x.git".into(), rev: "HEAD".into() },
            "docker.io/stellar/stellar-cli:23.2.1",
            "b68602842d3a1d169d54fe3e57c0511a774df4710553d6d4d22e653d62bf5f5b",
        );
        let err = reproduce(&Docker::local(), &request).unwrap_err();
        assert!(matches!(err, VerifyError::UnpinnedImage(_)), "got {err:?}");
    }

    #[test]
    fn source_layout_requires_exactly_one_top_level_dir() {
        // SEP-58 step 4. Two top-level dirs means we cannot tell which is the
        // crate root to build from.
        let mut builder = tar::Builder::new(Vec::new());
        for name in ["a/Cargo.toml", "b/Cargo.toml"] {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, &b""[..]).unwrap();
        }
        let tar = builder.into_inner().unwrap();
        assert!(matches!(source::single_top_dir(&tar), Err(VerifyError::SourceLayout(2))));
    }

    #[test]
    fn traversal_paths_in_a_source_archive_are_rejected() {
        // `docker cp` unpacks this tar; `..` would escape the staging dir. The
        // name has to be written into the header by hand because tar::Builder
        // refuses to emit `..` — a hostile archive would not be so polite.
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        let name = b"../evil.sh";
        header.as_old_mut().name[..name.len()].copy_from_slice(name);
        header.set_cksum();

        let mut builder = tar::Builder::new(Vec::new());
        builder.append(&header, &b""[..]).unwrap();
        let tar = builder.into_inner().unwrap();

        assert!(matches!(source::single_top_dir(&tar), Err(VerifyError::SourceFetch(_))));
    }
}
