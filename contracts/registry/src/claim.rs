//! The claim digest: what makes two attestations comparable.
//!
//! A verifier's result depends on the on-chain hash *and* on the source, the build image and
//! the build flags. Two verifiers are only asking the same question if all of these match, so
//! every attestation is filed under `(wasm_hash, input_digest)` where
//!
//! ```text
//! input_digest = sha256( "sorofy-claim-v1"
//!                        ‖ source_sha256        32 bytes
//!                        ‖ bldimg_digest        32 bytes  (the hex after `@sha256:`, decoded)
//!                        ‖ u32_be(len(bldopt))
//!                        ‖ for each bldopt, in submission order: u32_be(len) ‖ bytes )
//! ```
//!
//! The contract never computes this: it only stores and compares the 32 bytes the verifier
//! supplies. The function exists so the layout is written down once, in code, and pinned by a
//! known-answer test whose expected values were produced independently (Python). The API
//! implements the same layout and must reproduce the same vectors.

use soroban_sdk::{Bytes, BytesN, Env, Vec};

const DOMAIN: &[u8] = b"sorofy-claim-v1";

pub fn claim_input_digest(
    env: &Env,
    source_sha256: &BytesN<32>,
    bldimg_digest: &BytesN<32>,
    bldopt: &Vec<Bytes>,
) -> BytesN<32> {
    let mut buf = Bytes::from_slice(env, DOMAIN);
    buf.append(&source_sha256.clone().into());
    buf.append(&bldimg_digest.clone().into());
    buf.extend_from_array(&bldopt.len().to_be_bytes());
    for opt in bldopt.iter() {
        buf.extend_from_array(&opt.len().to_be_bytes());
        buf.append(&opt);
    }
    env.crypto().sha256(&buf).to_bytes()
}
