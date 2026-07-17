//! Public REST API for Sorofy, the Soroban contract verification service.
//!
//! Day0 scaffold: the Axum server, `POST /verify`, `GET /verify/{id}`,
//! on-chain lookup, and cache land in Day2. For now this just confirms the
//! workspace wires up against `verifier-core`.

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Sanity check that verifier-core is linked.
    let empty = verifier_core::sha256_hex(b"");
    tracing::info!(sha256_empty = %empty, "sorofy-api scaffold — Day2 adds the HTTP server");
}
