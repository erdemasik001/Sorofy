//! The REST layer (PLAN Day2 item 1): `POST /verify` starts a reproduction
//! job, `GET /verify/{contract_id|wasm_hash}` serves the cached outcome.
//!
//! Jobs run on `spawn_blocking` — the engine drives `git`/`docker` as blocking
//! subprocesses — behind a small semaphore: container builds are heavyweight,
//! and an unbounded queue of them is a self-inflicted denial of service.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
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

/// Rate limit on accepted `POST /verify`, per principal (docs/security.md, G3):
/// sustained requests per second, and the burst a caller may spend at once.
/// Admission already bounds *outstanding* work; this bounds the *rate* of new
/// work, so a caller cannot churn the RPC/DB lookups as fast as slots free.
const RATE_LIMIT_REFILL_PER_SEC: f64 = 1.0;
const RATE_LIMIT_BURST: f64 = 10.0;

/// Cap on distinct principals the limiter tracks, so its own map cannot grow
/// without bound; when full, idle (fully-refilled) buckets are evicted first.
const MAX_TRACKED_CLIENTS: usize = 4096;

/// Page size for `GET /verifications` when the caller does not ask for one.
/// `Db::recent` enforces the hard ceiling; this is just a sensible default.
const DEFAULT_PAGE: u32 = 24;

/// A token-bucket rate limiter keyed by principal. Cheap to clone (shared inner).
#[derive(Clone)]
struct RateLimiter {
    inner: Arc<Mutex<HashMap<String, Bucket>>>,
    burst: f64,
    refill_per_sec: f64,
    max_clients: usize,
}

#[derive(Clone, Copy)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Tokens `b` would hold at `now`, capped at `burst`.
fn refilled(b: &Bucket, now: Instant, burst: f64, refill_per_sec: f64) -> f64 {
    let elapsed = now.saturating_duration_since(b.last).as_secs_f64();
    (b.tokens + elapsed * refill_per_sec).min(burst)
}

impl RateLimiter {
    fn new(burst: f64, refill_per_sec: f64, max_clients: usize) -> Self {
        RateLimiter {
            inner: Arc::new(Mutex::new(HashMap::new())),
            burst,
            refill_per_sec,
            max_clients,
        }
    }

    /// Allow one request for `key`, or return how long until a token frees up.
    fn check(&self, key: &str) -> Result<(), Duration> {
        self.check_at(key, Instant::now())
    }

    /// `check` with an injected clock, so the bucket maths are unit-testable.
    fn check_at(&self, key: &str, now: Instant) -> Result<(), Duration> {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // Bound the map: when it is full and this is a new key, drop buckets that
        // have fully refilled (idle clients). If none have, every tracked client
        // is active — allow the insert rather than wrongly throttle a real caller.
        if map.len() >= self.max_clients && !map.contains_key(key) {
            let (burst, refill) = (self.burst, self.refill_per_sec);
            map.retain(|_, b| refilled(b, now, burst, refill) < burst);
        }
        let bucket = map.entry(key.to_string()).or_insert(Bucket {
            tokens: self.burst,
            last: now,
        });
        bucket.tokens = refilled(bucket, now, self.burst, self.refill_per_sec);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            let wait = (1.0 - bucket.tokens) / self.refill_per_sec;
            Err(Duration::from_secs_f64(wait))
        }
    }
}

/// Terminal outcome of a job, for metrics.
#[derive(Clone, Copy)]
enum Outcome {
    Verified,
    Mismatch,
    Errored,
}

/// Process-lifetime counters exposed at `GET /metrics` (docs/security.md / roadmap
/// 0.4). Cheap to clone (shared inner). All counters are monotonic except
/// `in_flight`, which is a gauge kept balanced by one `on_accepted` per job and
/// exactly one `on_finished` per job.
#[derive(Clone, Default)]
struct Metrics {
    inner: Arc<MetricsInner>,
}

#[derive(Default)]
struct MetricsInner {
    submitted: AtomicU64,
    in_flight: AtomicU64,
    verified: AtomicU64,
    mismatch: AtomicU64,
    errored: AtomicU64,
    build_ms_total: AtomicU64,
    builds_timed: AtomicU64,
}

impl Metrics {
    /// A job was accepted and spawned.
    fn on_accepted(&self) {
        self.inner.submitted.fetch_add(1, Ordering::Relaxed);
        self.inner.in_flight.fetch_add(1, Ordering::Relaxed);
    }

    /// A job finished with `outcome`; `build_seconds` is present when a build ran
    /// (i.e. not for an engine error that failed before/without building).
    fn on_finished(&self, outcome: Outcome, build_seconds: Option<f64>) {
        self.inner.in_flight.fetch_sub(1, Ordering::Relaxed);
        let counter = match outcome {
            Outcome::Verified => &self.inner.verified,
            Outcome::Mismatch => &self.inner.mismatch,
            Outcome::Errored => &self.inner.errored,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        if let Some(secs) = build_seconds {
            self.inner
                .build_ms_total
                .fetch_add((secs * 1000.0) as u64, Ordering::Relaxed);
            self.inner.builds_timed.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn snapshot(&self) -> serde_json::Value {
        let m = &self.inner;
        let timed = m.builds_timed.load(Ordering::Relaxed);
        let avg_build_seconds = if timed > 0 {
            (m.build_ms_total.load(Ordering::Relaxed) as f64 / timed as f64) / 1000.0
        } else {
            0.0
        };
        serde_json::json!({
            "jobs_submitted": m.submitted.load(Ordering::Relaxed),
            "jobs_in_flight": m.in_flight.load(Ordering::Relaxed),
            "verified": m.verified.load(Ordering::Relaxed),
            "mismatch": m.mismatch.load(Ordering::Relaxed),
            "error": m.errored.load(Ordering::Relaxed),
            "avg_build_seconds": avg_build_seconds,
        })
    }
}

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
    /// Per-principal request-rate limit on `POST /verify` (docs/security.md, G3).
    rate_limiter: RateLimiter,
    /// Process-lifetime job counters, served at `GET /metrics` (roadmap 0.4).
    metrics: Metrics,
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
            rate_limiter: RateLimiter::new(
                RATE_LIMIT_BURST,
                RATE_LIMIT_REFILL_PER_SEC,
                MAX_TRACKED_CLIENTS,
            ),
            metrics: Metrics::default(),
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route(
            "/app.css",
            get(|headers: HeaderMap| async move {
                asset(
                    APP_CSS.as_bytes(),
                    "text/css; charset=utf-8",
                    etag_of(APP_CSS.as_bytes(), &CSS_ETAG),
                    &headers,
                )
            }),
        )
        .route(
            "/app.js",
            get(|headers: HeaderMap| async move {
                asset(
                    APP_JS.as_bytes(),
                    "text/javascript; charset=utf-8",
                    etag_of(APP_JS.as_bytes(), &JS_ETAG),
                    &headers,
                )
            }),
        )
        .route(
            "/fonts/figtree-latin.woff2",
            get(|headers: HeaderMap| async move {
                asset(
                    FONT_LATIN,
                    "font/woff2",
                    etag_of(FONT_LATIN, &FONT_ETAG),
                    &headers,
                )
            }),
        )
        .route(
            "/fonts/figtree-latin-ext.woff2",
            get(|headers: HeaderMap| async move {
                asset(
                    FONT_LATIN_EXT,
                    "font/woff2",
                    etag_of(FONT_LATIN_EXT, &FONT_EXT_ETAG),
                    &headers,
                )
            }),
        )
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        // The bearer gate is a *route layer*, not a check inside the handler, so
        // it runs before axum's `Json` extractor. With the check in the handler
        // an unauthenticated caller still reached the deserializer and read its
        // messages back (422 on a bad body, 415 on a bad content type) — never a
        // bypass, since no job could be created, but a schema disclosure and a
        // scrap of unauthenticated work that neither needed to exist.
        .route(
            "/verify",
            post(start_verification).route_layer(axum::middleware::from_fn_with_state(
                state.clone(),
                require_bearer_token,
            )),
        )
        .route("/verifications", get(list_verifications))
        .route("/verify/{key}", get(get_verification))
        .with_state(state)
        .layer(axum::middleware::from_fn(log_requests))
        .layer(axum::middleware::from_fn(security_headers))
}

/// Bearer gate on `POST /verify`, ahead of body extraction (docs/security.md).
///
/// `start_verification` checks the same thing again. That redundancy is
/// deliberate: this layer is the gate, the handler's check is what still holds if
/// the route is ever rewired, and an auth control is the wrong place to economise.
async fn require_bearer_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if !is_authorized(state.api_token.as_deref(), request.headers()) {
        return ApiError::Unauthorized.into_response();
    }
    next.run(request).await
}

/// Response security headers for the browsable explorer (docs/security.md, G8).
///
/// The page renders content that originates with strangers — contract ids, source
/// URIs, and above all the build log, which is the output of compiling code we did
/// not write. Escaping is the control that keeps that safe, and it is tested; this
/// is the layer behind it, so that a future escaping slip is a *blocked* script
/// rather than an executed one.
///
/// The policy can afford to be strict because the page was written without a single
/// inline script, inline style, or event-handler attribute, and it loads nothing
/// cross-origin — so `unsafe-inline` is needed nowhere. `data:` is allowed for
/// images alone, which covers the inline SVG favicon and the CSS paper-grain
/// texture. Keep it that way: reaching for `unsafe-inline` to land one quick style
/// would forfeit most of what this header buys.
async fn security_headers(request: Request, next: Next) -> Response {
    const POLICY: &str = "default-src 'none'; \
         script-src 'self'; \
         style-src 'self'; \
         img-src 'self' data:; \
         font-src 'self'; \
         connect-src 'self'; \
         base-uri 'none'; \
         form-action 'self'; \
         frame-ancestors 'none'";

    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(POLICY),
    );
    // The assets are served with explicit content types; this stops a browser
    // second-guessing them, which is how a served text file becomes a script.
    headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // Paths carry contract ids and job numbers; there is no reason to hand them
    // to a third party on an outbound click.
    headers.insert(
        axum::http::header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

/// One structured log line per request: method, path, status, latency (roadmap
/// 0.4). Never logs headers or bodies, so the bearer token is not captured. Health
/// and metrics polls log at DEBUG so a load balancer cannot flood the INFO log.
async fn log_requests(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let started = Instant::now();
    let response = next.run(request).await;
    let status = response.status().as_u16();
    let latency_ms = started.elapsed().as_millis() as u64;
    if path == "/health" || path == "/metrics" {
        tracing::debug!(%method, path, status, latency_ms, "handled request");
    } else {
        tracing::info!(%method, path, status, latency_ms, "handled request");
    }
    response
}

/// Liveness + a cheap cache ping (roadmap 0.4). Public and unauthenticated so a
/// load balancer or uptime check can poll it; 503 if the db does not answer.
async fn health(State(state): State<AppState>) -> Response {
    match state.db.ping() {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "ok", "service": "sorofy" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "status": "degraded", "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}

/// Process-lifetime job counters (roadmap 0.4). Public: a single-tenant testnet
/// box exposes only aggregate counts, no per-request data.
async fn metrics(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(state.metrics.snapshot())
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
    /// Admission queue full — too many jobs already outstanding.
    TooManyRequests,
    /// Per-principal request rate exceeded; carries the suggested wait.
    RateLimited(Duration),
    BadRequest(String),
    NotFound(String),
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (code, msg, retry_after_secs) = match self {
            ApiError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "missing or invalid bearer token".to_string(),
                None,
            ),
            ApiError::TooManyRequests => (
                StatusCode::TOO_MANY_REQUESTS,
                "too many verification jobs in flight; retry shortly".to_string(),
                None,
            ),
            ApiError::RateLimited(retry_after) => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate limit exceeded; slow down".to_string(),
                // Round up to a whole second, and never advertise 0.
                Some(retry_after.as_secs_f64().ceil().max(1.0) as u64),
            ),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg, None),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg, None),
            ApiError::Internal(err) => {
                tracing::error!(error = %err, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}"), None)
            }
        };
        let mut resp = (code, Json(serde_json::json!({ "error": msg }))).into_response();
        if let Some(secs) = retry_after_secs {
            resp.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                secs.to_string().parse().expect("integer is a valid header"),
            );
        }
        resp
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        ApiError::Internal(err)
    }
}

/// The explorer, compiled into the binary.
///
/// `include_str!` rather than a static-file directory: the service ships as one
/// container and one process, and an asset that can go missing at runtime is a
/// deploy failure mode we would have to write a playbook step for. There is no
/// build step, no package manager, and nothing fetched from a CDN — the page
/// works on a host with no egress, which is the same property the build sandbox
/// is held to.
const INDEX_HTML: &str = include_str!("../static/index.html");
const APP_CSS: &str = include_str!("../static/app.css");
const APP_JS: &str = include_str!("../static/app.js");

/// Figtree, SIL OFL 1.1 — the licence travels with the font in
/// `static/fonts/OFL.txt`, which the OFL requires us to ship alongside it.
/// Two subsets, split the way Google splits them, each a variable file covering
/// weights 300–900.
const FONT_LATIN: &[u8] = include_bytes!("../static/fonts/figtree-latin.woff2");
const FONT_LATIN_EXT: &[u8] = include_bytes!("../static/fonts/figtree-latin-ext.woff2");

/// `GET /` — the explorer for browsers, the endpoint listing for API clients.
///
/// Negotiated on `Accept` and deliberately biased to JSON: only a client that
/// explicitly asks for `text/html` gets the page. A browser does (it leads with
/// `text/html`), while `curl`'s `Accept: */*` keeps returning exactly the JSON it
/// returned before the explorer existed, so no existing caller changes shape.
async fn index(headers: HeaderMap) -> Response {
    let wants_html = headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"));

    if wants_html {
        return asset(
            INDEX_HTML.as_bytes(),
            "text/html; charset=utf-8",
            etag_of(INDEX_HTML.as_bytes(), &INDEX_ETAG),
            &headers,
        );
    }
    Json(serde_json::json!({
        "service": "sorofy",
        "endpoints": {
            "POST /verify": "start a verification job",
            "GET /verify/{id|contract_id|wasm_hash}": "cached result",
            "GET /verifications?limit=&offset=": "newest-first page of results",
            "GET /health": "liveness + cache ping",
            "GET /metrics": "job counters",
        },
    }))
    .into_response()
}

/// Serve one embedded asset with its content type, validated by ETag.
///
/// The asset URLs are fixed, so they cannot carry a version — which rules out a
/// plain `max-age`: the page and the stylesheet expire independently, and a
/// deploy that changes both would happily pair new HTML with a cached old
/// stylesheet. (Observed, not theorised: it served the previous palette after a
/// rebuild.) `no-cache` does not mean "do not store", it means "revalidate
/// before use", so with a content ETag the steady state is a 304 and a changed
/// asset is picked up on the next load.
fn asset(
    body: &'static [u8],
    content_type: &'static str,
    etag: &str,
    headers: &HeaderMap,
) -> Response {
    let known = headers
        .get(axum::http::header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|candidate| candidate.trim() == etag));

    let base = [
        (axum::http::header::CACHE_CONTROL, "no-cache".to_string()),
        (axum::http::header::ETAG, etag.to_string()),
    ];
    if known {
        return (StatusCode::NOT_MODIFIED, base).into_response();
    }
    (
        base,
        [(axum::http::header::CONTENT_TYPE, content_type)],
        body,
    )
        .into_response()
}

/// `"sha256-prefix"` of an asset, computed once. Content-derived so it changes
/// exactly when the asset does, and no more often.
fn etag_of(body: &'static [u8], cell: &'static std::sync::OnceLock<String>) -> &'static str {
    cell.get_or_init(|| format!("\"{}\"", &verifier_core::sha256_hex(body)[..16]))
}

static INDEX_ETAG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static CSS_ETAG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static JS_ETAG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static FONT_ETAG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static FONT_EXT_ETAG: std::sync::OnceLock<String> = std::sync::OnceLock::new();

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

    // Rate-limit accepted callers (docs/security.md, G3). Keyed by principal, so a
    // leaked token is throttled in aggregate however many hosts replay it. Checked
    // after auth: an unauthenticated flood is already cheap (401) and must not be
    // able to fill the limiter's map.
    if let Err(retry_after) = state
        .rate_limiter
        .check(&rate_limit_key(state.api_token.as_deref(), &headers))
    {
        return Err(ApiError::RateLimited(retry_after));
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
    state.metrics.on_accepted();
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
    let provided = bearer_token(headers).unwrap_or("");
    constant_time_eq(provided.as_bytes(), expected.as_bytes())
}

/// The token from an `Authorization: Bearer <token>` header, if well-formed.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
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
}

/// Rate-limit key = the authenticated principal. With auth on that is the bearer
/// token (so a leaked token is throttled in aggregate across every host replaying
/// it); with auth off (local dev), all callers share one bucket.
fn rate_limit_key(api_token: Option<&str>, headers: &HeaderMap) -> String {
    match api_token {
        Some(_) => bearer_token(headers)
            .map(|t| format!("tok:{t}"))
            .unwrap_or_else(|| "anon".to_string()),
        None => "anon".to_string(),
    }
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

    let ((outcome, build_seconds), recorded) = match outcome {
        Ok(Ok(report)) => {
            let (status, outcome) = match report.result {
                VerificationResult::Verified => (JobStatus::Verified, Outcome::Verified),
                _ => (JobStatus::Mismatch, Outcome::Mismatch),
            };
            let build_seconds = report.build_seconds;
            let report_json = serde_json::to_value(&report).expect("report serializes");
            (
                (outcome, Some(build_seconds)),
                state.db.complete(id, status, &report_json),
            )
        }
        Ok(Err(engine_err)) => (
            (Outcome::Errored, None),
            state.db.fail(id, &engine_err.to_string()),
        ),
        Err(join_err) => (
            (Outcome::Errored, None),
            state.db.fail(id, &format!("job panicked: {join_err}")),
        ),
    };
    state.metrics.on_finished(outcome, build_seconds);
    if let Err(db_err) = recorded {
        tracing::error!(id, error = %db_err, "failed to record job outcome");
    }
}

/// Query string of `GET /verifications`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    limit: Option<u32>,
    offset: Option<u32>,
}

/// Newest-first page of verifications — what the explorer's landing view reads.
///
/// Public, like the other reads: the cache *is* the product's public record, and
/// a verification anyone can look up one at a time is not made more secret by
/// being hard to enumerate.
async fn list_verifications(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<ListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_PAGE).min(crate::db::MAX_PAGE);
    let offset = query.offset.unwrap_or(0);
    let items = state.db.recent(limit, offset)?;
    // `total` lets a client page without walking off the end; it is read after
    // the page, so a job finishing in between can only make it a touch stale,
    // never inconsistent with the rows already returned.
    let total = state.db.count()?;
    Ok(Json(serde_json::json!({
        "total": total,
        "limit": limit,
        "offset": offset,
        "items": items,
    })))
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
    fn the_embedded_assets_are_present_and_wired_together() {
        // include_str! would fail the build if a file were missing, so what is
        // worth asserting is that they still reference each other: a renamed
        // route or a dropped tag turns the explorer into a blank page, and
        // nothing else in the suite would notice.
        assert!(
            INDEX_HTML.contains("/app.css"),
            "index must link the stylesheet"
        );
        assert!(INDEX_HTML.contains("/app.js"), "index must load the script");
        assert!(
            INDEX_HTML.contains("id=\"view\""),
            "the script renders into #view"
        );
        assert!(
            APP_CSS.contains(".brut-card"),
            "the ported primitives must be there"
        );
        assert_eq!(&FONT_LATIN[..4], b"wOF2", "latin subset must be real WOFF2");
        assert_eq!(
            &FONT_LATIN_EXT[..4],
            b"wOF2",
            "latin-ext subset must be real WOFF2"
        );
        // The @font-face `src` and the routes must agree, or the browser asks
        // for a URL that 404s and the page silently falls back to a system face.
        assert!(APP_CSS.contains("/fonts/figtree-latin.woff2"));
        assert!(APP_CSS.contains("/fonts/figtree-latin-ext.woff2"));
        assert!(
            APP_JS.contains("/verifications"),
            "the explorer reads the listing endpoint"
        );
    }

    #[test]
    fn the_explorer_never_writes_untrusted_text_as_markup() {
        // The build log is the output of compiling a stranger's source. It must
        // reach the DOM as text; esc() covers the rest of the interpolations.
        assert!(
            APP_JS.contains("pre.textContent = row.report.build_log"),
            "the build log must be inserted with textContent, not as markup"
        );
        assert!(
            !APP_JS.contains("innerHTML = row"),
            "no raw row interpolation"
        );
    }

    /// The CSP is only worth its `style-src 'self'` while the assets stay free of
    /// inline styles — and one can hide inside a JS string literal, which is
    /// exactly how it got past review the first time: the markup had none, but
    /// `app.js` built `<span class="spin" style="display:inline-block">` at
    /// runtime and the browser blocked it. Cosmetic then; the next one might not
    /// be, and the tempting fix is always to loosen the policy.
    #[test]
    fn the_explorer_assets_carry_no_inline_styles() {
        for (name, source) in [("index.html", INDEX_HTML), ("app.js", APP_JS)] {
            assert!(
                !source.contains("style=\""),
                "{name} has an inline style attribute; \
                 move it to a class in app.css rather than relaxing style-src"
            );
        }
        // Runtime styling through the CSSOM is blocked by the same directive.
        assert!(
            !APP_JS.contains(".style.") && !APP_JS.contains("cssText"),
            "app.js styles an element directly; use a class instead"
        );
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

    /// A state with auth on, for the router-level gate tests below.
    fn gated_state() -> AppState {
        AppState::new(
            Db::open_in_memory().expect("in-memory db"),
            Docker::autodetect(),
            "http://rpc.invalid".into(),
            true,
            Some("s3cret".into()),
        )
    }

    fn post_verify(token: Option<&str>) -> Request {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/verify")
            .header("content-type", "application/json");
        if let Some(t) = token {
            builder = builder.header("authorization", format!("Bearer {t}"));
        }
        // A body that cannot deserialize into `VerifyRequest`: reaching the
        // extractor is observable as a 422, being stopped short of it as a 401.
        builder
            .body(axum::body::Body::from("{}"))
            .expect("build request")
    }

    /// The bearer gate runs *ahead* of the body extractor.
    ///
    /// While the check lived inside the handler, axum deserialized first, so an
    /// unauthenticated caller got `422` carrying the deserializer's complaint
    /// (`missing field bldimg`). Never a bypass — no job could be created — but it
    /// disclosed the request schema and did work for a request already destined to
    /// be refused. Surfaced by the 0.7 deploy; see docs/security.md.
    #[tokio::test]
    async fn unauthenticated_post_is_refused_before_the_body_is_parsed() {
        use tower::ServiceExt;

        let response = router(gated_state())
            .oneshot(post_verify(None))
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// The counterpart, so the test above cannot pass by rejecting everything:
    /// with the right token the request gets past the gate and the extractor does
    /// its job, which a malformed body makes visible as a 422.
    #[tokio::test]
    async fn authenticated_post_reaches_the_body_extractor() {
        use tower::ServiceExt;

        let response = router(gated_state())
            .oneshot(post_verify(Some("s3cret")))
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// Responses carry the explorer's security headers, and the policy stays
    /// strict (docs/security.md, G8).
    ///
    /// The page is written without inline script or style, so `unsafe-inline`
    /// should never appear here. If a future change seems to need it, that change
    /// is what to reconsider — not this assertion.
    #[tokio::test]
    async fn responses_carry_a_strict_content_security_policy() {
        use tower::ServiceExt;

        let response = router(test_state())
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(axum::body::Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("router response");

        let csp = response
            .headers()
            .get(axum::http::header::CONTENT_SECURITY_POLICY)
            .and_then(|v| v.to_str().ok())
            .expect("CSP header present");

        assert!(csp.contains("default-src 'none'"), "{csp}");
        assert!(csp.contains("script-src 'self'"), "{csp}");
        assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
        assert!(!csp.contains("unsafe-inline"), "policy loosened: {csp}");
        assert!(!csp.contains("unsafe-eval"), "policy loosened: {csp}");

        assert_eq!(
            response
                .headers()
                .get(axum::http::header::X_CONTENT_TYPE_OPTIONS)
                .and_then(|v| v.to_str().ok()),
            Some("nosniff")
        );
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
        // Rate-limit rejection is 429 and advertises a Retry-After (rounded up).
        let limited = ApiError::RateLimited(Duration::from_millis(2500)).into_response();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            limited
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .unwrap(),
            "3"
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

    #[test]
    fn rate_limiter_allows_burst_then_blocks_then_refills() {
        let rl = RateLimiter::new(3.0, 1.0, 16);
        let t0 = Instant::now();
        // A burst of 3 is allowed at one instant.
        assert!(rl.check_at("p", t0).is_ok());
        assert!(rl.check_at("p", t0).is_ok());
        assert!(rl.check_at("p", t0).is_ok());
        // The 4th at the same instant is limited, with a positive wait hint.
        let wait = rl.check_at("p", t0).expect_err("bucket is empty");
        assert!(wait > Duration::ZERO);
        // One second on, exactly one token has refilled: one more, then blocked.
        let t1 = t0 + Duration::from_secs(1);
        assert!(rl.check_at("p", t1).is_ok());
        assert!(rl.check_at("p", t1).is_err());
        // A different principal has its own independent bucket.
        assert!(rl.check_at("other", t0).is_ok());
    }

    #[test]
    fn rate_limit_key_is_token_when_auth_on_else_anon() {
        // Auth on → keyed by the presented bearer token (per-principal).
        assert_eq!(
            rate_limit_key(Some("s3cret"), &headers_with_auth("Bearer abc")),
            "tok:abc"
        );
        // Auth on but no/blank token → a shared fallback bucket.
        assert_eq!(rate_limit_key(Some("s3cret"), &HeaderMap::new()), "anon");
        // Auth off → everyone shares one bucket.
        assert_eq!(
            rate_limit_key(None, &headers_with_auth("Bearer abc")),
            "anon"
        );
    }

    #[tokio::test]
    async fn rate_limited_principal_gets_429() {
        let state = test_state(); // auth off → the handler keys on "anon"
                                  // Spend the whole burst for the key the handler will use.
        for _ in 0..RATE_LIMIT_BURST as u32 {
            state
                .rate_limiter
                .check("anon")
                .expect("burst is available");
        }
        // The next POST is refused at the rate limiter — before parse/admission/RPC.
        let body = VerifyRequest {
            repo: Some("https://example.com/x.git".into()),
            rev: Some("abc".into()),
            wasm_hash: Some("aa".into()),
            ..req()
        };
        let err = start_verification(State(state.clone()), HeaderMap::new(), Json(body))
            .await
            .expect_err("rate limited");
        assert!(matches!(err, ApiError::RateLimited(_)));
    }

    #[test]
    fn metrics_track_accepted_and_outcomes() {
        let m = Metrics::default();
        m.on_accepted();
        m.on_accepted();
        // Two accepted, two in flight, none finished.
        let s = m.snapshot();
        assert_eq!(s["jobs_submitted"], 2);
        assert_eq!(s["jobs_in_flight"], 2);

        m.on_finished(Outcome::Verified, Some(4.0));
        m.on_finished(Outcome::Mismatch, Some(2.0));
        let s = m.snapshot();
        assert_eq!(s["jobs_in_flight"], 0); // gauge back to zero
        assert_eq!(s["verified"], 1);
        assert_eq!(s["mismatch"], 1);
        assert_eq!(s["error"], 0);
        assert_eq!(s["avg_build_seconds"], 3.0); // mean of 4s and 2s

        // An engine error with no build contributes to `error` but not the average.
        m.on_accepted();
        m.on_finished(Outcome::Errored, None);
        let s = m.snapshot();
        assert_eq!(s["error"], 1);
        assert_eq!(s["avg_build_seconds"], 3.0);
    }

    #[tokio::test]
    async fn health_is_ok_when_the_db_answers() {
        let resp = health(State(test_state())).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn metrics_endpoint_returns_the_snapshot() {
        let state = test_state();
        state.metrics.on_accepted();
        let Json(body) = metrics(State(state)).await;
        assert_eq!(body["jobs_submitted"], 1);
        assert_eq!(body["jobs_in_flight"], 1);
    }
}
