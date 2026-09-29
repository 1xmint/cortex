//! A run pauses when its owner's credits run out, and resumes on top-up.
//!
//! There are no holds anywhere in this flow. Credits are charged as each
//! attempt settles, at exact cost. What is under test is the one moment that
//! matters: the provider gateway's per-call reservation is checked, live and
//! inside its own transaction, against what the owner can still pay for (their
//! credits less the uncharged exposure of their run attempts). When it does not
//! fit, the call is refused with "insufficient credits", the attempt ends (the
//! calls it already made are charged as settled), the step is not failed and
//! not retried, the run waits in `awaiting_top_up`, and nothing is dispatched
//! for it until the owner tops up and `POST /api/runs/{id}/resume` (or the
//! top-up webhook) lets the scheduler pick the step up again. Any other refusal
//! -- the operator's authorization cap included -- does not pause.
//!
//! Everything runs against the real router, scheduler and gateway (stub
//! transport: nothing leaves the machine and no money moves). A worker takes
//! the real `ExecuteStep` frame off the websocket and uses the capability in
//! it to call the gateway the way the CLI inside the sandbox would.

use std::sync::{Arc, Once};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tower::ServiceExt;

use cortex_api::provider_gateway::GatewayCapability;
use cortex_api::scheduler;
use cortex_api::state::AppState;
use cortex_core::protocol::{
    BrainMessage, ProviderClaim, ProviderGatewayAccess, WorkerMessage, PROTOCOL_VERSION,
};
use cortex_core::provider::ProviderId;

/// The only user without a bearer token: what an unauthenticated local
/// deployment resolves every request to.
const USER: &str = "local";

/// The operator's own ceiling on one authorization. Far above any balance the
/// tests give the user, so the balance is always what binds.
const OPERATOR_MAX_MICRO_USD: &str = "2000000000";

/// Environment is process-global and every test in this binary shares it, so
/// it is set once, before the first `AppState::new`, to the same values for
/// everyone.
fn gateway_env() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        std::env::set_var("CORTEX_PROVIDER_GATEWAY_MODE", "stub");
        std::env::set_var(
            "CORTEX_PROVIDER_GATEWAY_SIGNING_KEY",
            "pause-topup-test-signing-key-32-bytes-long",
        );
        std::env::set_var(
            "CORTEX_PROVIDER_GATEWAY_MAX_MICRO_USD",
            OPERATOR_MAX_MICRO_USD,
        );
        std::env::set_var("CORTEX_PROVIDER_GATEWAY_FUNDED_MICRO_USD", "1000000000000");
        std::env::set_var("CORTEX_BILLING_ENFORCE", "true");
    });
}

fn real_repository() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = tmp.path();

    std::fs::create_dir_all(root.join(".cortex")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"subject\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    )
    .unwrap();

    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Cortex Test"],
        vec!["add", "-A"],
        vec!["commit", "--quiet", "-m", "initial"],
    ] {
        let _ = std::process::Command::new("git")
            .args(&args)
            .current_dir(root)
            .output();
    }

    tmp
}

async fn test_app(workspace: &std::path::Path) -> (axum::Router, Arc<AppState>) {
    gateway_env();
    let ledger_path = workspace.join(".cortex/ledger.jsonl");
    let state = AppState::new(ledger_path, workspace.to_path_buf(), None).await;

    let scheduler_tx = scheduler::spawn_scheduler(state.clone());
    state.set_scheduler_tx(scheduler_tx).await;

    let app = cortex_api::build_cortex_router(state.clone());
    (app, state)
}

async fn serve_app(app: axum::Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    format!("http://{addr}")
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let resp = app.clone().oneshot(request).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&bytes).into()));
    (status, json)
}

async fn create_run(app: &axum::Router, goal: &str) -> String {
    let (status, json) = send(
        app,
        Request::builder()
            .method("POST")
            .uri("/api/runs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::json!({ "goal": goal }).to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create run failed: {json}");
    json["run_id"].as_str().unwrap().to_string()
}

async fn resume(app: &axum::Router, run_id: &str) -> (StatusCode, serde_json::Value) {
    send(
        app,
        Request::builder()
            .method("POST")
            .uri(format!("/api/runs/{run_id}/resume"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn cancel(app: &axum::Router, run_id: &str) -> (StatusCode, serde_json::Value) {
    send(
        app,
        Request::builder()
            .method("POST")
            .uri(format!("/api/runs/{run_id}/cancel"))
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await
}

async fn get_run(app: &axum::Router, run_id: &str) -> serde_json::Value {
    let (status, json) = send(
        app,
        Request::builder()
            .method("GET")
            .uri(format!("/api/runs/{run_id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "get run failed: {json}");
    json
}

type WorkerSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    WsMessage,
>;
type WorkerStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

fn issue_worker_key(state: &AppState) -> String {
    let db = state.db.as_ref().expect("test app has a database");
    let key = cortex_api::worker_key::generate_worker_key();
    db.create_worker_key(
        &format!("wk_{}", &key.hash[..12]),
        &key.hash,
        &key.display_prefix,
        "local",
        cortex_api::worker_key::DEFAULT_WORKER_SCOPE,
        None,
    )
    .expect("worker key is issued");
    key.secret
}

/// Connect and register a worker. The sink is returned so the socket stays
/// open; dropping it would deregister the worker.
async fn connect_worker(base_url: &str, token: String) -> (WorkerSink, WorkerStream) {
    let ws_url = format!("{}/api/ws", base_url.replace("http://", "ws://"));
    let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("worker connects");
    let (mut sink, mut stream) = ws_stream.split();

    let _ = timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("welcome arrives")
        .expect("stream open")
        .expect("ws ok");

    let register = WorkerMessage::Register {
        token,
        protocol_version: PROTOCOL_VERSION,
        providers: vec![ProviderClaim {
            provider: ProviderId::Claude,
            cli_version: None,
        }],
        workspace_dir: "/tmp/test-workspace".to_string(),
        repos: vec![],
    };
    sink.send(WsMessage::Text(
        serde_json::to_string(&register).unwrap().into(),
    ))
    .await
    .expect("register sent");
    tokio::time::sleep(Duration::from_millis(200)).await;

    (sink, stream)
}

async fn next_brain_message(stream: &mut WorkerStream, wait: Duration) -> Option<BrainMessage> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match timeout(remaining, stream.next()).await {
            Ok(Some(Ok(WsMessage::Text(text)))) => {
                if let Ok(brain) = serde_json::from_str::<BrainMessage>(&text) {
                    return Some(brain);
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => panic!("ws error: {e}"),
            Ok(None) => panic!("worker stream closed"),
            Err(_) => return None,
        }
    }
}

/// The next `ExecuteStep` within `wait`, skipping every other frame.
async fn execute_step_within(
    stream: &mut WorkerStream,
    wait: Duration,
) -> Option<(String, Option<ProviderGatewayAccess>)> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let BrainMessage::ExecuteStep {
            step_id,
            provider_gateway,
            ..
        } = next_brain_message(stream, remaining).await?
        {
            return Some((step_id, provider_gateway));
        }
    }
}

async fn first_execute_step(stream: &mut WorkerStream) -> (String, ProviderGatewayAccess) {
    let (step_id, access) = execute_step_within(stream, Duration::from_secs(45))
        .await
        .expect("no ExecuteStep within 45s");
    (
        step_id,
        access.expect("the dispatched step carries a gateway capability"),
    )
}

/// Every `ExecuteStep` that arrives within `wait`.
async fn execute_steps_within(stream: &mut WorkerStream, wait: Duration) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + wait;
    let mut seen = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return seen;
        }
        match execute_step_within(stream, remaining).await {
            Some((step_id, _)) => seen.push(step_id),
            None => return seen,
        }
    }
}

/// One model call through the gateway, the way the CLI inside the sandbox
/// makes it: the capability from the frame as the bearer, one request key per
/// call, `max_tokens` fixed at the largest the model may answer with.
async fn gateway_call(
    app: &axum::Router,
    access: &ProviderGatewayAccess,
    request_key: &str,
) -> (StatusCode, serde_json::Value) {
    send(
        app,
        Request::builder()
            .method("POST")
            .uri("/internal/provider/v1/messages")
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!("Bearer {}", access.bearer.expose()),
            )
            .header("x-cortex-request-key", request_key)
            .body(Body::from(
                serde_json::json!({
                    "model": access.model,
                    "max_tokens": 32_000,
                    "messages": [{"role": "user", "content": "stub only"}],
                    "stream": true,
                    "tools": []
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await
}

async fn wait_for_run_status(state: &AppState, run_id: &str, want: &str) {
    let db = state.db.as_ref().unwrap();
    for _ in 0..100 {
        if db.get_run_status(run_id).as_deref() == Some(want) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "run {run_id} never reached {want}; it is {:?}",
        db.get_run_status(run_id)
    );
}

fn raw_count(workspace: &std::path::Path, sql: &str) -> i64 {
    let path = cortex_api::state::cortex_db_path(workspace);
    let conn = rusqlite::Connection::open(path).expect("open the test database");
    conn.query_row(sql, [], |row| row.get(0)).expect("count")
}

/// Bring a run to `awaiting_top_up`: a user with one credit, one dispatched
/// step, and one call whose reservation cannot fit in that balance.
struct Paused {
    _repo: tempfile::TempDir,
    app: axum::Router,
    state: Arc<AppState>,
    base_url: String,
    _sink: WorkerSink,
    stream: WorkerStream,
    run_id: String,
    step_id: String,
}

async fn paused_run() -> Paused {
    let repo = real_repository();
    let (app, state) = test_app(repo.path()).await;
    let base_url = serve_app(app.clone()).await;
    state
        .db
        .as_ref()
        .unwrap()
        .add_pack_credits(USER, 1)
        .expect("seed one credit");

    let (sink, mut stream) = connect_worker(&base_url, issue_worker_key(&state)).await;
    let run_id = create_run(&app, "make the arithmetic in src/lib.rs correct").await;
    let (step_id, access) = first_execute_step(&mut stream).await;

    let (status, body) = gateway_call(&app, &access, "call-1").await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "one credit cannot cover a 32k-token reservation: {body}"
    );
    wait_for_run_status(&state, &run_id, "awaiting_top_up").await;

    Paused {
        _repo: repo,
        app,
        state,
        base_url,
        _sink: sink,
        stream,
        run_id,
        step_id,
    }
}

/// The capability an `ExecuteStep` frame carries, as the claims the database
/// checks a reservation against.
fn claims_of(access: &ProviderGatewayAccess) -> GatewayCapability {
    GatewayCapability::new(
        access.authorization_id.clone(),
        USER,
        access.run_id.clone(),
        access.attempt_id.clone(),
        access.provider.clone(),
        access.model.clone(),
        access.expires_at_ms,
    )
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Take the user's balance to exactly zero credits (they had `credits`).
fn drain_credits(state: &AppState, credits: i64) {
    state
        .db
        .as_ref()
        .unwrap()
        .add_pack_credits(USER, -credits)
        .expect("drain credits");
}

#[tokio::test(flavor = "multi_thread")]
async fn insufficient_credits_pauses_the_run_and_dispatches_nothing_more() {
    let mut paused = paused_run().await;
    let db = paused.state.db.as_ref().unwrap();

    // The step was not failed and not retried: it is back in the queue.
    assert_eq!(
        db.get_step_status(&paused.step_id).as_deref(),
        Some("orphaned"),
        "the paused step waits to be dispatched again; it is not failed"
    );

    // Nudge the scheduler as hard as an ordinary event can, then wait: a
    // paused run must not produce another ExecuteStep.
    paused
        .state
        .emit_scheduler_event(cortex_engine::captain::SchedulerEvent::Reconcile)
        .await;
    let further = execute_steps_within(&mut paused.stream, Duration::from_secs(4)).await;
    assert!(
        further.is_empty(),
        "a run awaiting a top-up dispatched {further:?}"
    );
    assert_eq!(
        db.get_run_status(&paused.run_id).as_deref(),
        Some("awaiting_top_up")
    );

    // The run's JSON says so, and records what it has spent (nothing: the one
    // call was refused before it reserved anything).
    let run = get_run(&paused.app, &paused.run_id).await;
    assert_eq!(run["status"], "awaiting_top_up");
    assert_eq!(run["spent_credits"], 0);

    // The attempt ended as out-of-credits, exactly once.
    assert_eq!(
        raw_count(
            paused._repo.path(),
            "SELECT COUNT(*) FROM attempt_endings WHERE cause = 'out_of_credits'"
        ),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn two_parallel_reservations_cannot_both_spend_the_same_credits() {
    let repo = real_repository();
    let (app, state) = test_app(repo.path()).await;
    let base_url = serve_app(app.clone()).await;
    // 5 credits = 500_000 micro-USD payable.
    state
        .db
        .as_ref()
        .unwrap()
        .add_pack_credits(USER, 5)
        .expect("seed five credits");

    let (_sink, mut stream) = connect_worker(&base_url, issue_worker_key(&state)).await;
    let run_id = create_run(&app, "make the arithmetic in src/lib.rs correct").await;
    let (_step_id, access) = first_execute_step(&mut stream).await;
    let claims = claims_of(&access);

    // Two calls of 400_000 each, at the same instant. Each fits alone; both
    // together do not. The check and the reservation share one transaction,
    // so exactly one wins.
    let db = state.db.as_ref().unwrap();
    let barrier = std::sync::Barrier::new(2);
    let (left, right) = std::thread::scope(|scope| {
        let left = scope.spawn(|| {
            barrier.wait();
            db.reserve_provider_request(&claims, "parallel-left", "sha256:left", 400_000, now_ms())
        });
        let right = scope.spawn(|| {
            barrier.wait();
            db.reserve_provider_request(
                &claims,
                "parallel-right",
                "sha256:right",
                400_000,
                now_ms(),
            )
        });
        (left.join().unwrap(), right.join().unwrap())
    });
    let refused: Vec<String> = [left, right]
        .into_iter()
        .filter_map(|result| result.err())
        .collect();
    assert_eq!(refused.len(), 1, "exactly one must be refused: {refused:?}");
    assert!(
        refused[0].starts_with("insufficient credits:"),
        "refused for the distinct reason: {}",
        refused[0]
    );

    // The next call through the gateway does not fit in the 100_000 left, so
    // the run pauses.
    let (status, body) = gateway_call(&app, &access, "call-after").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("insufficient credits"), "{body}");
    wait_for_run_status(&state, &run_id, "awaiting_top_up").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_top_up_mid_step_means_no_pause() {
    let repo = real_repository();
    let (app, state) = test_app(repo.path()).await;
    let base_url = serve_app(app.clone()).await;
    let db = state.db.as_ref().unwrap();
    db.add_pack_credits(USER, 1).expect("seed one credit");

    let (_sink, mut stream) = connect_worker(&base_url, issue_worker_key(&state)).await;
    let run_id = create_run(&app, "make the arithmetic in src/lib.rs correct").await;
    let (_step_id, access) = first_execute_step(&mut stream).await;

    // The owner tops up while the step is running, before its call is
    // refused. Nothing was decided at dispatch, so nothing is stranded.
    db.add_pack_credits(USER, 100_000_000).expect("top up");

    let (status, body) = gateway_call(&app, &access, "call-1").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(db.get_run_status(&run_id).as_deref(), Some("running"));
    assert_eq!(
        raw_count(
            repo.path(),
            "SELECT COUNT(*) FROM attempt_endings WHERE cause = 'out_of_credits'"
        ),
        0
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_with_no_credits_is_402_need_one_credit() {
    let paused = paused_run().await;
    drain_credits(&paused.state, 1);

    let (status, body) = resume(&paused.app, &paused.run_id).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["need_credits"], 1);
    assert_eq!(body["available_credits"], 0);

    // Still paused, still nothing charged.
    assert_eq!(
        paused
            .state
            .db
            .as_ref()
            .unwrap()
            .get_run_status(&paused.run_id)
            .as_deref(),
        Some("awaiting_top_up")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_with_one_credit_is_200_and_dispatches_the_paused_step_exactly_once() {
    let mut paused = paused_run().await;

    // One credit is enough to resume; the next refusal, if any, pauses again.
    let (status, body) = resume(&paused.app, &paused.run_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "running");

    let dispatched = execute_steps_within(&mut paused.stream, Duration::from_secs(20)).await;
    assert_eq!(
        dispatched,
        vec![paused.step_id.clone()],
        "resume must re-dispatch the paused step, once"
    );
    assert_eq!(
        paused
            .state
            .db
            .as_ref()
            .unwrap()
            .get_run_status(&paused.run_id)
            .as_deref(),
        Some("running")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_webhook_top_up_resumes_a_paused_run() {
    let mut paused = paused_run().await;
    drain_credits(&paused.state, 1);
    let db = paused.state.db.as_ref().unwrap();

    // A paid $10 checkout: 100 credits.
    let session = serde_json::json!({
        "id": "cs_test_topup_1",
        "metadata": {"clerk_user_id": USER},
        "payment_status": "paid",
        "currency": "usd",
        "amount_subtotal": 1000,
    });
    let resumed =
        cortex_api::billing::grant_credit_topup(db, &session, "evt_topup_1").expect("grant");
    assert_eq!(
        resumed,
        vec![paused.run_id.clone()],
        "the grant resumes the paused run"
    );
    assert_eq!(db.get_run_status(&paused.run_id).as_deref(), Some("running"));

    // The webhook handler passes each resumed run to the scheduler.
    for run_id in resumed {
        paused
            .state
            .emit_scheduler_event(cortex_engine::captain::SchedulerEvent::RunResumed { run_id })
            .await;
    }
    let dispatched = execute_steps_within(&mut paused.stream, Duration::from_secs(20)).await;
    assert_eq!(dispatched, vec![paused.step_id.clone()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_restart_leaves_a_paused_run_paused() {
    let mut paused = paused_run().await;
    let workspace = paused._repo.path().to_path_buf();

    // A second server on the same database, as after a restart: its scheduler
    // runs startup recovery over every run that is planning or running.
    let (app2, state2) = test_app(&workspace).await;
    let base_url2 = serve_app(app2.clone()).await;
    let (_sink2, mut stream2) = connect_worker(&base_url2, issue_worker_key(&state2)).await;

    let dispatched = execute_steps_within(&mut stream2, Duration::from_secs(4)).await;
    assert!(
        dispatched.is_empty(),
        "restart recovery dispatched a paused run's step: {dispatched:?}"
    );
    assert_eq!(
        state2
            .db
            .as_ref()
            .unwrap()
            .get_run_status(&paused.run_id)
            .as_deref(),
        Some("awaiting_top_up")
    );
    assert!(
        execute_steps_within(&mut paused.stream, Duration::from_millis(500))
            .await
            .is_empty()
    );
    let _ = &paused.base_url;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_a_paused_run_charges_nothing_more() {
    let paused = paused_run().await;
    let workspace = paused._repo.path();
    let ledger_rows = "SELECT COUNT(*) FROM credit_transactions";
    let endings = "SELECT COUNT(*) FROM attempt_endings";
    let before = (
        raw_count(workspace, ledger_rows),
        raw_count(workspace, endings),
    );

    let (status, body) = cancel(&paused.app, &paused.run_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        paused
            .state
            .db
            .as_ref()
            .unwrap()
            .get_run_status(&paused.run_id)
            .as_deref(),
        Some("cancelled")
    );

    // Give the scheduler's settle tick every chance to charge something.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let after = (
        raw_count(workspace, ledger_rows),
        raw_count(workspace, endings),
    );
    assert_eq!(
        before, after,
        "cancelling a paused run added ledger or ending rows"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_operator_cap_refusal_does_not_pause() {
    let repo = real_repository();
    let (app, state) = test_app(repo.path()).await;
    let base_url = serve_app(app.clone()).await;
    let db = state.db.as_ref().unwrap();
    // Far more credit than any reservation: the balance is never the problem.
    db.add_pack_credits(USER, 100_000_000)
        .expect("seed credits");

    let (_sink, mut stream) = connect_worker(&base_url, issue_worker_key(&state)).await;
    let run_id = create_run(&app, "make the arithmetic in src/lib.rs correct").await;
    let (_step_id, access) = first_execute_step(&mut stream).await;

    // Use up the operator's authorization cap, leaving less than one call.
    let cap: i64 = OPERATOR_MAX_MICRO_USD.parse().unwrap();
    db.reserve_provider_request(
        &claims_of(&access),
        "use-up-the-cap",
        "sha256:cap",
        cap - 1,
        now_ms(),
    )
    .expect("the reservation that nearly exhausts the cap");

    let (status, body) = gateway_call(&app, &access, "call-1").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body.to_string().contains("authorization exhausted"),
        "{body}"
    );
    assert!(!body.to_string().contains("insufficient credits"), "{body}");

    // Give the pause path every chance to (wrongly) run.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        db.get_run_status(&run_id).as_deref(),
        Some("running"),
        "the operator's cap is not the owner's balance; it must not pause"
    );
    assert_eq!(
        raw_count(
            repo.path(),
            "SELECT COUNT(*) FROM attempt_endings WHERE cause = 'out_of_credits'"
        ),
        0
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_that_is_not_about_the_balance_does_not_pause() {
    let repo = real_repository();
    let (app, state) = test_app(repo.path()).await;
    let base_url = serve_app(app.clone()).await;
    // Plenty of credit: the authorization is as large as the operator allows,
    // so nothing here is a balance problem.
    state
        .db
        .as_ref()
        .unwrap()
        .add_pack_credits(USER, 100_000_000)
        .expect("seed credits");

    let (_sink, mut stream) = connect_worker(&base_url, issue_worker_key(&state)).await;
    let run_id = create_run(&app, "make the arithmetic in src/lib.rs correct").await;
    let (_step_id, access) = first_execute_step(&mut stream).await;

    let (status, body) = gateway_call(&app, &access, "call-1").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The same request key again is refused as a replay: a reservation error,
    // but neither "authorization exhausted" nor "insufficient credits".
    let (status, body) = gateway_call(&app, &access, "call-1").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        !body.to_string().contains("authorization exhausted")
            && !body.to_string().contains("insufficient credits"),
        "the replay must be refused for its own reason: {body}"
    );

    // A request the capability does not cover is refused outright.
    let (status, _) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/internal/provider/v1/messages")
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!("Bearer {}", access.bearer.expose()),
            )
            .header("x-cortex-request-key", "call-2")
            .body(Body::from(
                serde_json::json!({
                    "model": "some-other-model",
                    "max_tokens": 100,
                    "messages": [{"role": "user", "content": "x"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let db = state.db.as_ref().unwrap();
    assert_eq!(
        db.get_run_status(&run_id).as_deref(),
        Some("running"),
        "only running out of credits pauses a run"
    );
    assert_eq!(
        raw_count(
            repo.path(),
            "SELECT COUNT(*) FROM attempt_endings WHERE cause = 'out_of_credits'"
        ),
        0
    );
}
