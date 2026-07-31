//! The REST layer (PLAN Day2 item 1): `POST /verify` starts a reproduction
//! job, `GET /verify/{contract_id|wasm_hash}` serves the cached outcome.
//!
//! Jobs run on `spawn_blocking` — the engine drives `git`/`docker` as blocking
//! subprocesses — behind a small semaphore: container builds are heavyweight,
//! and an unbounded queue of them is a self-inflicted denial of service.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
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

/// Total verification jobs allowed outstanding (running + queued) at once.
///
/// `MAX_CONCURRENT_BUILDS` caps how many run *simultaneously*; this caps how many
/// may be accepted and pending at all — the admission bound. Past it, `POST /verify`
/// returns 429 instead of spawning an unbounded backlog of tasks and pending DB
/// rows (docs/security.md, G3). A few multiples above the build concurrency, so a
/// short burst queues rather than being refused.
const MAX_OUTSTANDING_JOBS: usize = 16;

/// Everything a handler needs. Cheap to clone.
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub docker: Arc<Docker>,
    pub rpc_url: String,
    /// Accept tags without digests (local dev; SEP-58 wants digests).
    pub allow_unpinned_image: bool,
    /// Bearer token required on `POST /verify`. `None` disables auth (local dev);
    /// `main` warns loudly at startup when it is unset.
    pub api_token: Option<String>,
    build_slots: Arc<Semaphore>,
    /// Admission control: caps total outstanding jobs (running + queued) so a
    /// flood of POSTs cannot grow the backlog without bound (docs/security.md, G3).
    admission: Arc<Semaphore>,
}

impl AppState {
    pub fn new(
        db: Db,
        docker: Docker,
        rpc_url: String,
        allow_unpinned_image: bool,
        api_token: Option<String>,
    ) -> Self {
        AppState {
            db,
            docker: Arc::new(docker),
            rpc_url,
            allow_unpinned_image,
            api_token,
            build_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_BUILDS)),
            admission: Arc::new(Semaphore::new(MAX_OUTSTANDING_JOBS)),
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

/// A caller mistake, reported as 400/401/404/500 with a reason.
enum ApiError {
    Unauthorized,
    TooManyRequests,
    BadRequest(String),
    NotFound(String),
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (code, msg) = match self {
            ApiError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "missing or invalid bearer token".to_string(),
            ),
            ApiError::TooManyRequests => (
                StatusCode::TOO_MANY_REQUESTS,
                "too many verification jobs in flight; retry shortly".to_string(),
            ),
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
    headers: HeaderMap,
    Json(req): Json<VerifyRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    // POST spends build capacity and drives the socket-mounted daemon, so it is
    // gated (docs/security.md, G2). GET stays public — a cheap cached read is the
    // whole point of the service.
    if !is_authorized(state.api_token.as_deref(), &headers) {
        return Err(ApiError::Unauthorized);
    }

    let source = parse_source(&req)?;

    // Admission control (docs/security.md, G3): bound total outstanding jobs.
    // try_acquire is non-blocking, so excess load is rejected right here with 429
    // rather than piling up spawned tasks, pending rows, and RPC lookups. The
    // permit is held for the job's whole life and freed only when it finishes.
    let admission = state
        .admission
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::TooManyRequests)?;

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
    tokio::spawn(run_job(state.clone(), id, job, admission));

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "id": id,
            "status": JobStatus::Pending,
            "wasm_hash": expected_hash,
        })),
    ))
}

/// Whether a request carries the configured bearer token.
///
/// `expected == None` means auth is disabled (local dev). Otherwise the
/// `Authorization: Bearer <token>` value must match, compared in constant time so
/// a wrong token can't be recovered byte-by-byte from response timing.
fn is_authorized(expected: Option<&str>, headers: &HeaderMap) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    let provided = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            let mut parts = s.splitn(2, ' ');
            match (parts.next(), parts.next()) {
                (Some(scheme), Some(tok)) if scheme.eq_ignore_ascii_case("bearer") => {
                    Some(tok.trim())
                }
                _ => None,
            }
        })
        .unwrap_or("");
    constant_time_eq(provided.as_bytes(), expected.as_bytes())
}

/// Length-then-content equality that does not short-circuit on the first
/// differing byte. The length comparison leaks length, which is acceptable for a
/// bearer token.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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
///
/// `_admission` is the admission-control permit from `start_verification`; holding
/// it for the whole job (until this function returns) is what makes an admission
/// slot free up only once the job is fully done, not when it was merely accepted.
async fn run_job(
    state: AppState,
    id: i64,
    job: ReproductionRequest,
    _admission: tokio::sync::OwnedSemaphorePermit,
) {
    let permit = state
        .build_slots
        .clone()
        .acquire_owned()
        .await
        .expect("semaphore is never closed");

    let docker = state.docker.clone();
    let outcome = tokio::task::spawn_blocking(move || reproduce(&docker, &job)).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with_auth(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::AUTHORIZATION, value.parse().unwrap());
        h
    }

    #[test]
    fn auth_disabled_when_no_token_configured() {
        // No configured token → open (local dev). main warns at startup.
        assert!(is_authorized(None, &HeaderMap::new()));
        assert!(is_authorized(None, &headers_with_auth("Bearer anything")));
    }

    #[test]
    fn auth_accepts_the_correct_bearer_token() {
        assert!(is_authorized(
            Some("s3cret"),
            &headers_with_auth("Bearer s3cret")
        ));
        // Scheme is case-insensitive (RFC 6750).
        assert!(is_authorized(
            Some("s3cret"),
            &headers_with_auth("bearer s3cret")
        ));
    }

    #[test]
    fn auth_rejects_wrong_missing_or_malformed_tokens() {
        assert!(!is_authorized(
            Some("s3cret"),
            &headers_with_auth("Bearer nope")
        ));
        assert!(!is_authorized(Some("s3cret"), &HeaderMap::new())); // no header
        assert!(!is_authorized(Some("s3cret"), &headers_with_auth("s3cret"))); // no scheme
        assert!(!is_authorized(
            Some("s3cret"),
            &headers_with_auth("Basic s3cret")
        ));
        assert!(!is_authorized(
            Some("s3cret"),
            &headers_with_auth("Bearer ")
        )); // empty token
    }

    #[test]
    fn constant_time_eq_matches_only_identical_byte_strings() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab")); // length differs
    }

    /// A minimal `VerifyRequest` with only `bldimg` set; each test fills in the
    /// source fields it exercises. Keeps the source-selection cases readable.
    fn req() -> VerifyRequest {
        VerifyRequest {
            contract_id: None,
            wasm_hash: None,
            repo: None,
            rev: None,
            source_uri: None,
            source_sha256: None,
            bldimg: "ghcr.io/x/img@sha256:abc".into(),
            bldopt: Vec::new(),
        }
    }

    #[test]
    fn git_source_needs_repo_and_rev() {
        // repo + rev → Git.
        let r = VerifyRequest {
            repo: Some("https://example.com/x.git".into()),
            rev: Some("abc123".into()),
            ..req()
        };
        match parse_source(&r) {
            Ok(SourceRef::Git { repo, rev }) => {
                assert_eq!(repo, "https://example.com/x.git");
                assert_eq!(rev, "abc123");
            }
            _ => panic!("expected a Git source from repo+rev"),
        }

        // repo without rev → 400, not a silent HEAD default (the engine's CLI
        // defaults to HEAD, but the API refuses to guess a moving target).
        let r = VerifyRequest {
            repo: Some("https://example.com/x.git".into()),
            ..req()
        };
        assert!(matches!(parse_source(&r), Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn archive_source_needs_uri_and_sha256() {
        // source_uri + source_sha256 → Archive.
        let r = VerifyRequest {
            source_uri: Some("https://example.com/s.tar.gz".into()),
            source_sha256: Some("DEADBEEF".into()),
            ..req()
        };
        match parse_source(&r) {
            Ok(SourceRef::Archive { uri, source_sha256 }) => {
                assert_eq!(uri, "https://example.com/s.tar.gz");
                // parse_source passes the digest through verbatim; canonicalization
                // is the fetch layer's job (it compares case-insensitively).
                assert_eq!(source_sha256, "DEADBEEF");
            }
            _ => panic!("expected an Archive source from source_uri+source_sha256"),
        }

        // source_uri without source_sha256 → 400 (SEP-58 step 3 needs the digest).
        let r = VerifyRequest {
            source_uri: Some("https://example.com/s.tar.gz".into()),
            ..req()
        };
        assert!(matches!(parse_source(&r), Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn exactly_one_source_shape_is_required() {
        // Both a git repo and an archive → ambiguous → 400.
        let both = VerifyRequest {
            repo: Some("https://example.com/x.git".into()),
            rev: Some("abc".into()),
            source_uri: Some("https://example.com/s.tar.gz".into()),
            source_sha256: Some("abc".into()),
            ..req()
        };
        assert!(matches!(parse_source(&both), Err(ApiError::BadRequest(_))));

        // Neither → 400.
        assert!(matches!(parse_source(&req()), Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn api_errors_map_to_their_status_codes() {
        assert_eq!(
            ApiError::BadRequest("x".into()).into_response().status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ApiError::NotFound("x".into()).into_response().status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            ApiError::TooManyRequests.into_response().status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            ApiError::Internal(anyhow::anyhow!("boom"))
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn unknown_request_fields_are_rejected() {
        // `deny_unknown_fields` guards against a caller misspelling a field (e.g.
        // `wasmhash`) and silently getting a different verification than intended.
        let bad = serde_json::json!({ "bldimg": "img@sha256:abc", "wasmhash": "typo" });
        assert!(serde_json::from_value::<VerifyRequest>(bad).is_err());

        // The correctly-spelled shape still deserializes.
        let good = serde_json::json!({ "bldimg": "img@sha256:abc", "wasm_hash": "aa" });
        assert!(serde_json::from_value::<VerifyRequest>(good).is_ok());
    }

    /// An AppState wired for handler tests: in-memory db, auth off, unpinned images
    /// allowed. Docker is only *constructed* (no daemon contact) — admission rejects
    /// before any reproduction runs, so no daemon is needed.
    fn test_state() -> AppState {
        AppState::new(
            Db::open_in_memory().expect("in-memory db"),
            Docker::autodetect(),
            "http://rpc.invalid".into(),
            true,
            None,
        )
    }

    #[tokio::test]
    async fn admission_full_rejects_with_429() {
        let state = test_state();

        // Drain every admission slot, as if MAX_OUTSTANDING_JOBS jobs were in flight.
        let mut held = Vec::new();
        for _ in 0..MAX_OUTSTANDING_JOBS {
            held.push(
                state
                    .admission
                    .clone()
                    .try_acquire_owned()
                    .expect("slot should be free"),
            );
        }

        // A well-formed POST is now refused at admission — before any RPC/DB/Docker.
        let body = VerifyRequest {
            repo: Some("https://example.com/x.git".into()),
            rev: Some("abc".into()),
            wasm_hash: Some("aa".into()),
            ..req()
        };
        let err = start_verification(State(state.clone()), HeaderMap::new(), Json(body))
            .await
            .expect_err("admission is full");
        assert!(matches!(err, ApiError::TooManyRequests));

        // Freeing a slot restores capacity (the gate is not permanently latched).
        held.pop();
        assert_eq!(state.admission.available_permits(), 1);
    }
}
