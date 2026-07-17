//! `sorofy-api` — the public verification service.
//!
//! Configuration via env (defaults suit local dev):
//! - `SOROFY_BIND`  — listen address, default `127.0.0.1:8080`
//! - `SOROFY_DB`    — sqlite path, default `sorofy.db`
//! - `SOROFY_RPC`   — Soroban RPC endpoint, default public testnet
//! - `SOROFY_ALLOW_UNPINNED_IMAGE=1` — accept non-digest `bldimg` (local dev)

use api::db::Db;
use api::server::{router, AppState};
use verifier_core::Docker;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let bind = std::env::var("SOROFY_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let db_path = std::env::var("SOROFY_DB").unwrap_or_else(|_| "sorofy.db".into());
    let rpc_url = std::env::var("SOROFY_RPC").unwrap_or_else(|_| api::rpc::TESTNET_RPC.into());
    let allow_unpinned = std::env::var("SOROFY_ALLOW_UNPINNED_IMAGE").is_ok_and(|v| v == "1");

    let docker = Docker::autodetect();
    docker.preflight().map_err(|e| anyhow::anyhow!("docker unavailable: {e}"))?;

    let db = Db::open(std::path::Path::new(&db_path))?;
    let state = AppState::new(db, docker, rpc_url.clone(), allow_unpinned);

    tracing::info!(%bind, db = %db_path, rpc = %rpc_url, allow_unpinned, "sorofy-api listening");
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}
