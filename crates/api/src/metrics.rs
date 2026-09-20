//! Production metrics for Cortex BYOS.
//!
//! Exposes a global Prometheus registry with metric families for:
//!   - HTTP requests (count, in-flight, latency histogram, error count)
//!   - Container lifecycle operations (create/start/stop/remove/exec) with
//!     outcome and duration
//!   - Container population gauges (total / running / stopped)
//!   - Subsystem health gauges (db, docker)
//!
//! The registry is process-global and lazily initialized. All metric handles
//! are pre-registered so that the `/metrics` endpoint always reports a stable
//! set of series (avoids the "metric appears only after first event" problem
//! that breaks Prometheus rate() queries).
//!
//! Designed for high cardinality control: route labels are normalized to the
//! matched router template, never the raw path, so per-user/per-id paths do
//! not explode the time-series count.

use once_cell::sync::Lazy;
use prometheus::{
    register_histogram_vec_with_registry, register_int_counter_vec_with_registry,
    register_int_counter_with_registry, register_int_gauge_vec_with_registry,
    register_int_gauge_with_registry, Encoder, HistogramVec, IntCounter, IntCounterVec, IntGauge,
    IntGaugeVec, Registry, TextEncoder,
};

/// Latency buckets (seconds) tuned for an HTTP API that ranges from sub-ms
/// cached reads to multi-second container operations.
const HTTP_LATENCY_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0,
];

/// Latency buckets (seconds) for container operations, which are slower and
/// span container start (sub-second) through exec timeouts (tens of seconds).
const CONTAINER_OP_BUCKETS: &[f64] = &[
    0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 20.0, 30.0, 60.0, 120.0,
];

/// All Cortex metrics, grouped behind a single struct so call sites read
/// clearly (`metrics().http_requests_total`).
pub struct Metrics {
    pub registry: Registry,

    // ── HTTP ──────────────────────────────────────────────────────────────
    /// Total HTTP requests, labeled by method, route template, status class.
    pub http_requests_total: IntCounterVec,
    /// Requests that resulted in a 5xx (server error).
    pub http_errors_total: IntCounterVec,
    /// In-flight HTTP requests (gauge), used to detect saturation.
    pub http_in_flight: IntGauge,
    /// Request latency histogram, labeled by method and route template.
    pub http_request_duration_seconds: HistogramVec,

    // ── Containers ──────────────────────────────────────────────────────────
    /// Container lifecycle operations, labeled by op and outcome (ok/error).
    pub container_ops_total: IntCounterVec,
    /// Container operation duration, labeled by op.
    pub container_op_duration_seconds: HistogramVec,
    /// Currently tracked containers, labeled by status (running/stopped).
    pub containers: IntGaugeVec,
    /// Containers reaped for idleness (cumulative).
    pub containers_reaped_total: IntCounter,
    /// Exec invocations inside containers that returned a non-zero exit code.
    pub container_exec_failures_total: IntCounter,

    // ── Verification queue ──────────────────────────────────────────────────
    /// Verification jobs by state. The number an operator watches to know
    /// whether deliveries are being graded or merely accumulating.
    pub verification_jobs: IntGaugeVec,
    /// Jobs handed back for a bounded retry, cumulative. A rising count means
    /// the container runtime is flapping.
    pub verification_retries_total: IntCounter,
    /// Time from claim to sealed verdict, labeled by outcome.
    pub verification_time_to_verdict_seconds: HistogramVec,

    // ── Subsystems ──────────────────────────────────────────────────────────
    /// 1 if the subsystem is healthy, 0 otherwise. Labeled by subsystem name.
    pub subsystem_up: IntGaugeVec,
}

/// Container lifecycle operation names. Using a closed enum keeps the `op`
/// label cardinality fixed and prevents typos in call sites.
#[derive(Clone, Copy, Debug)]
pub enum ContainerOp {
    Create,
    Start,
    Stop,
    Remove,
    Exec,
    Inspect,
    LoginExec,
}

impl ContainerOp {
    pub fn as_str(self) -> &'static str {
        match self {
            ContainerOp::Create => "create",
            ContainerOp::Start => "start",
            ContainerOp::Stop => "stop",
            ContainerOp::Remove => "remove",
            ContainerOp::Exec => "exec",
            ContainerOp::Inspect => "inspect",
            ContainerOp::LoginExec => "login_exec",
        }
    }
}

impl Metrics {
    fn new() -> Self {
        let registry = Registry::new();

        let http_requests_total = register_int_counter_vec_with_registry!(
            "cortex_http_requests_total",
            "Total HTTP requests processed",
            &["method", "route", "status"],
            registry
        )
        .expect("register http_requests_total");

        let http_errors_total = register_int_counter_vec_with_registry!(
            "cortex_http_errors_total",
            "Total HTTP requests that resulted in a 5xx response",
            &["method", "route"],
            registry
        )
        .expect("register http_errors_total");

        let http_in_flight = register_int_gauge_with_registry!(
            "cortex_http_in_flight_requests",
            "Number of HTTP requests currently being served",
            registry
        )
        .expect("register http_in_flight");

        let http_request_duration_seconds = register_histogram_vec_with_registry!(
            prometheus::HistogramOpts::new(
                "cortex_http_request_duration_seconds",
                "HTTP request latency in seconds"
            )
            .buckets(HTTP_LATENCY_BUCKETS.to_vec()),
            &["method", "route"],
            registry
        )
        .expect("register http_request_duration_seconds");

        let container_ops_total = register_int_counter_vec_with_registry!(
            "cortex_container_ops_total",
            "Total container lifecycle operations by outcome",
            &["op", "outcome"],
            registry
        )
        .expect("register container_ops_total");

        let container_op_duration_seconds = register_histogram_vec_with_registry!(
            prometheus::HistogramOpts::new(
                "cortex_container_op_duration_seconds",
                "Container operation latency in seconds"
            )
            .buckets(CONTAINER_OP_BUCKETS.to_vec()),
            &["op"],
            registry
        )
        .expect("register container_op_duration_seconds");

        let containers = register_int_gauge_vec_with_registry!(
            "cortex_containers",
            "Number of tracked BYOS containers by status",
            &["status"],
            registry
        )
        .expect("register containers");

        let containers_reaped_total = register_int_counter_with_registry!(
            "cortex_containers_reaped_total",
            "Total BYOS containers stopped by the idle reaper",
            registry
        )
        .expect("register containers_reaped_total");

        let container_exec_failures_total = register_int_counter_with_registry!(
            "cortex_container_exec_failures_total",
            "Total container exec invocations that returned a non-zero exit code",
            registry
        )
        .expect("register container_exec_failures_total");

        let subsystem_up = register_int_gauge_vec_with_registry!(
            "cortex_subsystem_up",
            "Subsystem health: 1 = healthy, 0 = unhealthy",
            &["subsystem"],
            registry
        )
        .expect("register subsystem_up");

        let verification_jobs = register_int_gauge_vec_with_registry!(
            "cortex_verification_jobs",
            "Verification jobs by queue state",
            &["state"],
            registry
        )
        .expect("register verification_jobs");

        let verification_retries_total = register_int_counter_with_registry!(
            "cortex_verification_retries_total",
            "Verification jobs handed back for a bounded retry",
            registry
        )
        .expect("register verification_retries_total");

        let verification_time_to_verdict_seconds = register_histogram_vec_with_registry!(
            "cortex_verification_time_to_verdict_seconds",
            "Time from claiming a verification job to sealing its verdict",
            &["outcome"],
            CONTAINER_OP_BUCKETS.to_vec(),
            registry
        )
        .expect("register verification_time_to_verdict_seconds");

        Self {
            registry,
            http_requests_total,
            http_errors_total,
            http_in_flight,
            http_request_duration_seconds,
            container_ops_total,
            container_op_duration_seconds,
            containers,
            containers_reaped_total,
            container_exec_failures_total,
            verification_jobs,
            verification_retries_total,
            verification_time_to_verdict_seconds,
            subsystem_up,
        }
    }
}

static METRICS: Lazy<Metrics> = Lazy::new(Metrics::new);

/// Access the process-global metrics registry.
pub fn metrics() -> &'static Metrics {
    &METRICS
}

/// Publish the verification queue depth.
///
/// States that are currently empty are published as zero rather than omitted:
/// a gauge that disappears when a queue drains looks identical to a scrape
/// that failed.
pub fn set_verification_queue_depth(depth: &[(String, i64)]) {
    let m = metrics();
    for state in [
        "queued",
        "claimed",
        "retry_wait",
        "succeeded",
        "failed",
        "inconclusive",
        "dead",
    ] {
        let count = depth
            .iter()
            .find(|(name, _)| name == state)
            .map(|(_, count)| *count)
            .unwrap_or(0);
        m.verification_jobs.with_label_values(&[state]).set(count);
    }
}

/// A verification job was handed back for a bounded retry.
pub fn record_verification_retry() {
    metrics().verification_retries_total.inc();
}

/// A verification job reached a terminal state.
pub fn record_time_to_verdict(seconds: f64, outcome: &str) {
    metrics()
        .verification_time_to_verdict_seconds
        .with_label_values(&[outcome])
        .observe(seconds);
}

/// Record one container lifecycle operation. `started` is the instant the
/// operation began; duration is computed from it. `outcome` should be `"ok"`
/// or `"error"`.
pub fn record_container_op(op: ContainerOp, started: std::time::Instant, outcome: &str) {
    let m = metrics();
    let elapsed = started.elapsed().as_secs_f64();
    m.container_ops_total
        .with_label_values(&[op.as_str(), outcome])
        .inc();
    m.container_op_duration_seconds
        .with_label_values(&[op.as_str()])
        .observe(elapsed);
}

/// Update the container population gauges. Call after listing containers from
/// the database (e.g. in the idle reaper or admin stats).
pub fn set_container_population(running: i64, stopped: i64) {
    let m = metrics();
    m.containers.with_label_values(&["running"]).set(running);
    m.containers.with_label_values(&["stopped"]).set(stopped);
}

/// Set a subsystem health gauge (1 = up, 0 = down).
pub fn set_subsystem_up(subsystem: &str, up: bool) {
    metrics()
        .subsystem_up
        .with_label_values(&[subsystem])
        .set(if up { 1 } else { 0 });
}

/// Map an HTTP status code to its class label (`2xx`, `4xx`, ...). Keeps the
/// `status` label cardinality bounded at 5 values instead of ~60 codes.
pub fn status_class(status: u16) -> &'static str {
    match status {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        _ => "5xx",
    }
}

/// Render the registry in the Prometheus text exposition format.
pub fn render() -> Result<String, String> {
    let m = metrics();
    let encoder = TextEncoder::new();
    let mut buf = Vec::with_capacity(8 * 1024);
    encoder
        .encode(&m.registry.gather(), &mut buf)
        .map_err(|e| format!("failed to encode metrics: {e}"))?;
    String::from_utf8(buf).map_err(|e| format!("metrics not valid utf-8: {e}"))
}
