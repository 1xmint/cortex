use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use cortex_core::failure::{WorkerFailureKind, WorkerFailureReport};
use cortex_core::protocol::{
    BrainMessage, ProviderClaim, RepoInfo, WorkerMessage, PROTOCOL_VERSION,
};
use cortex_worker::executor::{detect_available_providers, Executor, StepExecution};
// In the library, not here, so the end-to-end test sends the frame this binary
// would have sent rather than one it built itself. See `report`'s module docs.
use cortex_worker::report::worker_event_to_message;
use cortex_worker::stream::WorkerEvent;

/// Map of step_id -> cancel signal sender. Shared between the connection loop
/// (which receives CancelStep) and spawned execution tasks (which register on start).
type CancelMap = Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".parse().unwrap()),
        )
        .init();

    let brain_url =
        env::var("CORTEX_BRAIN_URL").unwrap_or_else(|_| "wss://cortex.heyvera.org/api/ws".into());

    let token = env::var("CORTEX_TOKEN").unwrap_or_default();

    let workspace_dir = env::current_dir()
        .unwrap_or_else(|_| ".".into())
        .to_string_lossy()
        .to_string();

    let available_providers = detect_available_providers();

    tracing::info!("cortex worker v{PROTOCOL_VERSION}");
    tracing::info!("brain: {brain_url}");
    tracing::info!("workspace: {workspace_dir}");
    tracing::info!(
        "providers: {}",
        if available_providers.is_empty() {
            "none".to_string()
        } else {
            available_providers
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        }
    );

    if available_providers.is_empty() {
        tracing::warn!("no CLI providers found — install and authenticate claude or codex first");
    }

    let providers: Vec<ProviderClaim> = available_providers
        .iter()
        .map(|p| ProviderClaim {
            provider: *p,
            cli_version: None,
        })
        .collect();

    loop {
        tracing::info!("connecting to brain...");
        match connect_and_run(&brain_url, &token, &providers, &workspace_dir).await {
            Ok(()) => tracing::info!("disconnected"),
            Err(e) => tracing::error!("connection error: {e}"),
        }
        tracing::info!("reconnecting in 5s...");
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
}

async fn connect_and_run(
    url: &str,
    token: &str,
    providers: &[ProviderClaim],
    workspace_dir: &str,
) -> Result<(), String> {
    let (ws, _) = connect_async(url)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let (mut write, mut read) = ws.split();

    tracing::info!("connected");

    let (out_tx, mut out_rx) = mpsc::channel::<WorkerMessage>(64);

    let register = WorkerMessage::Register {
        token: token.to_string(),
        protocol_version: PROTOCOL_VERSION,
        providers: providers.to_vec(),
        workspace_dir: workspace_dir.to_string(),
        repos: detect_repos(workspace_dir),
    };
    let json = serde_json::to_string(&register).unwrap();
    write
        .send(Message::Text(json.into()))
        .await
        .map_err(|e| format!("send: {e}"))?;
    tracing::info!("registered");

    let ws_dir = workspace_dir.to_string();
    #[allow(unused_assignments)]
    let mut worker_id = String::new();

    let cancel_map: CancelMap = Arc::new(Mutex::new(HashMap::new()));

    loop {
        tokio::select! {
            Some(msg) = out_rx.recv() => {
                let json = serde_json::to_string(&msg).unwrap();
                if write.send(Message::Text(json.into())).await.is_err() {
                    return Err("write failed".into());
                }
            }

            result = read.next() => {
                match result {
                    Some(Ok(Message::Text(text))) => {
                        let brain_msg: BrainMessage = match serde_json::from_str(&text) {
                            Ok(m) => m,
                            Err(e) => {
                                tracing::warn!("bad message: {e}");
                                continue;
                            }
                        };

                        match brain_msg {
                            BrainMessage::Welcome { session_id, worker_id: wid, protocol_version } => {
                                worker_id = wid;
                                tracing::info!("session: {session_id}, worker: {worker_id}, protocol: v{protocol_version}");
                            }
                            BrainMessage::ExecuteStep {
                                run_id, step_id, attempt_id, lease_gen,
                                task, decision, delegation, egress,
                                provider_egress, provider_gateway, context, ..
                            } => {
                                // Without the `soma` feature this worker does
                                // not ask for a per-step delegation, because
                                // an API built the same way never issues one.
                                // Enforcing here would reject every step. The
                                // authenticated WebSocket the step arrived on
                                // is the gate in that configuration.
                                #[cfg(not(feature = "soma"))]
                                let delegation_ok = {
                                    let _ = &delegation;
                                    true
                                };

                                #[cfg(feature = "soma")]
                                let enforce = std::env::var("SOMA_ENFORCE_DELEGATION")
                                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                                    .unwrap_or(true);

                                #[cfg(feature = "soma")]
                                let delegation_ok = if let Some(ref deleg_val) = delegation {
                                    match serde_json::from_value::<soma::delegation::Delegation>(deleg_val.clone()) {
                                        Ok(deleg) => {
                                            let ctx = soma::delegation::InvocationContext {
                                                invoker_did: deleg.issuer_did.clone(),
                                                capability: format!("execute:step:{step_id}"),
                                                ..Default::default()
                                            };
                                            match soma::delegation::verify_delegation(&deleg, &ctx) {
                                                Ok(result) if result.is_valid() => {
                                                    tracing::debug!("step {step_id}: delegation verified");
                                                    true
                                                }
                                                Ok(result) => {
                                                    tracing::warn!("step {step_id}: delegation invalid: {result:?}");
                                                    !enforce
                                                }
                                                Err(e) => {
                                                    tracing::warn!("step {step_id}: delegation verification error: {e}");
                                                    !enforce
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            tracing::warn!("step {step_id}: failed to parse delegation: {e}");
                                            !enforce
                                        }
                                    }
                                } else if enforce {
                                    tracing::warn!("step {step_id}: no delegation provided — rejecting");
                                    false
                                } else {
                                    true
                                };

                                if !delegation_ok {
                                    tracing::error!("step {step_id}: rejected — invalid or missing delegation");
                                    let fail_msg = cortex_core::protocol::WorkerMessage::StepFailed {
                                        message_id: uuid::Uuid::new_v4().to_string(),
                                        step_id: step_id.clone(),
                                        attempt_id: attempt_id.clone(),
                                        lease_gen,
                                        failure: cortex_core::failure::WorkerFailureReport {
                                            kind: cortex_core::failure::WorkerFailureKind::PermissionDenied,
                                            exit_code: None,
                                            stderr_excerpt: Some("step delegation verification failed".into()),
                                            tool: None,
                                        },
                                    };
                                    let _ = out_tx.send(fail_msg).await;
                                    continue;
                                }

                                tracing::info!("step {step_id}: {}", task.objective);
                                let out = out_tx.clone();
                                let dir = PathBuf::from(&ws_dir);
                                let step_exec = StepExecution {
                                    // Straight from the frame. The brain has
                                    // always sent it; the worker used to drop
                                    // it into `..` and then write an empty
                                    // string onto the job (F17).
                                    run_id,
                                    step_id: step_id.clone(),
                                    attempt_id,
                                    lease_gen,
                                    // Straight from the frame. The worker never
                                    // widens this and never re-derives it: it
                                    // cannot see the repository the decision was
                                    // made against, and an absent field defaults
                                    // to `Deny`.
                                    egress,
                                    // Also straight from the frame, and kept
                                    // apart from `egress` for the same reason
                                    // it was sent apart: the two were decided
                                    // from different inputs, and merging them
                                    // here would lose which one opened what.
                                    provider_egress,
                                    provider_gateway,
                                    // F8: this used to fall into the `..` and
                                    // be discarded, so the model never saw the
                                    // repository map or anything else the API
                                    // assembled. It reaches the prompt only
                                    // through `cortex_core::provenance`, which
                                    // frames every piece of it as data.
                                    context,
                                };

                                // Create cancel channel and register it
                                let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
                                {
                                    let mut map = cancel_map.lock().await;
                                    map.insert(step_id.clone(), cancel_tx);
                                }

                                let cmap = cancel_map.clone();
                                let sid = step_id.clone();
                                tokio::spawn(async move {
                                    execute_and_report(task, decision, step_exec, dir, out, cancel_rx).await;
                                    // Clean up cancel map entry when done
                                    cmap.lock().await.remove(&sid);
                                });
                            }
                            BrainMessage::CancelStep { step_id, reason } => {
                                tracing::info!("cancel step {step_id}: {reason}");
                                let mut map = cancel_map.lock().await;
                                if let Some(tx) = map.remove(&step_id) {
                                    let _ = tx.send(());
                                    tracing::info!("sent cancel signal for step {step_id}");
                                } else {
                                    tracing::warn!("no active execution found for step {step_id}");
                                }
                            }
                            BrainMessage::StaleLeaseNotice { step_id, your_lease_gen, current_lease_gen, disposition } => {
                                tracing::warn!(
                                    "stale lease for step {step_id}: gen {your_lease_gen} < {current_lease_gen}, {disposition}"
                                );
                            }
                            BrainMessage::Ping => {
                                let pong = serde_json::to_string(&WorkerMessage::Pong).unwrap();
                                let _ = write.send(Message::Text(pong.into())).await;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = write.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Err(e)) => return Err(format!("ws: {e}")),
                    _ => {}
                }
            }
        }
    }
}

async fn execute_and_report(
    task: cortex_core::task::TaskContract,
    decision: cortex_core::routing::RoutingDecision,
    step: StepExecution,
    dir: PathBuf,
    out: mpsc::Sender<WorkerMessage>,
    cancel_rx: oneshot::Receiver<()>,
) {
    let (tx, mut rx) = mpsc::channel::<WorkerEvent>(64);

    let task_clone = task.clone();
    let decision_clone = decision.clone();

    let step_id = step.step_id.clone();
    let attempt_id = step.attempt_id.clone();
    let lease_gen = step.lease_gen;

    // Spawn the actual executor
    let exec_handle = tokio::spawn(async move {
        Executor::execute_sandboxed(&task_clone, &decision_clone, &step, tx, dir.as_path()).await
    });

    // Lease renewal: send LeaseRenew every 30 seconds
    let lease_out = out.clone();
    let lease_step_id = step_id.clone();
    let (lease_stop_tx, mut lease_stop_rx) = oneshot::channel::<()>();
    let lease_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
        interval.tick().await; // first tick is immediate, skip it
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let msg = WorkerMessage::LeaseRenew {
                        step_id: lease_step_id.clone(),
                        lease_gen,
                    };
                    if lease_out.send(msg).await.is_err() {
                        break;
                    }
                    tracing::debug!("sent lease renewal for step {}", lease_step_id);
                }
                _ = &mut lease_stop_rx => {
                    break;
                }
            }
        }
    });

    // cancel_rx is consumed once when cancellation fires; we break immediately after.
    let mut cancel_rx = cancel_rx;

    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Some(ev) => {
                        let msg = worker_event_to_message(ev);
                        if out.send(msg).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        // Executor finished, event channel closed
                        break;
                    }
                }
            }
            _ = &mut cancel_rx => {
                tracing::info!("cancelling step {step_id} — aborting executor");

                // Abort the executor task which holds the child process
                exec_handle.abort();

                // Send a Failed event for the cancelled step
                let failure = WorkerFailureReport {
                    kind: WorkerFailureKind::Cancelled,
                    exit_code: None,
                    stderr_excerpt: Some("step cancelled by brain".to_string()),
                    tool: None,
                };
                let msg = WorkerMessage::StepFailed {
                    message_id: Uuid::new_v4().to_string(),
                    step_id: step_id.clone(),
                    attempt_id: attempt_id.clone(),
                    lease_gen,
                    failure,
                };
                let _ = out.send(msg).await;
                break;
            }
        }
    }

    // Stop lease renewal
    let _ = lease_stop_tx.send(());
    let _ = lease_handle.await;
}

fn detect_repos(workspace_dir: &str) -> Vec<RepoInfo> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(workspace_dir)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let path = String::from_utf8_lossy(&o.stdout).trim().to_string();

            let remote_url = std::process::Command::new("git")
                .args(["remote", "get-url", "origin"])
                .current_dir(&path)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

            let branch = std::process::Command::new("git")
                .args(["branch", "--show-current"])
                .current_dir(&path)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

            let head_commit = std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&path)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

            vec![RepoInfo {
                path,
                remote_url,
                branch,
                head_commit,
            }]
        }
        _ => Vec::new(),
    }
}
