//! `sorofy-api` — the public verification service.
//!
//! Configuration via env (defaults suit local dev):
//! - `SOROFY_BIND`  — listen address, default `127.0.0.1:8080`
//! - `SOROFY_DB`    — sqlite path, default `sorofy.db`
//! - `SOROFY_RPC`   — Soroban RPC endpoint, default public testnet
//! - `SOROFY_ALLOW_UNPINNED_IMAGE=1` — accept non-digest `bldimg` (local dev)
//! - `SOROFY_API_TOKEN` — bearer token required on `POST /verify`; unset ⇒ open
//!   (local dev only — set it before exposing the service, see docs/security.md)
//! - `SOROFY_BACKUP_DIR` — if set, periodically snapshot the cache into this
//!   directory (unset ⇒ no backups)
//! - `SOROFY_BACKUP_INTERVAL_HOURS` — how often to snapshot, default 24

use api::db::Db;
use api::server::{router, AppState};
use verifier_core::Docker;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let bind = std::env::var("SOROFY_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let db_path = std::env::var("SOROFY_DB").unwrap_or_else(|_| "sorofy.db".into());
    let rpc_url = std::env::var("SOROFY_RPC").unwrap_or_else(|_| api::rpc::TESTNET_RPC.into());
    let allow_unpinned = std::env::var("SOROFY_ALLOW_UNPINNED_IMAGE").is_ok_and(|v| v == "1");
    let api_token = std::env::var("SOROFY_API_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    if api_token.is_none() {
        tracing::warn!(
            "SOROFY_API_TOKEN is not set — POST /verify is UNAUTHENTICATED. Fine for local \
             dev; set it before exposing the service (docs/security.md, G2)."
        );
    }

    let docker = Docker::autodetect();
    docker
        .preflight()
        .map_err(|e| anyhow::anyhow!("docker unavailable: {e}"))?;

    let db = Db::open(std::path::Path::new(&db_path))?;
    tracing::info!(schema_version = db.schema_version()?, "cache schema ready");
    if let Ok(dir) = std::env::var("SOROFY_BACKUP_DIR") {
        spawn_periodic_backup(db.clone(), std::path::PathBuf::from(dir));
    }
    let state = AppState::new(db, docker, rpc_url.clone(), allow_unpinned, api_token);

    let auth_enabled = state.api_token.is_some();
    tracing::info!(%bind, db = %db_path, rpc = %rpc_url, allow_unpinned, auth = auth_enabled, "sorofy-api listening");
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}

/// Snapshot the cache into `dir` on an interval (roadmap 0.5).
///
/// `Db::backup_to` is `VACUUM INTO`, so a snapshot is a consistent, restorable
/// database taken while the service keeps serving. Each file is timestamped, so
/// snapshots accumulate rather than overwrite — retention is the operator's
/// (a cron `find -mtime +N -delete` on the volume), not something the service
/// should silently decide. A failure is logged and the loop continues: losing a
/// backup must never take the service down.
fn spawn_periodic_backup(db: Db, dir: std::path::PathBuf) {
    let hours: u64 = std::env::var("SOROFY_BACKUP_INTERVAL_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|h| *h > 0)
        .unwrap_or(24);
    tracing::info!(dir = %dir.display(), interval_hours = hours, "periodic cache backup enabled");

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(hours * 3600));
        // The first tick fires immediately; skip it so startup is not slowed by a
        // snapshot, and the first backup lands one interval in.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = std::fs::create_dir_all(&dir) {
                tracing::error!(error = %e, dir = %dir.display(), "could not create backup dir");
                continue;
            }
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default();
            let dest = dir.join(format!("sorofy-{stamp}.db"));
            // Blocking sqlite work belongs off the async runtime's worker threads.
            let db = db.clone();
            let result =
                tokio::task::spawn_blocking(move || db.backup_to(&dest).map(|()| dest)).await;
            match result {
                Ok(Ok(path)) => tracing::info!(path = %path.display(), "cache backup written"),
                Ok(Err(e)) => tracing::error!(error = %e, "cache backup failed"),
                Err(e) => tracing::error!(error = %e, "backup task panicked"),
            }
        }
    });
}
