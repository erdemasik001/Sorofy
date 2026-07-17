//! Sorofy API internals: on-chain lookup, result cache, and the REST layer.
//!
//! A library so integration tests can drive the pieces directly; the deployable
//! server lives in `src/main.rs`.

pub mod db;
pub mod rpc;
pub mod server;
