use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tokio::sync::mpsc;
use uuid::Uuid;

use cortex_core::failure::WorkerFailureKind;
use cortex_core::protocol::{BrainMessage, StepOutput, WorkerMessage, PROTOCOL_VERSION};
use cortex_core::routing::RiskLevel;
use cortex_core::task::TaskContract;
use cortex_engine::captain::SchedulerEvent;
use cortex_engine::verifier::{
    verify_step, CheckEvidence as VerifierCheckEvidence, CheckStatus, StructuredStepEvidence,
    VerifierInput, VerifierVerdict, VerifierWorkContract,
};

use crate::clerk;
use crate::context_flow::{ArtifactKind, ContextBus};
use crate::mission_control::MissionControlEvent;
use crate::state::{AppState, StepEvent};

const GRACE_PERIOD_MS: i64 = 60_000;
const REGISTER_TIMEOUT_SECS: u64 = 10;

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    if state.is_shutting_down.load(Ordering::SeqCst) {
        return (StatusCode::SERVICE_UNAVAILABLE, "server is shutting down").into_response();
    }
    ws.on_upgrade(move |socket| handle_worker_connection(socket, state))
        .into_response()
}

async fn handle_worker_connection(mut socket: WebSocket, state: Arc<AppState>) {
    let worker_id = format!("w-{}", &Uuid::new_v4().to_string()[..12]);
    let session_id = Uuid::new_v4().to_string();
    tracing::info!("worker websocket connected: session={session_id}, worker={worker_id}");

    let welcome = serde_json::to_string(&BrainMessage::Welcome {
        session_id: session_id.clone(),
        worker_id: worker_id.clone(),
        protocol_version: PROTOCOL_VERSION,
    })
    .unwrap();

    if socket.send(Message::Text(welcome.into())).await.is_err() {
        return;
    }

    let (brain_tx, mut brain_rx) = mpsc::channel::<BrainMessage>(32);

    let mut registered = false;
    let mut authed_user_id: Option<String> = None;
    let mut step_run_cache: HashMap<String, String> = HashMap::new();

    // --- Registration timeout: first message must be a valid Register within REGISTER_TIMEOUT_SECS ---
    let first_msg = match tokio::time::timeout(
        Duration::from_secs(REGISTER_TIMEOUT_SECS),
        socket.recv(),
    )
    .await
    {
        Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<WorkerMessage>(&text) {
            Ok(msg @ WorkerMessage::Register { .. }) => Some(msg),
            Ok(_) => {
                tracing::warn!(
                    "worker {worker_id}: first message was not Register, closing connection"
                );
                let _ = socket.send(Message::Close(None)).await;
                return;
            }
            Err(e) => {
                tracing::warn!(
                    "worker {worker_id}: invalid first message: {e}, closing connection"
                );
                let _ = socket.send(Message::Close(None)).await;
                return;
            }
        },
        Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
            tracing::info!("worker {worker_id}: disconnected before registration");
            return;
        }
        Ok(Some(Err(e))) => {
            tracing::warn!("worker {worker_id}: socket error before registration: {e}");
            return;
        }
        Ok(_) => {
            tracing::warn!("worker {worker_id}: non-text message before registration, closing");
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
        Err(_) => {
            tracing::warn!(
                "worker {worker_id}: registration timeout ({REGISTER_TIMEOUT_SECS}s), closing connection"
            );
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };

    // Process the Register message
    if let Some(register_msg) = first_msg {
        handle_worker_msg(
            &state,
            &worker_id,
            &session_id,
            &mut registered,
            &mut authed_user_id,
            &brain_tx,
            register_msg,
            &mut step_run_cache,
        )
        .await;
    }

    // If registration/auth failed, close immediately
    if !registered {
        tracing::warn!("worker {worker_id}: registration failed, closing connection");
        let _ = socket.send(Message::Close(None)).await;
        return;
    }

    // --- Main event loop (only reached after successful registration) ---
    loop {
        tokio::select! {
            Some(msg) = brain_rx.recv() => {
                let json = serde_json::to_string(&msg).unwrap();
                if socket.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }

            result = socket.recv() => {
                match result {
                    Some(Ok(Message::Text(text))) => {
                        let worker_msg: WorkerMessage = match serde_json::from_str(&text) {
                            Ok(m) => m,
                            Err(e) => {
                                tracing::warn!("invalid worker message: {e}");
                                continue;
                            }
                        };
                        handle_worker_msg(
                            &state, &worker_id, &session_id,
                            &mut registered, &mut authed_user_id,
                            &brain_tx, worker_msg,
                            &mut step_run_cache,
                        ).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => continue,
                }
            }
        }
    }

    if registered {
        let user_id = authed_user_id.as_deref().unwrap_or("local");
        tracing::info!("worker disconnected: {worker_id} (user={user_id})");

        if let Some(db) = &state.db {
            db.disconnect_worker_session(&session_id);
            db.set_worker_status(&worker_id, "disconnected");

            let grace_deadline = chrono::Utc::now().timestamp_millis() + GRACE_PERIOD_MS;
            db.set_worker_grace_deadline(&worker_id, grace_deadline);
            tracing::info!(
                "worker {worker_id} grace period: {}s",
                GRACE_PERIOD_MS / 1000
            );
        }

        state.unregister_worker(&worker_id).await;
        state
            .emit_scheduler_event(SchedulerEvent::WorkerDisconnected {
                worker_id: worker_id.clone(),
            })
            .await;

        // Emit MC event for worker disconnected
        state
            .emit_mc_event(
                user_id,
                MissionControlEvent::WorkerDisconnected {
                    worker_id: worker_id.clone(),
                },
            )
            .await;
    }
}

async fn handle_worker_msg(
    state: &AppState,
    worker_id: &str,
    session_id: &str,
    registered: &mut bool,
    authed_user_id: &mut Option<String>,
    brain_tx: &mpsc::Sender<BrainMessage>,
    msg: WorkerMessage,
    step_run_cache: &mut HashMap<String, String>,
) {
    match msg {
        WorkerMessage::Register {
            token,
            protocol_version,
            providers,
            workspace_dir,
            repos,
        } => {
            // Authenticate worker via JWT if Clerk is configured
            let user_id = match authenticate_worker(state, &token).await {
                Ok(uid) => uid,
                Err(e) => {
                    tracing::warn!("worker auth failed: {e}");
                    let err_msg = serde_json::to_string(&serde_json::json!({
                        "type": "error",
                        "code": "auth_failed",
                        "message": e
                    }))
                    .unwrap();
                    let _ = brain_tx
                        .send(BrainMessage::CancelStep {
                            step_id: "auth".into(),
                            reason: err_msg,
                        })
                        .await;
                    return;
                }
            };

            *authed_user_id = Some(user_id.clone());

            let provider_ids: Vec<_> = providers.iter().map(|p| p.provider).collect();

            tracing::info!(
                "worker registered: id={worker_id}, user={user_id}, protocol=v{protocol_version}, \
                 providers={:?}, dir={workspace_dir}, repos={}",
                provider_ids,
                repos.len()
            );

            *registered = true;

            // Persist to DB
            if let Some(db) = &state.db {
                db.register_worker(worker_id, &user_id);
                db.create_worker_session(session_id, worker_id);

                for claim in &providers {
                    db.upsert_provider_capability(
                        worker_id,
                        &user_id,
                        &claim.provider.to_string(),
                        claim.cli_version.as_deref(),
                    );
                }
            }

            // Clear any disabled providers from previous sessions (re-auth resets)
            state.clear_disabled_providers(worker_id).await;

            // Capture provider strings before moving provider_ids
            let provider_strings: Vec<String> =
                provider_ids.iter().map(|p| p.to_string()).collect();

            state
                .register_worker(
                    worker_id.to_string(),
                    user_id.clone(),
                    provider_ids,
                    brain_tx.clone(),
                )
                .await;

            state
                .emit_scheduler_event(SchedulerEvent::WorkerConnected {
                    worker_id: worker_id.to_string(),
                })
                .await;

            // Emit MC event for worker connected
            state
                .emit_mc_event(
                    &user_id,
                    MissionControlEvent::WorkerConnected {
                        worker_id: worker_id.to_string(),
                        providers: provider_strings,
                    },
                )
                .await;
        }

        WorkerMessage::StepStarted {
            message_id,
            step_id,
            lease_gen,
            provider,
            model,
            execution_job,
            ..
        } => {
            tracing::info!("step {step_id} started: {provider}/{model} (msg={message_id})");
            if let Some(db) = &state.db {
                if !db.verify_step_worker(&step_id, worker_id) {
                    tracing::warn!(
                        "SECURITY: worker {worker_id} attempted StepStarted for step {step_id} \
                         which is not assigned to it — dropping message (msg={message_id})"
                    );
                    return;
                }
                if !db.start_step(&step_id, lease_gen) {
                    tracing::warn!(
                        "start_step returned false for step {step_id} lease_gen={lease_gen} — \
                         step may already be terminal or stale"
                    );
                }

                // Provenance for the receipt: image, isolation class, resource
                // profile, network policy, model identity, budgets. Recorded
                // after the worker-authenticity check above, so an unassigned
                // worker cannot write a job row.
                match &execution_job {
                    Some(job) => {
                        let run_id =
                            resolve_run_id(step_run_cache, state, &step_id).unwrap_or_default();
                        if !db.record_execution_job(&run_id, job) {
                            // Idempotent on (attempt_id, lease_gen): a
                            // resubmission is the same logical execution.
                            tracing::debug!(
                                step_id = %step_id,
                                attempt_id = %job.attempt_id,
                                "execution job already recorded for this attempt"
                            );
                        }
                    }
                    // A worker that predates the job field. Recorded as absent
                    // rather than reconstructed — a receipt that guesses its
                    // own provenance is worse than one that admits it has none.
                    None => tracing::warn!(
                        step_id = %step_id,
                        "worker reported no execution job; provenance for this attempt is unavailable"
                    ),
                }
            }

            // Emit MC event
            if let Some(user_id) = authed_user_id.as_deref() {
                let run_id = resolve_run_id(step_run_cache, state, &step_id);
                if let Some(run_id) = run_id {
                    state
                        .emit_mc_event(
                            user_id,
                            MissionControlEvent::StepStarted {
                                run_id,
                                step_id: step_id.clone(),
                                provider: provider.clone(),
                                model: model.clone(),
                            },
                        )
                        .await;
                }
            }
        }

        WorkerMessage::StepOutput { step_id, line, .. } => {
            tracing::debug!("step {step_id}: {}", &line[..line.len().min(80)]);

            // Forward to SSE channel (chat endpoint)
            if let Some(tx) = state.get_step_sender(&step_id).await {
                let _ = tx
                    .send(StepEvent::Output {
                        step_id: step_id.clone(),
                        line: line.clone(),
                    })
                    .await;
            }

            // Emit MC event (throttled by the MC connection handler)
            if let Some(user_id) = authed_user_id.as_deref() {
                let run_id = resolve_run_id(step_run_cache, state, &step_id);
                if let Some(run_id) = run_id {
                    state
                        .emit_mc_event(
                            user_id,
                            MissionControlEvent::StepOutput {
                                run_id,
                                step_id: step_id.clone(),
                                line: line.clone(),
                            },
                        )
                        .await;
                }
            }
        }

        WorkerMessage::StepCompleted {
            message_id,
            step_id,
            attempt_id,
            lease_gen,
            exit_code,
            output,
            base_commit,
            head_commit,
            branch,
            ..
        } => {
            // Verify this worker owns the step
            if let Some(db) = &state.db {
                if !db.verify_step_worker(&step_id, worker_id) {
                    tracing::warn!(
                        "SECURITY: worker {worker_id} attempted StepCompleted for step {step_id} \
                         which is not assigned to it — dropping message (msg={message_id})"
                    );
                    return;
                }
            }

            tracing::info!(
                "step {step_id} completed (exit {exit_code}, files_changed={}, branch={:?}) msg={message_id}",
                output.files_changed.len(),
                branch,
            );

            let files_json = serde_json::to_string(&output.files_changed).ok();
            let truncated_summary = if output.summary.len() > 2000 {
                format!("{}...[truncated]", &output.summary[..2000])
            } else {
                output.summary.clone()
            };
            let mut completion_accepted = false;
            let mut completion_error = "step failed before verification".to_string();
            // As above: assigned on every path that reads it, so an initialiser
            // here would only be a value nothing can observe.
            let step_transitioned;
            if let Some(db) = &state.db {
                // Record usage for pressure tracking
                record_step_usage(
                    db,
                    &step_id,
                    lease_gen,
                    authed_user_id.as_deref(),
                    output.tokens_in,
                    output.tokens_out,
                );

                let resolved_run_id = resolve_run_id(step_run_cache, state, &step_id);
                // No initialiser: every path below assigns this, and the one
                // that used to be here (`exit_code == 0`) was both dead and
                // misleading — a clean exit has never been what decides
                // whether a delivery is accepted.
                let verified_success;
                let mut verifier_failure = String::new();

                if let Some(run_id) = resolved_run_id.as_deref() {
                    let (
                        verifier_status,
                        verifier_verdict,
                        verifier_diagnostic,
                        evidence_json,
                        report_passed,
                    ) = verify_worker_completion(
                        db,
                        run_id,
                        &step_id,
                        &attempt_id,
                        lease_gen,
                        worker_id,
                        exit_code,
                        &output,
                        base_commit.as_deref(),
                        head_commit.as_deref(),
                        branch.as_deref(),
                        &truncated_summary,
                        &message_id,
                    );

                    verified_success = report_passed;
                    if !report_passed {
                        verifier_failure = verifier_diagnostic;
                    }

                    db.record_verifier_report(
                        &step_id,
                        run_id,
                        lease_gen,
                        Some(worker_id),
                        "engine_verifier",
                        &verifier_status,
                        &verifier_verdict,
                        &evidence_json,
                    );
                } else {
                    verifier_failure =
                        "verifier could not resolve run for completed step".to_string();
                    verified_success = false;
                }

                if verified_success {
                    step_transitioned = db.deliver_step(
                        &step_id,
                        &attempt_id,
                        lease_gen,
                        Some(&truncated_summary),
                        files_json.as_deref(),
                        base_commit.as_deref(),
                        head_commit.as_deref(),
                    );
                    if !step_transitioned {
                        tracing::warn!(
                            "deliver_step returned false for step {step_id} lease_gen={lease_gen} — \
                             likely stale lease_gen (step may have been re-leased or already delivered)"
                        );
                        completion_error =
                            "stale worker completion ignored: step is no longer leased to this attempt"
                                .to_string();
                    } else {
                        db.deliver_attempt(&step_id, lease_gen);

                        // The step is `delivered`: a tree exists and nothing
                        // has been checked. It is not done, and nothing
                        // downstream may treat it as done.
                        //
                        // Reaching this branch means deliver_step's CAS on
                        // lease_gen held, so this delivery is the live one — a
                        // superseded attempt never gets here and so can never
                        // bill.
                        //
                        // Delivery *enqueues*; it does not execute. The
                        // verification job is inserted in the same transaction
                        // that moves the step to `verifying`, so a crash
                        // between the two is not representable. The dispatcher
                        // claims it, and a dispatcher that dies mid-check
                        // leaves a claim that expires rather than a delivery
                        // that is stranded.
                        //
                        // This used to be `tokio::spawn`. A restart, a deploy,
                        // an unavailable container runtime, or a panic and the
                        // verification simply never happened — and the code
                        // said so in a log line nobody was watching.
                        if let (Some(run_id), Some(head)) =
                            (resolved_run_id.clone(), head_commit.clone())
                        {
                            // The exam, frozen at dispatch. Digested now so the
                            // dispatcher can refuse a job whose specs changed
                            // between enqueue and claim rather than grading
                            // against a different exam than the one promised.
                            let specs = db.load_check_specs(&run_id, &step_id);
                            let digest = crate::db::spec_set_digest(&specs);
                            let job_id = uuid::Uuid::new_v4().to_string();

                            let enqueued = db.begin_verifying_step(
                                &step_id,
                                &attempt_id,
                                lease_gen,
                                Some(crate::db::VerificationEnqueue {
                                    job_id: &job_id,
                                    run_id: &run_id,
                                    delivered_commit: &head,
                                    spec_set_digest: &digest,
                                    runner_policy_ver:
                                        crate::verification_driver::RUNNER_POLICY_VERSION,
                                }),
                            );
                            if enqueued {
                                tracing::info!(
                                    step_id = %step_id,
                                    job_id = %job_id,
                                    "delivery enqueued for independent verification"
                                );
                            } else {
                                tracing::warn!(
                                    step_id = %step_id,
                                    "could not move the delivery to verifying; \
                                     it stays delivered and reconciliation will see it"
                                );
                            }
                        } else {
                            // No run or no commit means there is nothing to
                            // grade against. The step stays `delivered` rather
                            // than being promoted, which is the honest state.
                            tracing::warn!(
                                step_id = %step_id,
                                "delivered with no resolvable run or head commit; \
                                 nothing to verify against, step stays delivered"
                            );
                        }
                    }
                } else {
                    if verifier_failure.is_empty() {
                        verifier_failure = "verifier rejected worker completion".to_string();
                    }
                    completion_error = verifier_failure.clone();
                    db.record_failed_step_output(
                        &step_id,
                        lease_gen,
                        Some(&truncated_summary),
                        files_json.as_deref(),
                        base_commit.as_deref(),
                        head_commit.as_deref(),
                    );
                    step_transitioned = db.fail_step(
                        &step_id,
                        lease_gen,
                        &verifier_failure,
                        Some("VerifierRejected"),
                    );
                    if step_transitioned {
                        db.fail_attempt(
                            &step_id,
                            lease_gen,
                            Some("VerifierRejected"),
                            Some(&verifier_failure),
                        );
                    } else {
                        tracing::warn!(
                            "fail_step returned false for verifier-rejected step {step_id} lease_gen={lease_gen} — \
                             likely stale lease_gen (step may have been re-leased or already completed)"
                        );
                        completion_error =
                            "stale verifier rejection ignored: step is no longer leased to this attempt"
                                .to_string();
                    }
                }
                completion_accepted = verified_success && step_transitioned;

                if step_transitioned {
                    if let Some(run_id) = resolved_run_id {
                        if verified_success {
                            if let Some(ref branch_name) = branch {
                                db.record_run_branch(&run_id, branch_name);
                            }
                        }

                        if verified_success {
                            state
                                .emit_scheduler_event(SchedulerEvent::StepDelivered {
                                    run_id: run_id.clone(),
                                    step_id: step_id.clone(),
                                    cost_estimate: output.cost_estimate,
                                })
                                .await;
                        } else {
                            state
                                .emit_scheduler_event(SchedulerEvent::StepFailed {
                                    run_id: run_id.clone(),
                                    step_id: step_id.clone(),
                                })
                                .await;
                        }

                        if let Some(user_id) = authed_user_id.as_deref() {
                            if verified_success {
                                state
                                    .emit_mc_event(
                                        user_id,
                                        MissionControlEvent::StepDelivered {
                                            run_id: run_id.clone(),
                                            step_id: step_id.clone(),
                                            exit_code,
                                            files_changed: output.files_changed.clone(),
                                            cost_estimate: output.cost_estimate,
                                        },
                                    )
                                    .await;
                            } else {
                                state
                                    .emit_mc_event(
                                        user_id,
                                        MissionControlEvent::StepFailed {
                                            run_id: run_id.clone(),
                                            step_id: step_id.clone(),
                                            error: verifier_failure.clone(),
                                            failure_kind: "VerifierRejected".to_string(),
                                        },
                                    )
                                    .await;
                            }
                        }

                        let artifact_kind = if verified_success {
                            if !output.files_changed.is_empty() {
                                ArtifactKind::Code
                            } else if output.summary.to_lowercase().contains("analy") {
                                ArtifactKind::Analysis
                            } else if output.summary.to_lowercase().contains("plan") {
                                ArtifactKind::Plan
                            } else {
                                ArtifactKind::Answer
                            }
                        } else {
                            ArtifactKind::Error
                        };

                        // A delivered artifact carries the worker's own account
                        // of its work, and nothing has checked it. It is still
                        // the best context a downstream step has, so it is kept
                        // — but it cannot claim the confidence of a verified
                        // result. Raising it is the verifier's job, once there
                        // is a verdict to raise it on.
                        //
                        // The artifact's *provenance* is `AgentOutput`
                        // regardless of which branch this takes, set inside
                        // `create_artifact`. That is the load-bearing part: a
                        // downstream step renders it as an unverified prior
                        // account either way, and cannot be persuaded otherwise
                        // by a float. This number only orders artifacts against
                        // each other during retrieval.
                        let confidence = if verified_success { 0.5 } else { 0.1 };
                        let artifact = ContextBus::create_artifact(
                            &step_id,
                            &run_id,
                            artifact_kind,
                            &output.summary,
                            &truncated_summary,
                            output.files_changed.clone(),
                            confidence,
                        );

                        state
                            .context_bus
                            .add_artifact(state.db.as_ref(), artifact)
                            .await;

                        tracing::debug!("artifact added to context-flow pipeline for run {run_id}");
                    }

                    if let Some(user_id) = authed_user_id.as_deref() {
                        if !verified_success {
                            state.vera_tracker.record_step_failed(
                                user_id,
                                "VerifierRejected",
                                None,
                            );
                        }
                        // No success interaction is recorded on delivery, and
                        // there is no longer a `record_step_completed` to do
                        // it with: it derived its outcome from the worker's
                        // exit code, which is precisely the signal invariant 6
                        // forbids from improving a score.
                        //
                        // The positive signal now comes from the verdict, in
                        // `verification_dispatcher`. That is the only place it
                        // can originate.
                        //
                        // The asymmetry above is deliberate, not an oversight:
                        // a self-reported *failure* is recorded here while a
                        // self-reported success is not. Invariant 6 forbids a
                        // self-report from improving a score; nobody reports
                        // themselves failing in order to look better, so the
                        // negative direction carries no such incentive.
                        //
                        // Nor does it double-count. `completion_accepted` is
                        // `verified_success && step_transitioned`, so a
                        // delivery rejected here never transitions to
                        // `verifying`, never enqueues a verification job, and
                        // therefore never produces a verdict that could record
                        // the same failure a second time.
                    }
                }
            }

            // Forward to SSE channel (chat endpoint)
            if let Some(tx) = state.get_step_sender(&step_id).await {
                if completion_accepted {
                    let _ = tx
                        .send(StepEvent::Completed {
                            step_id: step_id.clone(),
                            exit_code,
                        })
                        .await;
                } else {
                    let _ = tx
                        .send(StepEvent::Failed {
                            step_id: step_id.clone(),
                            error: completion_error,
                        })
                        .await;
                }
            }
            state.remove_step_sender(&step_id).await;
        }

        WorkerMessage::StepFailed {
            message_id,
            step_id,
            lease_gen,
            failure,
            ..
        } => {
            // Verify this worker owns the step
            if let Some(db) = &state.db {
                if !db.verify_step_worker(&step_id, worker_id) {
                    tracing::warn!(
                        "SECURITY: worker {worker_id} attempted StepFailed for step {step_id} \
                         which is not assigned to it — dropping message (msg={message_id})"
                    );
                    return;
                }
            }

            tracing::warn!("step {step_id} failed: {:?} msg={message_id}", failure.kind);

            let error_msg = failure.stderr_excerpt.as_deref().unwrap_or("unknown error");
            let kind_str = format!("{:?}", failure.kind);
            let is_cancelled = matches!(failure.kind, WorkerFailureKind::Cancelled);

            // Classify the failure to detect auth expiry
            let is_auth_expired = matches!(
                failure.kind,
                WorkerFailureKind::CliNotAuthenticated | WorkerFailureKind::CliAuthExpired
            );

            if let Some(db) = &state.db {
                let step_transitioned = if is_cancelled {
                    db.cancel_step(&step_id, lease_gen, error_msg)
                } else {
                    db.fail_step(&step_id, lease_gen, error_msg, Some(&kind_str))
                };
                if !step_transitioned {
                    tracing::warn!(
                        "terminal failure transition returned false for step {step_id} lease_gen={lease_gen} — \
                         likely stale lease_gen (step may have been re-leased or already completed)"
                    );
                    if let Some(tx) = state.get_step_sender(&step_id).await {
                        let _ = tx
                            .send(StepEvent::Failed {
                                step_id: step_id.clone(),
                                error: "stale worker failure ignored: step is no longer leased to this attempt".to_string(),
                            })
                            .await;
                    }
                    state.remove_step_sender(&step_id).await;
                    return;
                }
                db.fail_attempt(&step_id, lease_gen, Some(&kind_str), Some(error_msg));

                // Record usage even on failure (still consumed tokens/time)
                record_step_usage(
                    db,
                    &step_id,
                    lease_gen,
                    authed_user_id.as_deref(),
                    None,
                    None,
                );

                if let Some(run_id) = resolve_run_id(step_run_cache, state, &step_id) {
                    state
                        .emit_scheduler_event(SchedulerEvent::StepFailed {
                            run_id: run_id.clone(),
                            step_id: step_id.clone(),
                        })
                        .await;

                    // Emit MC event
                    if let Some(user_id) = authed_user_id.as_deref() {
                        state
                            .emit_mc_event(
                                user_id,
                                MissionControlEvent::StepFailed {
                                    run_id,
                                    step_id: step_id.clone(),
                                    error: error_msg.to_string(),
                                    failure_kind: kind_str.clone(),
                                },
                            )
                            .await;
                    }
                }

                // Vera observes the failed step
                if let Some(user_id) = authed_user_id.as_deref() {
                    state
                        .vera_tracker
                        .record_step_failed(user_id, &kind_str, None);
                }

                // Forward to SSE channel (chat endpoint)
                if let Some(tx) = state.get_step_sender(&step_id).await {
                    let _ = tx
                        .send(StepEvent::Failed {
                            step_id: step_id.clone(),
                            error: error_msg.to_string(),
                        })
                        .await;
                }
                state.remove_step_sender(&step_id).await;

                // Emit ProviderAuthExpired so scheduler disables the provider on this worker
                if is_auth_expired {
                    let provider_name = failure
                        .tool
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string());
                    let user_id = authed_user_id
                        .clone()
                        .unwrap_or_else(|| "local".to_string());
                    tracing::warn!(
                        "provider auth expired: provider={provider_name}, worker={worker_id}, user={user_id}"
                    );
                    state
                        .emit_scheduler_event(SchedulerEvent::ProviderAuthExpired {
                            worker_id: worker_id.to_string(),
                            provider: provider_name,
                            user_id,
                        })
                        .await;
                }
            }
        }

        WorkerMessage::StepBlocked {
            message_id,
            step_id,
            attempt_id,
            lease_gen,
            blocked,
        } => {
            if let Some(db) = &state.db {
                if !db.verify_step_worker(&step_id, worker_id) {
                    tracing::warn!(
                        "SECURITY: worker {worker_id} attempted StepBlocked for step {step_id} \
                         which is not assigned to it — dropping message (msg={message_id})"
                    );
                    return;
                }
            }

            // The worker refused to execute because it could not establish the
            // isolation the job required. That is our infrastructure failing,
            // not the customer's step, so it is recorded as `execution_failed`
            // and never as `failed`. Nothing is charged, and no bandit learns
            // that a provider did badly — it never ran.
            tracing::error!(
                step_id = %step_id,
                reason = %blocked.reason.as_str(),
                detail = %blocked.detail,
                "worker refused to execute; the sandbox could not be established"
            );

            if let Some(db) = &state.db {
                let reason = blocked.to_string();
                let transitioned =
                    db.record_execution_failure(&step_id, &attempt_id, lease_gen, &reason);
                if !transitioned {
                    tracing::warn!(
                        "execution failure did not apply to step {step_id} lease_gen={lease_gen} — \
                         likely a stale attempt"
                    );
                }
                db.fail_attempt(
                    &step_id,
                    lease_gen,
                    Some(blocked.reason.as_str()),
                    Some(&reason),
                );

                if transitioned {
                    if let Some(run_id) = resolve_run_id(step_run_cache, state, &step_id) {
                        state
                            .emit_scheduler_event(SchedulerEvent::StepFailed {
                                run_id: run_id.clone(),
                                step_id: step_id.clone(),
                            })
                            .await;

                        if let Some(user_id) = authed_user_id.as_deref() {
                            state
                                .emit_mc_event(
                                    user_id,
                                    MissionControlEvent::StepFailed {
                                        run_id,
                                        step_id: step_id.clone(),
                                        error: reason.clone(),
                                        // Named for what it is. An operator
                                        // reading this needs to know the cause
                                        // is theirs to fix, not the task's.
                                        failure_kind: "ExecutionBlocked".to_string(),
                                    },
                                )
                                .await;
                        }
                    }
                }
            }

            if let Some(tx) = state.get_step_sender(&step_id).await {
                let _ = tx
                    .send(StepEvent::Failed {
                        step_id: step_id.clone(),
                        error: blocked.to_string(),
                    })
                    .await;
            }
            state.remove_step_sender(&step_id).await;
        }

        WorkerMessage::LeaseRenew { step_id, lease_gen } => {
            // Verify this worker owns the step
            if let Some(db) = &state.db {
                if !db.verify_step_worker(&step_id, worker_id) {
                    tracing::warn!(
                        "SECURITY: worker {worker_id} attempted LeaseRenew for step {step_id} \
                         which is not assigned to it — dropping message"
                    );
                    return;
                }
            }

            if let Some(db) = &state.db {
                // Extend lease by 5 minutes from now
                let new_deadline = chrono::Utc::now().timestamp_millis() + 5 * 60 * 1000;
                let renewed = db.renew_lease(&step_id, lease_gen, new_deadline);
                if renewed {
                    tracing::debug!("lease renewed for step {step_id} gen={lease_gen}");
                } else {
                    tracing::warn!(
                        "lease renewal failed for step {step_id} gen={lease_gen} — stale or not leased"
                    );
                }
            }
        }

        WorkerMessage::Heartbeat { active_steps } => {
            tracing::debug!("heartbeat: {} active steps", active_steps.len());
            if let Some(db) = &state.db {
                db.update_worker_seen(worker_id);
                db.update_heartbeat(worker_id);
            }
        }

        WorkerMessage::Pong => {}
    }
}

fn verify_worker_completion(
    db: &crate::db::Database,
    run_id: &str,
    step_id: &str,
    attempt_id: &str,
    lease_gen: i64,
    worker_id: &str,
    exit_code: i32,
    output: &StepOutput,
    base_commit: Option<&str>,
    head_commit: Option<&str>,
    branch: Option<&str>,
    truncated_summary: &str,
    message_id: &str,
) -> (String, String, String, String, bool) {
    let persisted_contract = db.get_step_work_contract(step_id, lease_gen);
    let (risk, objective) = db
        .get_step_details(step_id)
        .map(|(_, _, _, risk, objective)| (parse_verifier_risk(&risk), objective))
        .unwrap_or((RiskLevel::Medium, String::new()));

    let git = output
        .evidence
        .as_ref()
        .and_then(|packet| packet.git.as_ref());
    let checks = output
        .evidence
        .as_ref()
        .map(|packet| {
            packet
                .checks
                .iter()
                .map(|check| VerifierCheckEvidence {
                    name: check.name.clone(),
                    status: check_status(check.exit_code, check.timed_out),
                    summary: check_summary(check),
                })
                .collect()
        })
        .unwrap_or_default();
    let evidence = StructuredStepEvidence {
        step_id: Some(step_id.to_string()),
        attempt_id: Some(attempt_id.to_string()),
        summary: output.summary.clone(),
        exit_code: Some(exit_code),
        files_changed: output.files_changed.clone(),
        base_commit: git
            .and_then(|evidence| evidence.base_commit.clone())
            .or_else(|| base_commit.map(str::to_string)),
        head_commit: git
            .and_then(|evidence| evidence.head_commit.clone())
            .or_else(|| head_commit.map(str::to_string)),
        commands: Vec::new(),
        checks,
        signals: Vec::new(),
    };

    let fallback_allowed_paths = db.get_run_file_paths(run_id);
    let fallback_base_commit = base_commit.map(str::to_string);
    let contract = persisted_contract
        .as_ref()
        .map(|task| {
            VerifierWorkContract::from_task_contract(
                task,
                fallback_allowed_paths.clone(),
                fallback_base_commit.clone(),
            )
        })
        .unwrap_or_else(|| {
            VerifierWorkContract::from_task_contract(
                &fallback_task_contract(objective, risk),
                fallback_allowed_paths,
                fallback_base_commit,
            )
        });

    let input = VerifierInput { contract, evidence };

    let report = verify_step(input.clone());
    let status = verifier_status(report.verdict).to_string();
    let verdict = verifier_verdict(report.verdict).to_string();

    // What this boolean decides is whether the step is *delivered for
    // independent verification*, not whether it is any good. So it turns on
    // contract violations only.
    //
    // `report` is built from the worker's own account of itself, and every
    // input to it -- the check outcomes, the command outcomes, the evidence
    // floor -- is worker-reported. Rejecting a step here on that basis is
    // F14's mistake one hop further on: the work never reaches the dispatcher,
    // never earns a verdict, and `Verdict::Failed` and the refund behind it
    // stay unreachable. ADR-0001 and invariant 6 both say a worker's report is
    // a diagnostic and not a transition guard. See F19.
    //
    // `Blocked` is the exception and stays a gate, because it means the
    // *contract* was broken -- edits outside the allowed paths, or a base that
    // no longer matches. That is a fact about what the work was permitted to
    // touch rather than a judgement about whether it is correct, which is the
    // same line the worker's own auto-commit draws.
    //
    // The report itself is unchanged and still recorded verbatim below, so the
    // diagnosis survives even though it no longer decides anything.
    let passed = !matches!(report.verdict, VerifierVerdict::Blocked);
    let diagnostic = verifier_diagnostic(&status, &verdict, &report);
    let evidence_json = serde_json::json!({
        "source": "engine_verifier",
        "message_id": message_id,
        "step_id": step_id,
        "run_id": run_id,
        "lease_gen": lease_gen,
        "worker_id": worker_id,
        "worker_completed": {
            "exit_code": exit_code,
            "summary": truncated_summary,
            "files_changed": output.files_changed.clone(),
            "base_commit": base_commit,
            "head_commit": head_commit,
            "branch": branch,
            "cost_estimate": output.cost_estimate,
            "structured": output.structured.clone(),
        },
        "verifier_input": input,
        "verifier_report": report,
    })
    .to_string();

    (status, verdict, diagnostic, evidence_json, passed)
}

fn fallback_task_contract(objective: String, risk: RiskLevel) -> TaskContract {
    TaskContract::new(objective, cortex_core::provider::Tier::Execute, risk)
}

fn check_status(exit_code: Option<i32>, timed_out: bool) -> CheckStatus {
    if timed_out {
        CheckStatus::Failed
    } else {
        match exit_code {
            Some(0) => CheckStatus::Passed,
            Some(_) => CheckStatus::Failed,
            None => CheckStatus::Unknown,
        }
    }
}

fn check_summary(check: &cortex_core::protocol::CheckEvidence) -> Option<String> {
    let mut parts = Vec::new();
    if check.timed_out {
        parts.push("timed out".to_string());
    }
    if let Some(code) = check.exit_code {
        parts.push(format!("exit code {code}"));
    }
    if let Some(stderr) = check
        .stderr_excerpt
        .as_ref()
        .filter(|s| !s.trim().is_empty())
    {
        parts.push(stderr.clone());
    } else if let Some(stdout) = check
        .stdout_excerpt
        .as_ref()
        .filter(|s| !s.trim().is_empty())
    {
        parts.push(stdout.clone());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(": "))
    }
}

fn verifier_diagnostic(
    status: &str,
    verdict: &str,
    report: &cortex_engine::verifier::VerifierReport,
) -> String {
    let mut parts = vec![format!(
        "verifier rejected worker completion: status={status}, verdict={verdict}, next_action={:?}",
        report.next_action
    )];

    if !report.evidence_floor.satisfied && !report.evidence_floor.missing.is_empty() {
        parts.push(format!(
            "missing evidence: {}",
            report.evidence_floor.missing.join(", ")
        ));
    }

    if !report.allowed_path_violations.is_empty() {
        let violations = report
            .allowed_path_violations
            .iter()
            .take(5)
            .map(|violation| format!("{} ({})", violation.path, violation.reason))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("path violations: {violations}"));
    }

    if !report.stale_base_notes.is_empty() {
        let notes = report
            .stale_base_notes
            .iter()
            .take(3)
            .map(|note| note.note.clone())
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("base mismatch: {notes}"));
    }

    if !report.required_check_summary.missing.is_empty() {
        let missing = report
            .required_check_summary
            .missing
            .iter()
            .take(5)
            .map(|check| check.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("missing required checks: {missing}"));
    }

    if !report.required_check_summary.failed.is_empty() {
        let failed = report
            .required_check_summary
            .failed
            .iter()
            .take(5)
            .map(|check| {
                check
                    .summary
                    .as_ref()
                    .filter(|summary| !summary.trim().is_empty())
                    .map(|summary| format!("{} ({summary})", check.name))
                    .unwrap_or_else(|| check.name.clone())
            })
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("failed required checks: {failed}"));
    }

    if !report.check_summary.failures.is_empty() {
        let failed = report
            .check_summary
            .failures
            .iter()
            .take(5)
            .map(|failure| {
                failure
                    .summary
                    .as_ref()
                    .filter(|summary| !summary.trim().is_empty())
                    .map(|summary| format!("{} ({summary})", failure.name))
                    .unwrap_or_else(|| failure.name.clone())
            })
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("check failures: {failed}"));
    }

    if report.command_summary.failed > 0 {
        let failed = report
            .command_summary
            .failures
            .iter()
            .take(5)
            .map(|failure| {
                failure
                    .summary
                    .as_ref()
                    .filter(|summary| !summary.trim().is_empty())
                    .map(|summary| format!("{} ({summary})", failure.command))
                    .unwrap_or_else(|| failure.command.clone())
            })
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("command failures: {failed}"));
    }

    truncate_diagnostic(&parts.join("; "), 2_000)
}

fn truncate_diagnostic(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn parse_verifier_risk(risk: &str) -> RiskLevel {
    match risk.to_ascii_lowercase().as_str() {
        "low" => RiskLevel::Low,
        "high" => RiskLevel::High,
        "critical" => RiskLevel::Critical,
        _ => RiskLevel::Medium,
    }
}

fn verifier_status(verdict: VerifierVerdict) -> &'static str {
    match verdict {
        VerifierVerdict::Success | VerifierVerdict::Failed | VerifierVerdict::Blocked => "verified",
        VerifierVerdict::NeedsEvidence => "needs_evidence",
    }
}

fn verifier_verdict(verdict: VerifierVerdict) -> &'static str {
    match verdict {
        VerifierVerdict::Success => "pass",
        VerifierVerdict::NeedsEvidence => "needs_evidence",
        VerifierVerdict::Failed => "fail",
        VerifierVerdict::Blocked => "blocked",
    }
}

/// Resolve step_id to run_id using a per-connection cache to avoid repeated DB lookups.
/// A step producing 100 output lines would otherwise trigger 100 DB queries with Mutex locks.
fn resolve_run_id(
    cache: &mut HashMap<String, String>,
    state: &AppState,
    step_id: &str,
) -> Option<String> {
    if let Some(cached) = cache.get(step_id) {
        return Some(cached.clone());
    }
    if let Some(db) = &state.db {
        if let Some(rid) = db.get_step_run_id(step_id) {
            cache.insert(step_id.to_string(), rid.clone());
            return Some(rid);
        }
    }
    None
}

fn record_step_usage(
    db: &crate::db::Database,
    step_id: &str,
    lease_gen: i64,
    user_id: Option<&str>,
    tokens_in: Option<i64>,
    tokens_out: Option<i64>,
) {
    let user_id = user_id.unwrap_or("local");

    if let Some((provider, model, started_at)) = db.get_attempt_provider_model(step_id, lease_gen) {
        let now = chrono::Utc::now().timestamp_millis();
        let duration_ms = now - started_at;

        let tier = if let Some((_, _, tier_str, _, _)) = db.get_step_details(step_id) {
            tier_str
        } else {
            "execute".to_string()
        };

        db.record_usage(
            user_id,
            &provider,
            &tier,
            &model,
            None,
            tokens_in,
            tokens_out,
            Some(duration_ms),
        );
    }
}

async fn authenticate_worker(state: &AppState, token: &str) -> Result<String, String> {
    // A `cwk_` worker service credential, checked FIRST.
    //
    // First because it is the only credential a headless worker can actually
    // hold: it has no browser, so it can never present a Clerk user JWT, and
    // before this branch existed the only worker token the server accepted in
    // practice was the empty string. Ordering it ahead of the empty-token
    // branch also means the anonymous path below can never shadow a real
    // credential, whatever that path is later configured to do.
    if crate::worker_key::is_worker_key(token) {
        let db = state
            .db
            .as_ref()
            .ok_or_else(|| "database not available".to_string())?;
        let now = chrono::Utc::now().timestamp_millis();
        // Unknown, revoked and expired are one answer on purpose: an
        // authentication boundary does not tell a caller which of those it was.
        // The revoked/expired filtering is in the SQL, not here.
        return db
            .authenticate_worker_key(&crate::worker_key::hash_worker_key(token), now)
            .ok_or_else(|| "invalid worker key".to_string());
    }

    if token.is_empty() {
        // The anonymous path, and it is now closed unless somebody opened it.
        //
        // This used to read `if state.clerk_secret_key.is_none()`, which made
        // "Clerk is not configured" mean "anyone may connect as `local`". Those
        // are not the same statement: a deployment can lose its
        // `CLERK_SECRET_KEY` by accident, and production ran for months in
        // exactly that configuration. Nobody sets
        // `CORTEX_ALLOW_ANONYMOUS_WORKER=1` by accident.
        if state.allow_anonymous_worker {
            return Ok(crate::worker_key::ANONYMOUS_WORKER_USER.to_string());
        }
        return Err("empty auth token".into());
    }

    // A token that starts with '{' is a JSON Soma delegation. Without the
    // `soma` feature this build has no way to verify one, so it is an unknown
    // credential format and is rejected here. It must not fall through to the
    // Clerk verifier: a JSON blob is not a JWT, and the failure would be
    // reported as a bad JWT.
    //
    // This fence used to carry a second, sharper reason: an unset
    // `clerk_secret_key` made the fallback below answer `Ok("local")` for
    // anything, so a Soma-shaped token that reached it would have been
    // *authenticated*. That hole is closed — the fallback now refuses unless
    // `CORTEX_ALLOW_ANONYMOUS_WORKER` is set — but the fence stays, because
    // naming the credential format in the error is still the right answer and
    // because it does not depend on how the anonymous path is configured.
    #[cfg(not(feature = "soma"))]
    if token.starts_with('{') {
        return Err("unsupported worker credential format".into());
    }

    // Try Soma delegation first (token is JSON starting with '{')
    #[cfg(feature = "soma")]
    if token.starts_with('{') {
        let delegation: soma::delegation::Delegation =
            serde_json::from_str(token).map_err(|e| format!("invalid soma token: {e}"))?;

        let cortex_did = state
            .soma_heart
            .as_ref()
            .map(|h| h.did().to_string())
            .unwrap_or_default();

        let ctx = soma::delegation::InvocationContext {
            invoker_did: delegation.subject_did.clone(),
            audience_did: Some(cortex_did),
            capability: "route:*".into(),
            ..Default::default()
        };

        // Verify issuer is Cortex's heart — reject self-issued delegations
        if let Some(heart) = &state.soma_heart {
            if delegation.issuer_did != heart.did() {
                return Err(format!(
                    "delegation issuer {} is not Cortex heart {}",
                    delegation.issuer_did,
                    heart.did()
                ));
            }
        }

        let result = soma::delegation::verify_delegation(&delegation, &ctx)
            .map_err(|e| format!("soma verification error: {e}"))?;

        if !result.is_valid() {
            return Err(format!("soma delegation invalid: {result:?}"));
        }

        if let Some(heart) = &state.soma_heart {
            heart.record_heartbeat(
                soma::heartbeat::HeartbeatEventType::QueryReceived,
                &serde_json::json!({
                    "type": "worker_connect",
                    "delegation_id": delegation.id,
                    "subject_did": delegation.subject_did,
                })
                .to_string(),
            );
        }

        tracing::info!(
            "worker authenticated via soma delegation: {}",
            delegation.subject_did
        );
        return Ok(delegation.subject_did);
    }

    // Fall back to Clerk JWT.
    let clerk_secret = match &state.clerk_secret_key {
        Some(key) => key,
        None => {
            // Same change as above, and this was the more dangerous of the two:
            // with no Clerk secret this arm answered `Ok("local")` for *any*
            // non-empty token it was handed, having verified nothing at all.
            if state.allow_anonymous_worker {
                return Ok(crate::worker_key::ANONYMOUS_WORKER_USER.to_string());
            }
            return Err("worker authentication is not configured".into());
        }
    };

    let keys = clerk::get_or_refresh_jwks_pub(
        &state.jwks_cache,
        &state.jwks_stampede,
        clerk_secret,
        false,
    )
    .await?;

    match clerk::verify_token_pub(token, &keys) {
        Ok(user_id) => Ok(user_id),
        Err(_) => {
            let keys = clerk::get_or_refresh_jwks_pub(
                &state.jwks_cache,
                &state.jwks_stampede,
                clerk_secret,
                true,
            )
            .await?;
            clerk::verify_token_pub(token, &keys)
        }
    }
}

#[cfg(all(test, not(feature = "soma")))]
mod soma_fence_tests {
    use super::*;

    async fn test_state() -> std::sync::Arc<AppState> {
        let temporary = tempfile::tempdir().expect("temporary workspace");
        let workspace = temporary.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".cortex")).expect("workspace metadata");
        // `None` for the Clerk secret is the local-dev configuration, and it
        // used to be the dangerous one: it was the configuration in which the
        // fallback path at the bottom of `authenticate_worker` returned
        // `Ok("local")` for anything it was handed. That hole is closed now
        // (see `worker_credential`), but leaving the secret unset here is still
        // the point of this test — the rejection has to come from the format
        // check, not from Clerk being configured.
        let state = AppState::new(workspace.join(".cortex/ledger.jsonl"), workspace, None).await;
        std::mem::forget(temporary);
        state
    }

    /// A Soma delegation is JSON. Without the feature this build cannot verify
    /// one, so it must be refused as an unknown credential format — not
    /// accepted, and not quietly handed to the Clerk verifier, which in a
    /// no-Clerk deployment would answer `Ok("local")` and authenticate it.
    #[tokio::test]
    async fn json_worker_token_is_rejected_rather_than_falling_through_to_local() {
        let state = test_state().await;

        let delegation = serde_json::json!({
            "id": "deleg-1",
            "issuer_did": "did:soma:whoever",
            "subject_did": "did:soma:attacker",
            "capabilities": ["route:*"],
        })
        .to_string();

        let outcome = authenticate_worker(&state, &delegation).await;

        assert!(
            outcome.is_err(),
            "a JSON credential must not authenticate a worker in a build \
             without the soma feature, got {outcome:?}"
        );
        assert_ne!(
            outcome.ok().as_deref(),
            Some("local"),
            "the no-Clerk fallback must never see a Soma-shaped token"
        );
    }

    /// The empty-token local-dev path is untouched by the fence — when the
    /// operator has explicitly opened it. Asserted so a later tightening of the
    /// rejection above cannot silently break local development and be mistaken
    /// for the fence working.
    ///
    /// Note what changed: the escape hatch now has to be opened on the state.
    /// An unset `CLERK_SECRET_KEY` no longer implies it. The corresponding
    /// closed-by-default assertion lives in `worker_credential`.
    #[tokio::test]
    async fn empty_token_means_local_dev_when_anonymous_workers_are_allowed() {
        let mut state = test_state().await;
        std::sync::Arc::get_mut(&mut state)
            .expect("sole owner of the state")
            .allow_anonymous_worker = true;
        assert_eq!(
            authenticate_worker(&state, "").await.ok().as_deref(),
            Some("local")
        );
    }
}

/// The worker service credential, and the anonymous path it closes.
///
/// # Why these tests are written the way they are
///
/// Every worker test that existed before this module built `AppState` with
/// `clerk_secret_key: None`. That is the one configuration in which the empty
/// token was *accepted* — so the whole suite ran on the safe side of the
/// boundary, and the case that mattered was never exercised. The tests below
/// configure Clerk, which is what production does, and assert at the boundary
/// rather than near it.
#[cfg(test)]
mod worker_credential {
    use super::*;

    /// A state with Clerk **configured** — the production shape. The secret is
    /// never used by any assertion here: no test in this module presents a JWT,
    /// so the JWKS fetch is never reached. It is set precisely so that the
    /// `clerk_secret_key.is_none()` shortcuts cannot be what makes a test pass.
    async fn state_with_clerk() -> std::sync::Arc<AppState> {
        state_with(Some("sk_test_worker_credential".to_string())).await
    }

    /// A state with Clerk unconfigured — the local-dev shape, and the one the
    /// old code treated as "let anybody in".
    async fn state_without_clerk() -> std::sync::Arc<AppState> {
        state_with(None).await
    }

    async fn state_with(clerk_secret_key: Option<String>) -> std::sync::Arc<AppState> {
        let temporary = tempfile::tempdir().expect("temporary workspace");
        let workspace = temporary.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".cortex")).expect("workspace metadata");
        let state = AppState::new(
            workspace.join(".cortex/ledger.jsonl"),
            workspace,
            clerk_secret_key,
        )
        .await;
        // The tempdir must outlive the state's open database handle.
        std::mem::forget(temporary);
        state
    }

    /// Mint a key and record it, returning the plaintext the worker would hold.
    ///
    /// `expires_at` is epoch milliseconds; `None` means "until revoked".
    fn issue(state: &AppState, owner: &str, expires_at: Option<i64>) -> String {
        let key = crate::worker_key::generate_worker_key();
        state
            .db
            .as_ref()
            .expect("database")
            .create_worker_key(
                &format!("wk-{}", &key.hash[..8]),
                &key.hash,
                &key.display_prefix,
                owner,
                crate::worker_key::DEFAULT_WORKER_SCOPE,
                expires_at,
            )
            .expect("issue worker key");
        key.secret
    }

    fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    /// The credential works, and it works in the configuration that matters:
    /// Clerk configured, so nothing in the JWT fallback can be what accepted it.
    #[tokio::test]
    async fn a_valid_worker_key_authenticates_with_clerk_configured() {
        let state = state_with_clerk().await;
        let secret = issue(&state, "user_alice", None);

        assert_eq!(
            authenticate_worker(&state, &secret).await.ok().as_deref(),
            Some("user_alice"),
            "a valid cwk_ key must resolve to its owner"
        );
    }

    /// The empty token is refused when Clerk is configured. This is the
    /// assertion the old suite could not make, because it never built this
    /// state.
    #[tokio::test]
    async fn an_empty_token_is_refused_with_clerk_configured() {
        let state = state_with_clerk().await;
        assert!(
            authenticate_worker(&state, "").await.is_err(),
            "an empty token must never authenticate a worker"
        );
    }

    /// **The security change.** With Clerk unconfigured and the escape hatch
    /// unset, an empty token is refused. This is the exact case production ran
    /// in: no `CLERK_SECRET_KEY`, and therefore — until this PR — any client
    /// at all could connect as `local` and be handed steps to execute.
    #[tokio::test]
    async fn an_empty_token_is_refused_when_clerk_is_unconfigured_and_the_flag_is_unset() {
        let state = state_without_clerk().await;
        assert!(
            !state.allow_anonymous_worker,
            "the anonymous worker path must be closed unless explicitly opened"
        );
        let outcome = authenticate_worker(&state, "").await;
        assert!(
            outcome.is_err(),
            "an unset CLERK_SECRET_KEY must not be sufficient to authenticate a \
             worker, got {outcome:?}"
        );
        assert_ne!(outcome.ok().as_deref(), Some("local"));
    }

    /// The same hole, reached through the other door: a non-empty token that is
    /// not a worker key used to be answered `Ok("local")` by the Clerk fallback
    /// when no secret was configured, without being verified at all.
    #[tokio::test]
    async fn an_unverifiable_token_is_refused_when_clerk_is_unconfigured() {
        let state = state_without_clerk().await;
        let outcome = authenticate_worker(&state, "eyJhbGciOiJSUzI1NiJ9.e30.sig").await;
        assert!(outcome.is_err(), "got {outcome:?}");
        assert_ne!(outcome.ok().as_deref(), Some("local"));
    }

    #[tokio::test]
    async fn a_revoked_key_is_refused() {
        let state = state_with_clerk().await;
        let secret = issue(&state, "user_bob", None);
        let db = state.db.as_ref().expect("database");

        // It authenticates before revocation — otherwise this test could pass
        // for the wrong reason.
        assert!(authenticate_worker(&state, &secret).await.is_ok());

        let hash = crate::worker_key::hash_worker_key(&secret);
        let prefix = crate::key_material::display_prefix(&secret);
        assert_eq!(db.revoke_worker_key(&prefix), 1);

        assert!(
            authenticate_worker(&state, &secret).await.is_err(),
            "a revoked key must not authenticate"
        );
        assert!(db.authenticate_worker_key(&hash, now_ms()).is_none());
        // Revocation is idempotent and does not move the recorded moment.
        assert_eq!(db.revoke_worker_key(&prefix), 0);
    }

    #[tokio::test]
    async fn an_expired_key_is_refused() {
        let state = state_with_clerk().await;
        let expired = issue(&state, "user_carol", Some(now_ms() - 60_000));

        assert!(
            authenticate_worker(&state, &expired).await.is_err(),
            "a key past its expiry must not authenticate"
        );

        // A key whose expiry is still in the future does authenticate, so the
        // assertion above is about expiry and not about `expires_at` being set.
        let live = issue(&state, "user_carol", Some(now_ms() + 3_600_000));
        assert_eq!(
            authenticate_worker(&state, &live).await.ok().as_deref(),
            Some("user_carol")
        );
    }

    /// A well-formed `cwk_` token that was never issued is refused, and is
    /// refused by the worker-key branch rather than falling through to Clerk.
    #[tokio::test]
    async fn an_unissued_worker_key_is_refused() {
        let state = state_with_clerk().await;
        let forged = crate::worker_key::generate_worker_key().secret;
        let outcome = authenticate_worker(&state, &forged).await;
        assert_eq!(outcome.err().as_deref(), Some("invalid worker key"));
    }

    /// The worker-key branch is checked before the empty-token branch and
    /// before the JWT fallback, so opening the anonymous hatch cannot shadow a
    /// real credential and cannot turn a bad key into a `local` session.
    #[tokio::test]
    async fn the_worker_key_branch_wins_even_with_the_anonymous_hatch_open() {
        let mut state = state_without_clerk().await;
        std::sync::Arc::get_mut(&mut state)
            .expect("sole owner of the state")
            .allow_anonymous_worker = true;

        let secret = issue(&state, "user_dave", None);
        assert_eq!(
            authenticate_worker(&state, &secret).await.ok().as_deref(),
            Some("user_dave"),
            "a real key must resolve to its owner, not to the anonymous user"
        );

        let forged = crate::worker_key::generate_worker_key().secret;
        let outcome = authenticate_worker(&state, &forged).await;
        assert!(outcome.is_err(), "got {outcome:?}");
        assert_ne!(
            outcome.ok().as_deref(),
            Some("local"),
            "an invalid worker key must not degrade into an anonymous session"
        );
    }

    /// Authentication stamps `last_used_at`, so an operator can tell a live
    /// credential from an abandoned one before revoking it.
    #[tokio::test]
    async fn a_successful_authentication_stamps_last_used_at() {
        let state = state_with_clerk().await;
        let secret = issue(&state, "user_erin", None);
        let db = state.db.as_ref().expect("database");

        let before = db.list_worker_keys(Some("user_erin"));
        assert_eq!(before.len(), 1);
        assert!(before[0]["lastUsedAt"].is_null());

        assert!(authenticate_worker(&state, &secret).await.is_ok());

        let after = db.list_worker_keys(Some("user_erin"));
        assert!(after[0]["lastUsedAt"].as_i64().is_some());
    }

    /// Nothing in the operator view carries key material beyond the
    /// non-secret display prefix.
    #[tokio::test]
    async fn the_operator_listing_never_carries_key_material() {
        let state = state_with_clerk().await;
        let secret = issue(&state, "user_frank", None);
        let hash = crate::worker_key::hash_worker_key(&secret);

        let listed = state
            .db
            .as_ref()
            .expect("database")
            .list_worker_keys(None)
            .iter()
            .map(|row| row.to_string())
            .collect::<String>();

        assert!(!listed.contains(&secret), "plaintext must never be listed");
        assert!(!listed.contains(&hash), "the hash must never be listed");
        assert!(listed.contains("user_frank"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_engine::verifier::{
        AcceptanceCoverage, AllowedPathViolation, CheckEvidence as EngineCheckEvidence,
        CheckFailure, CheckStatus as EngineCheckStatus, CheckSummary, CommandSummary,
        RequiredCheckSummary, VerifierFloorSummary, VerifierNextAction,
        VerifierReport as EngineVerifierReport,
    };

    fn rejected_report() -> EngineVerifierReport {
        EngineVerifierReport {
            verdict: VerifierVerdict::Failed,
            risk: RiskLevel::High,
            evidence_signals: Vec::new(),
            evidence_floor: VerifierFloorSummary {
                satisfied: false,
                signals_met: Vec::new(),
                missing: vec!["independent verification".into()],
            },
            allowed_path_violations: Vec::new(),
            command_summary: CommandSummary {
                total: 0,
                succeeded: 0,
                failed: 0,
                unknown: 0,
                failures: Vec::new(),
            },
            check_summary: CheckSummary {
                total: 0,
                passed: 0,
                failed: 0,
                skipped: 0,
                unknown: 0,
                failures: Vec::new(),
            },
            required_check_summary: RequiredCheckSummary::default(),
            acceptance_coverage: AcceptanceCoverage {
                evaluated: false,
                total: 0,
                covered: 0,
                uncovered: Vec::new(),
                note: "not evaluated".into(),
            },
            stale_base_notes: Vec::new(),
            next_action: VerifierNextAction::AddEvidence,
        }
    }

    #[test]
    fn verifier_diagnostic_includes_missing_evidence_and_next_action() {
        let report = rejected_report();

        let diagnostic = verifier_diagnostic("needs_evidence", "needs_evidence", &report);

        assert!(diagnostic.contains("next_action=AddEvidence"));
        assert!(diagnostic.contains("missing evidence: independent verification"));
    }

    #[test]
    fn verifier_diagnostic_includes_path_and_required_check_failures() {
        let mut report = rejected_report();
        report.allowed_path_violations.push(AllowedPathViolation {
            path: "docs/readme.md".into(),
            reason: "changed path is outside the allowed path contract".into(),
        });
        report
            .required_check_summary
            .failed
            .push(EngineCheckEvidence {
                name: "npm:build".into(),
                status: EngineCheckStatus::Failed,
                summary: Some("exit code 1".into()),
            });
        report.check_summary.failures.push(CheckFailure {
            name: "npm:build".into(),
            summary: Some("exit code 1".into()),
        });

        let diagnostic = verifier_diagnostic("verified", "fail", &report);

        assert!(diagnostic.contains("path violations: docs/readme.md"));
        assert!(diagnostic.contains("failed required checks: npm:build (exit code 1)"));
        assert!(diagnostic.contains("check failures: npm:build (exit code 1)"));
    }

    #[test]
    fn truncate_diagnostic_caps_long_messages() {
        let diagnostic = truncate_diagnostic("abcdef", 3);

        assert_eq!(diagnostic, "abc...");
    }
}
