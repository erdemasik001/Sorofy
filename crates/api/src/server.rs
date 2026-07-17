//! The REST layer (PLAN Day2 item 1): `POST /verify` starts a reproduction
//! job, `GET /verify/{contract_id|wasm_hash}` serves the cached outcome.
//!
//! Jobs run on `spawn_blocking` — the engine drives `git`/`docker` as blocking
//! subprocesses — behind a small semaphore: container builds are heavyweight,
//! and an unbounded queue of them is a self-inflicted denial of service.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use tokio::sync::Semaphore;
use verifier_core::{reproduce, Docker, ReproductionRequest, SourceRef, VerificationResult};

use crate::db::{Db, JobStatus};
use crate::rpc::{self, OnChainExecutable};

/// How many container builds may run at once.
const MAX_CONCURRENT_BUILDS: usize = 2;

/// Everything a handler needs. Cheap to clone.
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub docker: Arc<Docker>,
    pub rpc_url: String,
    /// Accept tags without digests (local dev; SEP-58 wants digests).
    pub allow_unpinned_image: bool,
    build_slots: Arc<Semaphore>,
}

impl AppState {
    pub fn new(db: Db, docker: Docker, rpc_url: String, allow_unpinned_image: bool) -> Self {
        AppState {
            db,
            docker: Arc::new(docker),
            rpc_url,
            allow_unpinned_image,
            build_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_BUILDS)),
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/verify", post(start_verification))
        .route("/verify/{key}", get(get_verification))
        .with_state(state)
}

/// `POST /verify` body. SEP-58 field names (`bldimg`, `bldopt`, `source_uri`,
/// `source_sha256`) plus our git extension (`repo` + `rev`) and the on-chain
/// anchor (`contract_id`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyRequest {
    /// Contract whose on-chain hash is the reproduction target. If set, the
    /// expected hash is resolved from the network, not taken from the caller.
    pub contract_id: Option<String>,
    /// Explicit target hash; used when there is no `contract_id`.
    pub wasm_hash: Option<String>,

    /// Git source: repository URL...
    pub repo: Option<String>,
    /// ...and the commit to build.
    pub rev: Option<String>,
    /// SEP-58 `source_uri` (archive), alternative to `repo`.
    pub source_uri: Option<String>,
    /// SEP-58 `source_sha256`, required with `source_uri`.
    pub source_sha256: Option<String>,

    /// SEP-58 `bldimg`.
    pub bldimg: String,
    /// SEP-58 `bldopt`.
    #[serde(default)]
    pub bldopt: Vec<String>,
}

/// A caller mistake, reported as 400/404/422 with a reason.
enum ApiError {
    BadRequest(String),
    NotFound(String),
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (code, msg) = match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::Internal(err) => {
                tracing::error!(error = %err, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}"))
            }
        };
        (code, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        ApiError::Internal(err)
    }
}

async fn index() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "service": "sorofy",
        "endpoints": {
            "POST /verify": "start a verification job",
            "GET /verify/{id|contract_id|wasm_hash}": "cached result",
        },
    }))
}

async fn start_verification(
    State(state): State<AppState>,
    Json(req): Json<VerifyRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let source = parse_source(&req)?;

    // Resolve the target hash. With a contract_id the network is the authority;
    // a caller-supplied wasm_hash is only an anchor when there is nothing
    // on-chain to ask.
    let expected_hash = match (&req.contract_id, &req.wasm_hash) {
        (Some(contract_id), _) => {
            let rpc_url = state.rpc_url.clone();
            let id = contract_id.clone();
            let exec = tokio::task::spawn_blocking(move || rpc::fetch_executable(&rpc_url, &id))
                .await
                .map_err(|e| ApiError::Internal(e.into()))?
                .map_err(|e| ApiError::BadRequest(format!("on-chain lookup failed: {e:#}")))?;
            match exec {
                Some(OnChainExecutable::Wasm { wasm_hash_hex }) => wasm_hash_hex,
                Some(OnChainExecutable::StellarAsset) => {
                    return Err(ApiError::BadRequest(
                        "contract is a built-in Stellar Asset Contract; there is no WASM to verify"
                            .into(),
                    ))
                }
                None => {
                    return Err(ApiError::NotFound(format!(
                        "contract {} does not exist on this network",
                        req.contract_id.as_deref().unwrap_or_default()
                    )))
                }
            }
        }
        (None, Some(hash)) => hash.to_lowercase(),
        (None, None) => {
            return Err(ApiError::BadRequest(
                "pass a contract_id (hash resolved on-chain) or an explicit wasm_hash".into(),
            ))
        }
    };

    let source_json = serde_json::to_value(&source).expect("SourceRef serializes");
    let id = state.db.insert_pending(
        req.contract_id.as_deref(),
        &expected_hash,
        &source_json,
        &req.bldimg,
    )?;

    let job = ReproductionRequest {
        source,
        bldimg: req.bldimg,
        bldopt: req.bldopt,
        expected_wasm_sha256: expected_hash.clone(),
        timeout: verifier_core::DEFAULT_TIMEOUT,
        allow_unpinned_image: state.allow_unpinned_image,
        emit_wasm: None,
    };
    tokio::spawn(run_job(state.clone(), id, job));

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "id": id,
            "status": JobStatus::Pending,
            "wasm_hash": expected_hash,
        })),
    ))
}

/// Exactly one source shape must be present.
fn parse_source(req: &VerifyRequest) -> Result<SourceRef, ApiError> {
    match (&req.repo, &req.source_uri) {
        (Some(repo), None) => Ok(SourceRef::Git {
            repo: repo.clone(),
            rev: req
                .rev
                .clone()
                .ok_or_else(|| ApiError::BadRequest("`repo` requires `rev` (a commit)".into()))?,
        }),
        (None, Some(uri)) => Ok(SourceRef::Archive {
            uri: uri.clone(),
            source_sha256: req.source_sha256.clone().ok_or_else(|| {
                ApiError::BadRequest("`source_uri` requires `source_sha256` (SEP-58)".into())
            })?,
        }),
        _ => Err(ApiError::BadRequest(
            "pass exactly one source: `repo`+`rev`, or `source_uri`+`source_sha256`".into(),
        )),
    }
}

/// Run one reproduction to completion and record the outcome.
async fn run_job(state: AppState, id: i64, job: ReproductionRequest) {
    let permit = state
        .build_slots
        .clone()
        .acquire_owned()
        .await
        .expect("semaphore is never closed");

    let docker = state.docker.clone();
    let outcome =
        tokio::task::spawn_blocking(move || reproduce(&docker, &job)).await;
    drop(permit);

    let recorded = match outcome {
        Ok(Ok(report)) => {
            let status = match report.result {
                VerificationResult::Verified => JobStatus::Verified,
                _ => JobStatus::Mismatch,
            };
            let report_json = serde_json::to_value(&report).expect("report serializes");
            state.db.complete(id, status, &report_json)
        }
        Ok(Err(engine_err)) => state.db.fail(id, &engine_err.to_string()),
        Err(join_err) => state.db.fail(id, &format!("job panicked: {join_err}")),
    };
    if let Err(db_err) = recorded {
        tracing::error!(id, error = %db_err, "failed to record job outcome");
    }
}

async fn get_verification(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // A bare integer is the job id POST /verify returned; anything else is a
    // contract id or wasm hash. The shapes cannot collide.
    let row = match key.parse::<i64>() {
        Ok(id) => state.db.get(id)?,
        Err(_) => state.db.lookup(&key)?,
    };
    match row {
        Some(row) => Ok(Json(serde_json::to_value(&row).expect("row serializes"))),
        None => Err(ApiError::NotFound("not_found".into())),
    }
}
