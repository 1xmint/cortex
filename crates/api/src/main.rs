use std::net::SocketAddr;

use tracing_subscriber::EnvFilter;

use cortex_api::scheduler;
use cortex_api::state::AppState;

/// Initialize the OpenTelemetry OTLP tracing pipeline.
/// Returns the tracer provider so it can be shut down on exit.
#[cfg(feature = "otel")]
fn init_otel(
    endpoint: &str,
) -> Result<opentelemetry_sdk::trace::SdkTracerProvider, Box<dyn std::error::Error>> {
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use opentelemetry_sdk::Resource;

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;

    let resource = Resource::builder()
        .with_attributes([
            KeyValue::new("service.name", env!("CARGO_PKG_NAME")),
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
        ])
        .build();

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    opentelemetry::global::set_tracer_provider(provider.clone());
    Ok(provider)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install SIGINT handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received SIGINT"),
        _ = terminate => tracing::info!("received SIGTERM"),
    }
}

#[tokio::main]
async fn main() {
    // Sentry must be initialized before the tracing subscriber so the tracing
    // layer can attach to it. The guard must live for the entire duration of
    // main; dropping it flushes pending events.
    // ── Sentry (optional feature) ────────────────────────────────────────────
    // Initialize before the tracing subscriber so the sentry tracing layer
    // can be wired in. The guard must live for the entire duration of main.
    #[cfg(feature = "sentry-tracking")]
    let _sentry_guard = if let Ok(dsn) = std::env::var("SENTRY_DSN") {
        // sentry 0.49 made `ClientOptions` `#[non_exhaustive]`, so it can no
        // longer be built with struct-literal syntax, and `traces_sample_rate`
        // is no longer a field: it is a builder method that sets the
        // `traces_sampling_strategy` field.
        let options = sentry::ClientOptions::default()
            .release(env!("CARGO_PKG_VERSION"))
            .traces_sample_rate(0.1);
        let guard = sentry::init((dsn, options));
        Some(guard)
    } else {
        None
    };

    // ── Tracing subscriber ───────────────────────────────────────────────────
    #[cfg(feature = "sentry-tracking")]
    {
        use tracing_subscriber::prelude::*;
        let env_filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".parse().unwrap());
        let sentry_active = std::env::var("SENTRY_DSN").is_ok();
        if sentry_active {
            tracing_subscriber::registry()
                .with(tracing_subscriber::fmt::layer().with_filter(env_filter))
                .with(sentry::integrations::tracing::layer())
                .init();
            eprintln!("Sentry error tracking enabled");
        } else {
            tracing_subscriber::fmt().with_env_filter(env_filter).init();
            eprintln!("SENTRY_DSN not set, error tracking disabled");
        }
    }

    #[cfg(not(feature = "sentry-tracking"))]
    {
        use tracing_subscriber::prelude::*;

        let env_filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".parse().unwrap());
        // Use structured JSON logging when CORTEX_JSON_LOGS=true (production default)
        let use_json = std::env::var("CORTEX_JSON_LOGS")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let fmt_layer = if use_json {
            tracing_subscriber::fmt::layer().json().boxed()
        } else {
            tracing_subscriber::fmt::layer().boxed()
        };

        // ── OpenTelemetry layer (gated on feature + env var) ─────────────────
        #[cfg(feature = "otel")]
        {
            let otel_endpoint = std::env::var("OTEL_ENDPOINT").ok();
            if let Some(ref endpoint) = otel_endpoint {
                match init_otel(endpoint) {
                    Ok(_provider) => {
                        let otel_layer = tracing_opentelemetry::layer();
                        tracing_subscriber::registry()
                            .with(fmt_layer.with_filter(env_filter))
                            .with(otel_layer)
                            .init();
                        eprintln!("OpenTelemetry tracing enabled → {endpoint}");
                    }
                    Err(e) => {
                        tracing_subscriber::registry()
                            .with(fmt_layer.with_filter(env_filter))
                            .init();
                        eprintln!("OpenTelemetry init failed (continuing without OTel): {e}");
                    }
                }
            } else {
                tracing_subscriber::registry()
                    .with(fmt_layer.with_filter(env_filter))
                    .init();
            }
        }

        #[cfg(not(feature = "otel"))]
        {
            tracing_subscriber::registry()
                .with(fmt_layer.with_filter(env_filter))
                .init();
        }

        if std::env::var("SENTRY_DSN").is_ok() {
            tracing::warn!(
                "SENTRY_DSN is set but the sentry-tracking feature was not compiled in; \
                 rebuild with --features sentry-tracking to enable it"
            );
        }
        if std::env::var("OTEL_ENDPOINT").is_ok() {
            #[cfg(not(feature = "otel"))]
            tracing::warn!(
                "OTEL_ENDPOINT is set but the otel feature was not compiled in; \
                 rebuild with --features otel to enable it"
            );
        }
    }

    let ledger_path =
        std::env::var("CORTEX_LEDGER_PATH").unwrap_or_else(|_| ".cortex/ledger.jsonl".to_string());

    if let Some(parent) = std::path::Path::new(&ledger_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let workspace_dir = std::env::var("CORTEX_WORKSPACE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| ".".into()));

    let auth_config = match cortex_api::clerk::load_heyvera_auth_config() {
        Ok(config) => config,
        Err(msg) => {
            tracing::error!("auth: {msg}");
            eprintln!("auth: {msg}");
            std::process::exit(1);
        }
    };

    match auth_config.mode {
        cortex_api::clerk::HeyVeraAuthMode::Clerk => {
            tracing::info!("auth: Clerk JWT verification enabled");
        }
        cortex_api::clerk::HeyVeraAuthMode::LocalDevelopment => {
            let cause = if std::env::var("CLERK_SECRET_KEY")
                .ok()
                .filter(|s| !s.is_empty())
                .is_none()
            {
                "no CLERK_SECRET_KEY set"
            } else {
                "CORTEX_AUTH_DISABLED"
            };
            tracing::info!(
                "auth: local development mode ({cause}) — all requests treated as user \"local\""
            );
        }
    }

    let state = AppState::new(ledger_path, workspace_dir, auth_config.clerk_secret_key).await;

    // BYOK is split-key now (see `cortex_api::byok`): there is no
    // server-held master key to rewrap under. Warn once, by name only, if
    // an old `CORTEX_BYOK_KEK_*` env var is still set somewhere in the
    // deploy config, so it's obvious that it no longer does anything.
    cortex_api::byok::warn_if_legacy_kek_env_present();

    // Start the scheduler loop
    let scheduler_tx = scheduler::spawn_scheduler(state.clone());
    state.set_scheduler_tx(scheduler_tx).await;

    // Verification. Refuse to start rather than risk two dispatchers on a
    // Mutex<Connection>, then recover whatever the previous process left
    // behind — before this server accepts a single request, because a
    // delivery stranded by the last deploy is the case this exists for.
    start_verification_dispatcher(&state);

    // Start container idle reaper (stops idle containers after 15min inactivity)
    cortex_api::docker::spawn_idle_reaper(state.clone());

    let app = cortex_api::build_cortex_router(state.clone());

    let port: u16 = std::env::var("CORTEX_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3001);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )
    .expect("failed to create socket");
    socket
        .set_reuse_address(true)
        .expect("failed to set SO_REUSEADDR");
    socket
        .set_nonblocking(true)
        .expect("failed to set nonblocking");
    socket.bind(&addr.into()).unwrap_or_else(|e| {
        panic!("failed to bind {addr}: {e} — is another instance running?");
    });
    socket.listen(1024).expect("failed to listen");

    let listener =
        tokio::net::TcpListener::from_std(socket.into()).expect("failed to create tokio listener");
    tracing::info!("cortex server listening on {addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();

    // Signal received — run graceful shutdown logic
    tracing::info!("server stopped accepting connections, running shutdown sequence");
    state.shutdown().await;
    tracing::info!("cortex server exited cleanly");
}

/// Assert single-node mode, reconcile what the previous process left, and start
/// the dispatcher.
///
/// A missing assertion is a hard failure. The SQLite store is a
/// `Mutex<Connection>` that two processes do not share, so a second dispatcher
/// double-dispatches; a warning here would be a warning nobody reads on the day
/// it matters.
fn start_verification_dispatcher(state: &std::sync::Arc<cortex_api::state::AppState>) {
    use cortex_api::verification_dispatcher;

    if let Err(reason) = verification_dispatcher::assert_single_node() {
        tracing::error!("{reason}");
        std::process::exit(1);
    }
    if let Some(db) = state.db.as_ref() {
        verification_dispatcher::reconcile_on_startup(db);
    }
    verification_dispatcher::spawn(state.clone());
}
