//! The claim a verifier attests to, computed API-side.
//!
//! Every attestation is filed on-chain under `(wasm_hash, input_digest)`. The registry never
//! computes `input_digest`: it stores and compares the 32 bytes the verifier hands it. So the
//! layout only holds two verifiers to the same question if *both* sides implement it
//! identically — which makes this module a mirror of `contracts/registry/src/claim.rs`, and
//! makes the known-answer vectors below the thing that actually ties them together.
//!
//! ```text
//! input_digest = sha256( "sorofy-claim-v1"
//!                        ‖ source_sha256        32 bytes
//!                        ‖ bldimg_digest        32 bytes  (the hex after `@sha256:`, decoded)
//!                        ‖ u32_be(len(bldopt))
//!                        ‖ for each bldopt, in submission order: u32_be(len) ‖ bytes )
//! ```

use anyhow::{anyhow, Context};
use sha2::{Digest, Sha256};
use verifier_core::ReproductionReport;

const DOMAIN: &[u8] = b"sorofy-claim-v1";

/// What one attestation says, in the four values `attest` takes.
///
/// Built only from a finished report, so the tuple is internally consistent by construction:
/// the registry derives the verdict as `rebuilt_hash == wasm_hash` and a verifier cannot
/// attest a pair that disagrees with what it rebuilt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// The hash the job set out to reproduce — for a `contract_id` job, the on-chain one.
    pub wasm_hash: [u8; 32],
    /// Binds the attestation to the *question*: source, image and flags.
    pub input_digest: [u8; 32],
    /// What this verifier actually built.
    pub rebuilt_hash: [u8; 32],
}

/// Why a finished report yields no claim.
///
/// Distinct from a submission failure: there is nothing here to retry, because the claim
/// cannot be formed at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoClaim {
    /// The daemon resolved no registry digest for the build image — a locally built image that
    /// was never pushed. Without it the claim has no image to bind to, and inventing one (a
    /// zero digest, the tag's text) would file the attestation under a claim no other verifier
    /// could ever land on. `--allow-unpinned-image` is a local-dev path, so this is expected
    /// there and never in a real deployment.
    NoImageDigest,
    /// A field of the report is not the 32-byte hex it is declared to be. Only reachable if
    /// the engine's own output is malformed.
    Malformed(String),
}

impl std::fmt::Display for NoClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoClaim::NoImageDigest => f.write_str(
                "the build image has no registry digest (locally built, never pushed), so the \
                 claim has no image to bind to",
            ),
            NoClaim::Malformed(why) => write!(f, "report field is not a 32-byte hex digest: {why}"),
        }
    }
}

impl Claim {
    /// The claim a finished reproduction attests to, or why there is none.
    pub fn from_report(report: &ReproductionReport) -> Result<Claim, NoClaim> {
        let field = |name: &str, hex: &str| {
            hex32(hex).map_err(|e| NoClaim::Malformed(format!("{name}: {e:#}")))
        };

        // `bldimg_digest` is the `repo@sha256:…` the daemon resolved. `None` means there was
        // nothing to resolve; that is a skip, not a failure (see `NoClaim::NoImageDigest`).
        let bldimg = report
            .bldimg_digest
            .as_deref()
            .ok_or(NoClaim::NoImageDigest)?;
        let bldimg_digest = field("bldimg_digest", image_digest_hex(bldimg)?)?;

        Ok(Claim {
            wasm_hash: field("expected_wasm_sha256", &report.expected_wasm_sha256)?,
            input_digest: input_digest(
                &field("source_sha256", &report.source_sha256)?,
                &bldimg_digest,
                &report.bldopt,
            ),
            rebuilt_hash: field("rebuilt_wasm_sha256", &report.rebuilt_wasm_sha256)?,
        })
    }
}

/// `sha256(domain ‖ source ‖ image ‖ length-prefixed bldopt)` — the layout above.
pub fn input_digest(
    source_sha256: &[u8; 32],
    bldimg_digest: &[u8; 32],
    bldopt: &[String],
) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(DOMAIN);
    h.update(source_sha256);
    h.update(bldimg_digest);
    // `u32` big-endian counts and lengths, exactly as the contract writes them. A count that
    // did not fit a u32 could not have come from a request the API accepted.
    h.update((bldopt.len() as u32).to_be_bytes());
    for opt in bldopt {
        h.update((opt.len() as u32).to_be_bytes());
        h.update(opt.as_bytes());
    }
    h.finalize().into()
}

/// The hex after `@sha256:` in a `repo@sha256:…` reference.
///
/// Rejects anything else rather than guessing: a reference without a digest is the
/// unpinned-image case, which [`NoClaim::NoImageDigest`] already covers upstream, and a
/// different algorithm is not something this layout can carry.
fn image_digest_hex(bldimg_digest: &str) -> Result<&str, NoClaim> {
    bldimg_digest
        .split_once("@sha256:")
        .map(|(_, hex)| hex)
        .ok_or_else(|| {
            NoClaim::Malformed(format!(
                "bldimg_digest is not a `repo@sha256:…` reference: {bldimg_digest}"
            ))
        })
}

/// Decode exactly 32 bytes of hex.
pub fn hex32(s: &str) -> anyhow::Result<[u8; 32]> {
    let bytes = hex::decode(s).with_context(|| format!("`{s}` is not hex"))?;
    bytes
        .try_into()
        .map_err(|v: Vec<u8>| anyhow!("expected 32 bytes, got {}", v.len()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use verifier_core::{TrustLevel, VerificationResult};

    /// The report of a real, reproducible build — the one case where an attestation should go
    /// out. Shared with `attest`'s tests, so "nothing was queued" there means the flag, and
    /// not a report that could never have formed a claim.
    pub(crate) fn attestable_report() -> ReproductionReport {
        report(Some(
            "ghcr.io/erdemasik001/sorofy-build-image@sha256:\
             cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588",
        ))
    }

    fn opts(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// The four vectors `contracts/registry/src/test.rs` pins the contract against, expected
    /// values and all. Their provenance is a Python implementation of the layout written
    /// independently of either side — so this asserts the two implementations agree with a
    /// third party, not merely with each other.
    #[test]
    fn the_claim_digest_matches_the_contracts_reference_vectors() {
        let (src, img) = ([0x11u8; 32], [0x22u8; 32]);

        assert_eq!(
            hex::encode(input_digest(
                &src,
                &img,
                &opts(&["--package=vrfy-token", "--optimize"])
            )),
            "ded0494ae3e75310e4989acb8daacfa79636c7e3c211b1af884fa2376ed38381"
        );
        assert_eq!(
            hex::encode(input_digest(&src, &img, &[])),
            "14ee749e3747cf362fffeb845000c5395c9d05055bb8f7e2fb48d41ce7e5064e"
        );
        // Flag order is part of the question.
        assert_eq!(
            hex::encode(input_digest(
                &src,
                &img,
                &opts(&["--optimize", "--package=vrfy-token"])
            )),
            "c0823d00b2b99e2f425b9f3fa283f652e315cca163ef3ad10e7fa1f408d881fb"
        );
    }

    /// The contract's other vector: the staged-tree and image digests Sorofy's engine reported
    /// when it rebuilt the deployed VRFY token.
    #[test]
    fn the_claim_digest_of_the_real_vrfy_token_build() {
        assert_eq!(
            hex::encode(input_digest(
                &hex32("47a88a77289c02370d0f04995e6faadcea4f5fb081981ff5ee48762804c0f306").unwrap(),
                &hex32("cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588").unwrap(),
                &opts(&["--package=vrfy-token"]),
            )),
            "32f0a6e512a0259f9d4e2f5a5781cfda2af1782cf4246d9a0fa9a1f314d262fb"
        );
    }

    /// The length prefixes are what stop two different flag lists from colliding: without
    /// them `["ab", "c"]` and `["a", "bc"]` would hash the same bytes.
    #[test]
    fn concatenation_alone_would_not_separate_these_flag_lists() {
        let (src, img) = ([0x11u8; 32], [0x22u8; 32]);
        assert_ne!(
            input_digest(&src, &img, &opts(&["ab", "c"])),
            input_digest(&src, &img, &opts(&["a", "bc"])),
        );
    }

    fn report(bldimg_digest: Option<&str>) -> ReproductionReport {
        ReproductionReport {
            result: VerificationResult::Verified,
            expected_wasm_sha256:
                "3e23ccf50f1a28a568229e21eddc2f11ea6ee495ebe2c3736e545d054be59028".into(),
            rebuilt_wasm_sha256: "3e23ccf50f1a28a568229e21eddc2f11ea6ee495ebe2c3736e545d054be59028"
                .into(),
            rebuilt_wasm_size: 8576,
            artifact: "target/wasm32v1-none/release/vrfy_token.wasm".into(),
            bldimg: "ghcr.io/erdemasik001/sorofy-build-image:pinned".into(),
            bldimg_digest: bldimg_digest.map(str::to_string),
            bldopt: opts(&["--package=vrfy-token"]),
            source_sha256: "47a88a77289c02370d0f04995e6faadcea4f5fb081981ff5ee48762804c0f306"
                .into(),
            trust_level: TrustLevel::Arbitrary,
            build_seconds: 65.6,
            build_log: None,
        }
    }

    /// End to end from the real VRFY report: the same `input_digest` as the contract's vector,
    /// with the image digest taken out of the full `repo@sha256:…` string.
    #[test]
    fn a_report_yields_the_claim_the_contract_pins() {
        let claim =
            Claim::from_report(&attestable_report()).expect("a well-formed report yields a claim");

        assert_eq!(
            hex::encode(claim.input_digest),
            "32f0a6e512a0259f9d4e2f5a5781cfda2af1782cf4246d9a0fa9a1f314d262fb"
        );
        // A verified rebuild attests the hash it was asked for.
        assert_eq!(claim.rebuilt_hash, claim.wasm_hash);
    }

    #[test]
    fn a_report_without_an_image_digest_yields_no_claim_rather_than_an_error() {
        assert_eq!(
            Claim::from_report(&report(None)),
            Err(NoClaim::NoImageDigest)
        );
    }

    #[test]
    fn a_reference_without_a_sha256_digest_is_refused_rather_than_guessed() {
        let err = Claim::from_report(&report(Some("ghcr.io/x/y:latest")))
            .expect_err("a tag is not a digest");
        assert!(
            matches!(err, NoClaim::Malformed(ref m) if m.contains("repo@sha256:")),
            "unexpected: {err}"
        );
    }

    #[test]
    fn a_mismatching_rebuild_still_forms_a_claim() {
        // Honest mismatches are attested too: they are comparable between verifiers, because
        // they must agree on the *rebuilt* hash. Only job errors are never attested.
        let mut r = attestable_report();
        r.result = VerificationResult::Mismatch;
        r.rebuilt_wasm_sha256 = "00".repeat(32);

        let claim = Claim::from_report(&r).expect("a mismatch is still a claim");
        assert_ne!(claim.rebuilt_hash, claim.wasm_hash);
    }

    #[test]
    fn hex32_rejects_the_wrong_length_and_non_hex() {
        assert!(hex32(&"aa".repeat(32)).is_ok());
        assert!(hex32(&"aa".repeat(31)).is_err());
        assert!(hex32(&"aa".repeat(33)).is_err());
        assert!(hex32("zz").is_err());
    }
}
