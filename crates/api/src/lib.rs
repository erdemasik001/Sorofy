//! Sorofy API internals: on-chain lookup, result cache, the REST layer, and the
//! attestation path that files results with the verifier registry.
//!
//! A library so integration tests can drive the pieces directly; the deployable
//! server lives in `src/main.rs`.

pub mod attest;
pub mod claim;
pub mod db;
pub mod rpc;
pub mod server;
