//! palcalc-server — a read-only HTTPS endpoint that serves a Palworld
//! dedicated-server roster (palbox / party / base per pal) to palCalc clients.
//!
//! Security posture (it is intended to face an open internet port):
//!   * TLS only (rustls); a self-signed cert is generated if none is provided
//!     and its fingerprint is printed for client pinning.
//!   * Every data request needs a valid bearer token (constant-time checked).
//!   * Read-only: only `GET /health` and `GET /roster` exist; everything else
//!     is 404/405. No request input ever touches the filesystem.
//!   * Bounded: per-IP rate limit, request timeout, tiny request-body cap, and
//!     a global in-flight cap on the expensive endpoint. The heavy save parse
//!     is cached and de-duplicated so a flood cannot trigger repeated parses.
//!   * Errors are generic to clients; details are logged server-side only.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use tokio::sync::Semaphore;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use palcalc_server::auth::Auth;
use palcalc_server::cache::RosterCache;
use palcalc_server::config::Config;
use palcalc_server::ratelimit::RateLimiter;
use palcalc_server::tls;

#[derive(Clone)]
struct AppState {
    auth: Arc<Auth>,
    cache: Arc<RosterCache>,
    rate: Arc<RateLimiter>,
    /// Global cap on concurrent expensive requests.
    sem: Arc<Semaphore>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install the rustls crypto provider (ring) before any TLS use. Ignoring
    // the result is fine: an Err only means one is already installed.
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cfg_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "palcalc-server.toml".to_string());
    let cfg = Config::load(&cfg_path)?;

    if !cfg.bind.ip().is_loopback() {
        tracing::warn!(
            "binding {} (non-loopback): the server will be reachable off-host — \
             ensure the config's tokens are strong and TLS is in place",
            cfg.bind
        );
    }

    let state = AppState {
        auth: Arc::new(Auth::new(&cfg.tokens)),
        cache: Arc::new(RosterCache::new(cfg.save_dir.clone())),
        rate: Arc::new(RateLimiter::new(cfg.rate_per_second, cfg.rate_burst)),
        sem: Arc::new(Semaphore::new(cfg.max_inflight)),
    };
    tracing::info!(
        "loaded {} access token(s); save dir {:?}",
        cfg.tokens.len(),
        cfg.save_dir
    );

    let tls = tls::load_or_generate(&cfg.tls_cert, &cfg.tls_key).await?;

    // Authenticated data routes.
    let protected = Router::new()
        .route("/roster", get(roster))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    let app = Router::new()
        .route("/health", get(health))
        .merge(protected)
        .fallback(not_found)
        // Innermost → outermost as listed downward; the last `.layer` is
        // outermost, so tracing wraps everything and the rate limit runs
        // before the timeout/handlers.
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(RequestBodyLimitLayer::new(1024))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(cfg.request_timeout_secs),
        ))
        .layer(middleware::from_fn_with_state(state.clone(), rate_limit))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let handle = axum_server::Handle::new();
    {
        let handle = handle.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown signal received; draining");
            handle.graceful_shutdown(Some(Duration::from_secs(5)));
        });
    }

    tracing::info!("listening on https://{}", cfg.bind);
    axum_server::bind_rustls(cfg.bind, tls)
        .handle(handle)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await?;
    Ok(())
}

/// Per-IP rate limit, applied to every request (including unauthenticated
/// ones, so brute-force attempts are throttled too).
async fn rate_limit(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    if state.rate.allow(peer.ip()) {
        next.run(req).await
    } else {
        (StatusCode::TOO_MANY_REQUESTS, "rate limited").into_response()
    }
}

/// Bearer-token gate for data routes.
async fn require_auth(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let header_val = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if state.auth.verify_header(header_val) {
        next.run(req).await
    } else {
        tracing::warn!("unauthorized request from {}", peer.ip());
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))],
            "unauthorized",
        )
            .into_response()
    }
}

async fn health() -> Response {
    // Kept intentionally minimal: no version or build details, so the open
    // (unauthenticated) endpoint gives an attacker nothing to fingerprint.
    Json(serde_json::json!({ "status": "ok" })).into_response()
}

async fn roster(State(state): State<AppState>) -> Response {
    // Cap concurrent expensive work; shed load rather than queue unboundedly.
    let _permit = match state.sem.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => return (StatusCode::SERVICE_UNAVAILABLE, "busy").into_response(),
    };
    match state.cache.body().await {
        Ok(body) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            body,
        )
            .into_response(),
        Err(e) => {
            tracing::error!("roster build failed: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}
