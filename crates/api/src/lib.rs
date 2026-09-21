mod admin;
pub mod api_error;
pub mod billing;
mod chat;
mod chat_paid;
pub mod clerk;
mod context_api;
// Public because tests/context_flow_integration_test.rs exercises it as a
// consumer would. That test has never compiled in CI: the rust job ran
// `cargo test --workspace --lib`, which excludes tests/.
pub mod check_runner;
pub mod context_flow;
mod conversations;
mod cortex_groups;
pub mod db;
mod deploy_status;
pub mod docker;
pub mod ecosystem_probe;
pub mod github;
pub mod github_repos;
mod integrations;
pub mod key_material;
mod lock;
pub mod metrics;
pub mod mission_control;
pub mod pricing;
pub mod provider_gateway;
mod provider_gateway_http;
mod supplier_anthropic;
mod supplier_openai;
pub mod verification_dispatcher;
pub mod verification_driver;
mod voice;
mod voice_session;
// pub mod memory; // removed for Context-Flow Pipeline deployment
// mod orchestrator; // removed for Context-Flow Pipeline deployment
mod ratelimit;
pub mod replit;
pub mod routes;
mod run_payload;
mod run_stream;
pub mod scheduler;
#[cfg(feature = "soma")]
pub mod soma;
#[cfg(feature = "soma")]
mod soma_bridge;
pub mod soma_fence;
pub mod state;
pub mod storage;

/// Build the loopback-only gateway surface used by the no-network CLI proof.
///
/// This is deliberately absent from normal builds and contains only the
/// production stub listener plus proof health/status endpoints.
#[cfg(feature = "gateway-cli-proof")]
#[doc(hidden)]
pub fn build_gateway_cli_proof_router(
    db: db::Database,
    signing_key: Vec<u8>,
    authorization_id: String,
) -> Router {
    provider_gateway_http::proof_router(db, signing_key, authorization_id)
}
pub mod stripe_client;
mod usage_api;
mod user;
mod validate;
pub mod vera;
pub mod worker_key;
mod ws;

pub use api_error::ApiError;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{any, delete, get, patch, post, put};
use axum::Router;
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};
use tower_http::services::ServeDir;

use state::AppState;

async fn product_v1_health(service: &'static str) -> impl axum::response::IntoResponse {
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    axum::Json(serde_json::json!({
        "status": "ok",
        "service": service,
        "version": "0.1.0",
        "timestamp": timestamp,
    }))
}

async fn cortex_v1_health() -> impl axum::response::IntoResponse {
    product_v1_health("cortex").await
}

async fn product_v1_ready(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    let db_ok = match &state.db {
        Some(database) => database.health_check(),
        None => false,
    };
    let ready = db_ok;
    let status = if ready {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        axum::Json(serde_json::json!({
            "ready": ready,
            "db": db_ok,
        })),
    )
}

async fn cortex_v1_ready(
    state: axum::extract::State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    product_v1_ready(state).await
}

fn cors_layer() -> CorsLayer {
    let allowed_origins = std::env::var("CORTEX_ALLOWED_ORIGINS").ok();
    match allowed_origins {
        Some(origins) if !origins.is_empty() => {
            let origins: Vec<_> = origins
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            tracing::info!("CORS: restricted to {} origin(s)", origins.len());
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(origins))
                .allow_methods(AllowMethods::any())
                .allow_headers(AllowHeaders::any())
        }
        _ => {
            if is_production_env() {
                panic!("CORTEX_ALLOWED_ORIGINS is required when HEYVERA_ENV/CORTEX_ENV/APP_ENV/ENVIRONMENT is production");
            }
            tracing::info!("CORS: permissive (set CORTEX_ALLOWED_ORIGINS to restrict)");
            CorsLayer::permissive()
        }
    }
}

async fn deploy_metadata() -> impl axum::response::IntoResponse {
    axum::Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "service": "cortex-api",
        "build_time": option_env!("BUILD_TIME").unwrap_or("unknown"),
    }))
}

#[derive(Clone, Debug)]
pub struct RequestId(pub String);

/// Correlation + observability middleware.
///
/// Responsibilities:
///   - Assign (or honour an inbound) request id and surface it on the response
///     as `x-request-id`, plus stash it in request extensions for handlers.
///   - Open a tracing span carrying the request id, method and matched route so
///     every log line emitted while serving the request is correlated.
///   - Record Prometheus metrics: in-flight gauge, total count by
///     method/route/status-class, 5xx error count, and a latency histogram.
///
/// The `route` label is the matched router template (e.g.
/// `/api/runs/{id}`), never the raw path, to keep metric cardinality bounded.
async fn request_id_middleware(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    // Honour an upstream-supplied request id if present (lets a gateway thread
    // correlation through), otherwise mint a fresh one.
    let request_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let method = req.method().clone();
    let path = req.uri().path().to_string();

    // Matched route template for low-cardinality metric labels. Falls back to a
    // sentinel for unmatched paths (static files, 404s) so we never label
    // metrics with unbounded raw paths.
    let route = req
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "<unmatched>".to_string());

    let start = std::time::Instant::now();

    let incoming_traceparent = req
        .headers()
        .get("traceparent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let (mut parts, body) = req.into_parts();
    parts.extensions.insert(RequestId(request_id.clone()));
    let req = axum::http::Request::from_parts(parts, body);

    // Span ties all downstream logs to this request for correlation. It is
    // attached to the handler future via `.instrument` so it follows the
    // request across every `.await` point.
    let span = tracing::info_span!(
        "http_request",
        request_id = %request_id,
        method = %method,
        route = %route,
    );

    let m = metrics::metrics();
    m.http_in_flight.inc();

    let response = {
        use tracing::Instrument as _;
        next.run(req).instrument(span).await
    };

    let elapsed = start.elapsed();
    let duration_ms = elapsed.as_millis() as u64;
    let status = response.status().as_u16();
    let class = metrics::status_class(status);

    m.http_in_flight.dec();
    m.http_requests_total
        .with_label_values(&[method.as_str(), &route, class])
        .inc();
    m.http_request_duration_seconds
        .with_label_values(&[method.as_str(), &route])
        .observe(elapsed.as_secs_f64());
    if status >= 500 {
        m.http_errors_total
            .with_label_values(&[method.as_str(), &route])
            .inc();
        tracing::error!(
            request_id = %request_id,
            method = %method,
            path = %path,
            route = %route,
            status = status,
            duration_ms = duration_ms,
            "request failed"
        );
    } else {
        tracing::info!(
            request_id = %request_id,
            method = %method,
            path = %path,
            route = %route,
            status = status,
            duration_ms = duration_ms,
            "request completed"
        );
    }

    let (mut parts, body) = response.into_parts();
    parts.headers.insert(
        "x-request-id",
        axum::http::HeaderValue::from_str(&request_id)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("unknown")),
    );

    if let Some(traceparent) = incoming_traceparent {
        if let Ok(val) = axum::http::HeaderValue::from_str(&traceparent) {
            parts.headers.insert("traceparent", val);
        }
    }

    axum::response::Response::from_parts(parts, body)
}

/// GET /metrics — Prometheus text exposition format.
///
/// Refreshes subsystem and container-population gauges on scrape so the
/// snapshot is current, then renders the registry. Unauthenticated by design:
/// it is meant to be scraped on the internal network; restrict via network
/// policy / reverse proxy in production.
async fn cortex_metrics_handler(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    // Refresh point-in-time gauges so a scrape reflects live state even between
    // event-driven updates.
    let db_up = state.db.as_ref().map(|d| d.health_check()).unwrap_or(false);
    metrics::set_subsystem_up("database", db_up);
    metrics::set_subsystem_up("docker", state.container_manager.is_some());

    if let Some(db) = &state.db {
        let containers = db.list_all_containers();
        let running = containers.iter().filter(|c| c.status == "running").count() as i64;
        let stopped = (containers.len() as i64) - running;
        metrics::set_container_population(running, stopped);
    }

    match metrics::render() {
        Ok(body) => (
            axum::http::StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            )],
            body,
        )
            .into_response(),
        Err(e) => {
            tracing::error!("failed to render metrics: {e}");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "failed to render metrics",
            )
                .into_response()
        }
    }
}

/// True when any product env flag is set to production.
/// Checks HEYVERA_ENV, CORTEX_ENV, APP_ENV, RUST_ENV, and ENVIRONMENT.
pub(crate) fn is_production_env() -> bool {
    [
        "HEYVERA_ENV",
        "CORTEX_ENV",
        "APP_ENV",
        "RUST_ENV",
        "ENVIRONMENT",
    ]
    .iter()
    .filter_map(|key| std::env::var(key).ok())
    .any(|value| value.eq_ignore_ascii_case("production") || value.eq_ignore_ascii_case("prod"))
}

fn is_api_path(path: &str) -> bool {
    path == "/metrics"
        || path.starts_with("/api/")
        || path.starts_with("/v1/")
        || path.starts_with("/internal/")
}

async fn api_not_found() -> axum::http::StatusCode {
    axum::http::StatusCode::NOT_FOUND
}

fn api_not_found_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/{*rest}", any(api_not_found))
        .route("/v1/{*rest}", any(api_not_found))
        .route("/internal/{*rest}", any(api_not_found))
}

/// Build router with only Cortex routes (cortex.heyvera.org).
pub fn build_cortex_router(state: Arc<AppState>) -> Router {
    let cortex_static_dir =
        std::env::var("CORTEX_STATIC_DIR").unwrap_or_else(|_| "cortex/dist".to_string());

    async fn spa_fallback_cortex(
        req: axum::http::Request<axum::body::Body>,
    ) -> Result<axum::response::Response, std::convert::Infallible> {
        if is_api_path(req.uri().path()) {
            return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
        }
        let static_dir =
            std::env::var("CORTEX_STATIC_DIR").unwrap_or_else(|_| "cortex/dist".to_string());
        let index_path = format!("{}/index.html", static_dir);
        let response = match std::fs::read_to_string(&index_path) {
            Ok(content) => axum::response::Html(content).into_response(),
            Err(_) => (
                axum::http::StatusCode::NOT_FOUND,
                axum::response::Html("<!DOCTYPE html><html><body><h1>Cortex Frontend Not Available</h1></body></html>")
            ).into_response(),
        };
        Ok(response)
    }

    let static_service =
        ServeDir::new(&cortex_static_dir).not_found_service(tower::service_fn(spa_fallback_cortex));

    let rate_limited = Router::new()
        .route("/api/chat", post(chat::chat))
        .route("/api/voice/dictation/token", post(voice::dictation_token))
        .route(
            "/api/voice/live/sessions",
            post(voice_session::live_session_start),
        )
        .route(
            "/api/voice/live/sessions/{id}",
            delete(voice_session::live_session_close),
        )
        .route("/api/runs", get(routes::list_runs).post(routes::create_run))
        .route("/api/runs/estimate", post(routes::estimate_run))
        .route("/api/runs/{id}", get(routes::get_run))
        .route("/api/runs/{id}/events", get(routes::get_run_events))
        .route(
            "/api/runs/{run_id}/steps/{step_id}/verifier-report/{report_id}",
            get(routes::get_verifier_report),
        )
        .route(
            "/api/runs/{run_id}/steps/{step_id}/receipt",
            get(routes::get_receipt),
        )
        .route("/api/runs/{id}/pr", post(routes::create_pr))
        .route("/api/runs/{id}/stream", get(run_stream::stream_run))
        .route("/api/chat/suggestions", get(chat::chat_suggestions))
        .route("/api/chat/options", post(chat::chat_options))
        .route(
            "/api/conversations",
            post(conversations::create_conversation),
        )
        .route(
            "/api/conversations/{id}/messages",
            post(conversations::add_message),
        )
        .route(
            "/api/context/runs/{run_id}/artifacts",
            get(context_api::list_artifacts_for_run),
        )
        .route(
            "/api/context/runs/{run_id}/context",
            get(context_api::preview_context_for_run),
        )
        .route("/api/context/stats", get(context_api::get_context_stats))
        .route("/api/context/health", get(context_api::get_context_health))
        .route("/api/context/impact", get(context_api::get_impact_set))
        .route(
            "/api/context/test",
            post(context_api::test_context_assembly),
        )
        .route("/api/github/repos", get(github_repos::list_repos))
        .route("/api/github/imports", get(github_repos::list_imports))
        .route("/api/github/import", post(github_repos::import_repo))
        .route(
            "/api/github/status/{import_id}",
            get(github_repos::import_status),
        )
        .route(
            "/api/github/sync/{import_id}",
            post(github_repos::sync_repo),
        )
        .route(
            "/api/projects",
            get(replit::list_projects).post(replit::create_project),
        )
        .route("/api/projects/import", post(replit::import_project))
        .route(
            "/api/projects/{id}",
            get(replit::get_project).delete(replit::delete_project),
        )
        .route(
            "/api/projects/{id}/chat",
            post(replit::proxy_chat_to_workspace),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            ratelimit::rate_limit_middleware,
        ));

    let admin_routes = Router::new()
        .route("/api/admin/workers", get(admin::get_workers))
        .route("/api/admin/stats", get(admin::system_stats))
        .route("/api/admin/decisions", get(admin::list_decisions))
        .route("/api/admin/runs", get(admin::list_all_runs))
        .route("/api/admin/runs/{id}", get(admin::get_run_detail))
        .route("/api/admin/pressure", get(admin::pressure_dashboard))
        .route("/api/admin/usage", get(usage_api::admin_usage))
        .route("/api/admin/usage/users", get(usage_api::admin_usage_users))
        .route(
            "/api/admin/codes",
            get(admin::list_promo_codes).post(admin::create_promo_code),
        )
        .route(
            "/api/admin/codes/{id}",
            patch(admin::update_promo_code).delete(admin::delete_promo_code),
        )
        .route("/api/admin/redemptions", get(admin::list_redemptions))
        .route("/api/admin/audit-log", get(admin::get_audit_log))
        .route("/api/admin/containers", get(admin::list_containers))
        .route("/api/admin/containers/stats", get(admin::container_stats))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin::require_admin_middleware,
        ));

    Router::new()
        .route(
            "/internal/provider/v1/messages",
            post(provider_gateway_http::messages),
        )
        .route("/v1/health", get(cortex_v1_health))
        .route("/v1/ready", get(cortex_v1_ready))
        .route("/metrics", get(cortex_metrics_handler))
        .route("/api/health", get(routes::health))
        .route("/api/deploy-info", get(routes::deploy_info))
        .route("/api/deploy-metadata", get(deploy_metadata))
        .route("/api/deploy-status", get(deploy_status::deploy_status))
        .route("/api/deployment/status", get(deploy_status::deploy_status))
        .route("/api/deployment/events", get(deploy_status::deployment_events))
        .route("/api/deployment/adapters", get(routes::get_deployment_adapters))
        .route("/api/providers", get(routes::get_providers))
        .route("/api/ledger", get(routes::get_ledger))
        .route("/api/conversations", get(conversations::list_conversations))
        .route("/api/conversations/{id}", get(conversations::get_conversation))
        .route("/api/conversations/{id}", patch(conversations::update_conversation))
        .route("/api/conversations/{id}", delete(conversations::delete_conversation))
        .route("/api/user/profile", get(user::get_profile))
        .route("/api/user/routing", get(user::get_routing_profile))
        .route("/api/user/routing", post(user::update_profile))
        .route("/api/user/github/status", get(user::github_status))
        .route("/api/user/repos/select", post(user::select_repos))
        .route("/api/integrations/status", get(integrations::integration_status))
        .route("/api/integrations/slack/oauth/start", post(integrations::slack_oauth_start))
        .route("/api/integrations/slack/oauth/callback", get(integrations::slack_oauth_callback))
        .route("/api/integrations/slack/channels", get(integrations::slack_channels))
        .route("/api/integrations/slack/import-channels", post(integrations::import_slack_channels))
        .route("/api/integrations/slack/events", post(integrations::slack_events))
        .route("/api/integrations/slack/command", post(integrations::slack_command))
        .route("/api/integrations/replit/workspaces", get(integrations::replit_workspaces))
        .route("/api/integrations/replit/import", post(integrations::import_replit_workspace))
        .route("/api/groups/{group_id}/tasks", get(cortex_groups::get_group_tasks))
        .route("/api/groups/{group_id}/tasks", post(cortex_groups::create_group_task))
        .route("/api/groups/{group_id}/tasks", put(cortex_groups::update_group_tasks))
        .route("/api/groups/{group_id}/tasks/actions", post(cortex_groups::apply_group_task_actions))
        .route("/api/groups/{group_id}/tasks/{task_id}", patch(cortex_groups::patch_group_task))
        .route("/api/groups/{group_id}/tasks/{task_id}/chats", post(cortex_groups::attach_group_task_chat))
        .route("/api/groups/{group_id}/tasks/{task_id}/projection", get(cortex_groups::get_group_task_projection))
        .route("/api/operations/summary", get(cortex_groups::get_personal_operations_summary))
        .route("/api/authority/scopes", get(cortex_groups::list_authority_scopes))
        .route("/api/authority/scopes", post(cortex_groups::create_authority_scope))
        .route("/api/authority/scopes/{scope_id}", patch(cortex_groups::update_authority_scope))
        .route("/api/authority/delegations", get(cortex_groups::list_authority_delegations))
        .route("/api/authority/delegations", post(cortex_groups::delegate_authority))
        .route("/api/authority/delegations/{delegation_id}", delete(cortex_groups::revoke_authority_delegation))
        .route("/api/groups/{group_id}/operations/summary", get(cortex_groups::get_group_operations_summary))
        .route("/api/groups/{group_id}/operations/graph", get(cortex_groups::get_group_operations_graph))
        .route("/api/groups/{group_id}/approvals", get(cortex_groups::list_group_approval_requests))
        .route("/api/groups/{group_id}/approvals", post(cortex_groups::create_group_approval_request))
        .route("/api/groups/{group_id}/approvals/{request_id}", patch(cortex_groups::resolve_group_approval_request))
        .route("/api/billing/status", get(billing::get_billing_status))
        .route("/api/billing/checkout", post(billing::create_checkout))
        .route("/api/billing/portal", post(billing::create_portal))
        .route("/api/billing/referral/validate", post(billing::validate_referral))
        .route("/api/billing/history", get(billing::get_billing_history))
        .route("/api/billing/usage", get(billing::get_billing_usage))
        .route("/api/stripe/webhook", post(billing::stripe_webhook))
        .route("/api/usage", get(usage_api::get_usage))
        .route("/api/usage/daily", get(usage_api::get_daily_usage))
        .route("/api/ws", get(ws::ws_handler))
        .route("/api/mc", get(mission_control::mc_handler))
        .route("/api/mc/snapshot", get(mission_control::mc_snapshot))
        .merge(admin_routes)
        .merge(rate_limited)
        .merge(api_not_found_router())
        // 12 MB: mock media PUT may carry image bytes when R2 is not configured.
        .layer(DefaultBodyLimit::max(12 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), soma_fence::headers_middleware))
        .layer(cors_layer())
        .layer(middleware::from_fn(request_id_middleware))
        .with_state(state)
        .fallback_service(static_service)
}
