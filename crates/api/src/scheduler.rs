use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use cortex_core::check_derivation::{derive_checks, DerivationInput};
use cortex_core::egress::{derive_egress, derive_provider_egress, EgressPlan};
use cortex_core::evaluator::{
    token_budget, AutoMode, BudgetEvidence, CandidateScore, DecisionEvidence, DefaultPolicy,
    IntentEvidence, PressureState, Profile, ProviderFitEvidence, RiskEvidence, WINDOW_SECS,
};
use cortex_core::protocol::{BrainMessage, PredecessorSummary, StepContext};
use cortex_core::provider::{ProviderId, Tier};
use cortex_core::routing::{Intent, RiskLevel, RoutingDecision};
use cortex_core::task::{
    AcceptanceCriterion, AcceptanceVerification, RequiredCheck, WorkKind, WorkRecipe,
    WorkRecipeSeed,
};
use cortex_engine::captain::{
    check_run_completion, plan_heal, EdgeType, RunStatus, SchedulerEvent, SchedulerState, StepKind,
    StepRef, StepStatus,
};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::db::{Database, ResourceLeaseRequest};
use crate::mission_control::MissionControlEvent;
use crate::state::AppState;

pub type SchedulerTx = mpsc::Sender<SchedulerEvent>;

pub fn spawn_scheduler(state: Arc<AppState>) -> SchedulerTx {
    let (tx, rx) = mpsc::channel::<SchedulerEvent>(256);

    tokio::spawn(async move {
        scheduler_loop(state, rx).await;
    });

    tx
}

async fn scheduler_loop(state: Arc<AppState>, mut rx: mpsc::Receiver<SchedulerEvent>) {
    let mut sched = SchedulerState::new();
    let mut reconcile_interval = tokio::time::interval(Duration::from_secs(30));
    let mut prune_interval = tokio::time::interval(Duration::from_secs(600));

    tracing::info!("scheduler started");

    recover_from_db(&state, &mut sched).await;

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                apply_event(&state, &mut sched, &event).await;
                schedule_until_blocked(&state, &mut sched).await;
            }
            _ = reconcile_interval.tick() => {
                expire_stale_leases(&state, &mut sched).await;
                expire_stale_resource_leases(&state).await;
                expire_grace_periods(&state, &mut sched).await;
                cleanup_expired_keys(&state);
                state.rate_limiter.cleanup();
                reconcile_ready_steps(&state, &mut sched).await;
                reconcile_run_completion(&state).await;
                schedule_until_blocked(&state, &mut sched).await;
            }
            _ = prune_interval.tick() => {
                // Prune spend logs for delegations inactive for 48h (2x default session TTL)
                #[cfg(feature = "soma")]
                if let Some(heart) = &state.soma_heart {
                    heart.prune_spend_logs(48 * 3600 * 1000);
                }
            }
        }
    }
}

async fn apply_event(state: &AppState, sched: &mut SchedulerState, event: &SchedulerEvent) {
    match event {
        SchedulerEvent::RunCreated { run_id } => {
            tracing::info!("scheduler: run created {run_id}");
            if let Some(db) = &state.db {
                db.update_run_status(run_id, "running", None);

                // Emit MC event for run creation
                let user_id = get_run_user(db, run_id);
                let goal = db.get_run_goal(run_id).unwrap_or_default();
                let step_count = db.get_all_step_statuses(run_id).len();
                state
                    .emit_mc_event(
                        &user_id,
                        MissionControlEvent::RunCreated {
                            run_id: run_id.clone(),
                            goal,
                            step_count,
                        },
                    )
                    .await;
            }
            load_ready_steps_for_run(state, sched, run_id).await;
        }

        SchedulerEvent::StepDelivered {
            run_id,
            step_id,
            cost_estimate,
        } => {
            tracing::info!("scheduler: step delivered {step_id} in run {run_id}");
            #[cfg(feature = "soma")]
            if let Some(heart) = &state.soma_heart {
                heart.record_heartbeat(
                    soma::heartbeat::HeartbeatEventType::RouteCompleted,
                    &serde_json::json!({"step_id": step_id, "run_id": run_id, "cost": cost_estimate}).to_string(),
                );
            }
            if let Some(db) = &state.db {
                let user_id = get_run_user(db, run_id);
                // The worker is free again regardless of what the verdict turns
                // out to be, so the concurrency slot is released here.
                sched.mark_step_done(&user_id);

                // Invariant 6: a worker's report can never emit a positive
                // routing reward. At delivery the step is `verifying` and no
                // verdict exists, so this gate is expected to reject — it fires
                // only for a step an operator has already accepted. The
                // reward for a genuine verdict belongs to the durable verifier
                // (PR B), which is the first thing in this system that both
                // knows a verdict and can reach the bandit.
                let accepted = db
                    .get_step_status(step_id)
                    .map(|status| matches!(status.as_str(), "verified" | "manual_override"))
                    .unwrap_or(false);
                if !accepted {
                    tracing::info!(
                        step_id = %step_id,
                        "no positive bandit reward: the step is delivered, not verified"
                    );
                } else {
                    match db.get_latest_verifier_report(step_id) {
                        Some(report) if report.is_verified_success() => {
                            update_bandit_from_outcome(state, db, step_id, true, *cost_estimate)
                                .await;
                        }
                        Some(report) => {
                            tracing::info!(
                                step_id = %step_id,
                                report_id = %report.id,
                                status = %report.status,
                                verdict = %report.verdict,
                                "skipping positive bandit reward for unverified step delivery"
                            );
                        }
                        None => {
                            tracing::info!(
                                step_id = %step_id,
                                "skipping positive bandit reward for delivery without verifier report"
                            );
                        }
                    }
                }
            }
            load_ready_steps_for_run(state, sched, run_id).await;
            check_run_done(state, run_id).await;
        }

        SchedulerEvent::StepFailed { run_id, step_id } => {
            tracing::info!("scheduler: step failed {step_id} in run {run_id}");
            #[cfg(feature = "soma")]
            if let Some(heart) = &state.soma_heart {
                heart.record_heartbeat(
                    soma::heartbeat::HeartbeatEventType::RouteFailed,
                    &serde_json::json!({"step_id": step_id, "run_id": run_id}).to_string(),
                );
            }
            if let Some(db) = &state.db {
                let user_id = get_run_user(db, run_id);
                sched.mark_step_done(&user_id);
                update_bandit_from_outcome(state, db, step_id, false, None).await;

                // Check for auth-related failures — these are not healable
                let last_error = db.get_step_last_error(step_id);
                let is_auth_failure = last_error
                    .as_deref()
                    .map(|e| e.contains("CliNotAuthenticated") || e.contains("CliAuthExpired"))
                    .unwrap_or(false);

                let status = db
                    .get_all_step_statuses(run_id)
                    .into_iter()
                    .find_map(|(id, status)| (id == *step_id).then_some(status));
                let is_cancelled = status.as_deref() == Some("cancelled");
                let is_no_provider = last_error.as_deref() == Some("no available provider");

                if is_cancelled {
                    tracing::info!("skipping heal for step {step_id} — step was cancelled");
                } else if is_auth_failure || is_no_provider {
                    tracing::warn!(
                        "skipping heal for step {step_id} — non-healable dispatch failure, cascading"
                    );
                    let skipped = db.cascade_failure(step_id);
                    if !skipped.is_empty() {
                        tracing::info!(
                            "cascaded failure from step {step_id}: skipped {} downstream steps",
                            skipped.len()
                        );
                    }
                } else {
                    try_heal(state, sched, run_id, step_id).await;
                }
            }
            load_ready_steps_for_run(state, sched, run_id).await;
            check_run_done(state, run_id).await;
        }

        SchedulerEvent::WorkerConnected { worker_id } => {
            tracing::info!("scheduler: worker connected {worker_id}");
        }

        SchedulerEvent::WorkerDisconnected { worker_id } => {
            tracing::info!("scheduler: worker disconnected {worker_id}");
        }

        SchedulerEvent::ProviderAuthExpired {
            worker_id,
            provider,
            user_id,
        } => {
            tracing::warn!(
                "scheduler: provider {provider} auth expired for worker {worker_id} — \
                 user {user_id} needs to re-authenticate"
            );

            // Resolve provider string to ProviderId and mark unhealthy
            if let Some(provider_id) = parse_provider_id(provider) {
                state.mark_provider_unhealthy(worker_id, provider_id).await;
            }
        }

        SchedulerEvent::Reconcile => {
            tracing::debug!("scheduler: reconcile tick");
        }
    }
}

async fn schedule_until_blocked(state: &AppState, sched: &mut SchedulerState) {
    while let Some(step) = sched.next_assignable() {
        match dispatch_step(state, &step).await {
            DispatchOutcome::Dispatched => {}
            DispatchOutcome::RetryLater => {
                sched.mark_step_done(&step.user_id);
                sched.enqueue_ready_step(step);
                break;
            }
            DispatchOutcome::WaitingForApproval => {
                sched.mark_step_done(&step.user_id);
                break;
            }
        }
    }
}

enum DispatchOutcome {
    Dispatched,
    RetryLater,
    WaitingForApproval,
}

async fn dispatch_step(state: &AppState, step: &StepRef) -> DispatchOutcome {
    let (worker_id, worker_tx) = match state.find_worker_for_user(&step.user_id).await {
        Some(pair) => pair,
        None => {
            tracing::warn!(
                "no worker for user {} — step {} stays queued",
                step.user_id,
                step.step_id
            );
            return DispatchOutcome::RetryLater;
        }
    };

    let db = match &state.db {
        Some(db) => db,
        None => return DispatchOutcome::RetryLater,
    };

    // --- Billing gate check ---
    let gate = crate::billing::check_usage_gate(
        db,
        &step.user_id,
        &state.usage_limits,
        state.billing_enforced,
    );
    if !gate.allowed {
        tracing::warn!(
            "billing gate blocked step {} for user {} — {:?} (step stays pending, will retry next tick)",
            step.step_id,
            step.user_id,
            gate.violation
        );
        return DispatchOutcome::RetryLater;
    }

    // --- Path leases, at step scope ---
    //
    // Acquired here rather than at run creation. A run-scoped lease is held
    // from creation until the run finishes, so two runs touching the same
    // directory serialise end to end even when only one step in each writes
    // there. Holding at step scope shortens that to the step.
    //
    // A conflict is not a failure. The step stays pending and is retried next
    // tick, exactly as it does for a missing worker or a closed billing gate —
    // which is queue-on-conflict using the mechanism already here, rather than
    // a new waiting state to keep correct.
    let write_set = cortex_core::write_set::derive_write_set(
        step_changes_the_tree(step.kind),
        &db.get_step_target_paths(&step.step_id).unwrap_or_default(),
        &[],
    );
    let mut lease_keys = write_set.lease_keys();
    cortex_core::write_set::canonical_order(&mut lease_keys);

    if !lease_keys.is_empty() {
        let repo_key = normalize_repo_key(db.get_run_repo_key(&step.run_id).as_deref());
        if let Err(conflict) = db.acquire_step_path_leases(
            &step.user_id,
            &step.run_id,
            &step.step_id,
            &repo_key,
            &lease_keys,
        ) {
            tracing::info!(
                step_id = %step.step_id,
                path = %conflict.resource_key,
                held_by_step = ?conflict.step_id,
                "path is leased by another step — waiting, will retry next tick"
            );
            return DispatchOutcome::RetryLater;
        }
    }

    // --- Build evidence and route through evaluator ---
    let tier = parse_tier(&step.tier);
    let risk = parse_risk(&step.risk);

    let ucb_guard = state.ucb_scorer.read().await;
    let (decision, evidence) = route_step(db, &step.user_id, step, tier, risk, Some(&*ucb_guard));
    drop(ucb_guard);

    // --- Evidence floor check ---
    // For high/critical risk, the evidence floor may block dispatch until
    // sufficient clean evidence is gathered. Currently signals are empty at
    // dispatch time (evidence accumulates after execution), so we log a warning
    // for high-risk steps to ensure observability.
    {
        use cortex_engine::evidence_floor::{check_floor, FloorVerdict};
        let verdict = check_floor(risk, &[]);
        match verdict {
            FloorVerdict::Blocked { missing, .. } => {
                tracing::info!(
                    "evidence floor for step {} ({:?} risk): {} requirement(s) must be satisfied post-execution: {}",
                    step.step_id,
                    risk,
                    missing.len(),
                    missing.join(", "),
                );
            }
            FloorVerdict::Satisfied { .. } => {}
        }
    }

    // --- Autonomy gate ---
    // Determine if this step can proceed autonomously or needs user approval.
    {
        let confidence = decision.score / 100.0; // normalize evaluator score to 0-1
        let is_first = state
            .cortex_store
            .as_ref()
            .and_then(|s| s.lock().ok())
            .and_then(|s| s.event_count().ok())
            .map(|c| c == 0)
            .unwrap_or(false);
        let autonomy =
            cortex_core::autonomy::decide_autonomy(confidence.clamp(0.0, 1.0), risk, is_first);
        if autonomy.requires_user_input() && risk.requires_approval() {
            const ASK_TYPE: &str = "autonomy.dispatch";
            match db.latest_cortex_step_approval_status(&step.user_id, &step.step_id, ASK_TYPE) {
                Some((approval_id, status)) if status == "approved" => {
                    tracing::info!(
                        "autonomy gate: step {} has approved dispatch approval {}",
                        step.step_id,
                        approval_id,
                    );
                }
                Some((approval_id, status))
                    if matches!(status.as_str(), "rejected" | "cancelled") =>
                {
                    tracing::warn!(
                        "autonomy gate: step {} was terminally blocked by approval {} with status {}",
                        step.step_id,
                        approval_id,
                        status,
                    );
                    if db.fail_unleased_step(
                        &step.step_id,
                        "approval rejected",
                        Some("ApprovalRejected"),
                    ) {
                        state
                            .emit_scheduler_event(SchedulerEvent::StepFailed {
                                run_id: step.run_id.clone(),
                                step_id: step.step_id.clone(),
                            })
                            .await;
                    }
                    return DispatchOutcome::Dispatched;
                }
                Some((approval_id, status)) if status == "pending" => {
                    tracing::warn!(
                        "autonomy gate: step {} is waiting for approval {} ({:?} risk, confidence={:.2})",
                        step.step_id,
                        approval_id,
                        risk,
                        confidence,
                    );
                    return DispatchOutcome::WaitingForApproval;
                }
                _ => {
                    let approval = db.ensure_cortex_step_approval_request(
                        &step.user_id,
                        &step.step_id,
                        ASK_TYPE,
                        "Approve risky Cortex dispatch",
                        &format!(
                            "Cortex wants to dispatch a {:?} risk step: {}",
                            risk, step.objective
                        ),
                        if risk >= RiskLevel::Critical {
                            "urgent"
                        } else {
                            "high"
                        },
                        "scheduler-risk-gate",
                    );
                    match approval {
                        Some(request) => {
                            tracing::warn!(
                                "autonomy gate: step {} created approval {} and is waiting ({:?} risk, confidence={:.2})",
                                step.step_id,
                                request.id,
                                risk,
                                confidence,
                            );
                        }
                        None => {
                            tracing::warn!(
                                "autonomy gate: step {} needs approval but is not bound to a Cortex group; dispatch blocked",
                                step.step_id,
                            );
                        }
                    }
                    return DispatchOutcome::WaitingForApproval;
                }
            }
        } else if autonomy.requires_user_input() {
            tracing::warn!(
                "autonomy gate: step {} requires explicit approval ({:?} risk, confidence={:.2}) but risk does not require a blocking dispatch gate",
                step.step_id,
                risk,
                confidence,
            );
        }
    }

    // --- Soma heartbeat: record routing decision ---
    // Observation only: the heartbeat records the decision, it never makes it.
    // The decision above is already final by this point.
    #[cfg(feature = "soma")]
    if let Some(heart) = &state.soma_heart {
        heart.record_heartbeat(
            soma::heartbeat::HeartbeatEventType::RouteSelected,
            &serde_json::json!({
                "step_id": step.step_id,
                "provider": decision.provider.to_string(),
                "model": decision.model_id,
                "risk": format!("{:?}", risk),
                "tier": format!("{:?}", tier),
                "score": decision.score,
            })
            .to_string(),
        );
    }

    // Don't dispatch if no provider is actually available
    if decision
        .rationale
        .iter()
        .any(|r| matches!(r, cortex_core::routing::RationaleCode::ProviderUnavailable))
    {
        tracing::warn!(
            "no available provider for step {} — all candidates vetoed",
            step.step_id
        );
        if db.fail_unleased_step(&step.step_id, "no available provider", Some("NoProvider")) {
            state
                .emit_scheduler_event(SchedulerEvent::StepFailed {
                    run_id: step.run_id.clone(),
                    step_id: step.step_id.clone(),
                })
                .await;
            return DispatchOutcome::Dispatched;
        }
        return DispatchOutcome::RetryLater;
    }

    let attempt_id = Uuid::new_v4().to_string();
    let lease_duration = step.kind.lease_duration_ms();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let deadline = now_ms + lease_duration;

    // The worker this step is being dispatched to, not the literal string
    // `"scheduler"`.
    //
    // `steps.assigned_worker` is a foreign key onto `workers(id)` and no worker
    // is ever called "scheduler", so with `PRAGMA foreign_keys = ON` this
    // statement raised `FOREIGN KEY constraint failed` on **every** dispatch —
    // and `lease_step` swallowed the error and returned `None`, which is
    // indistinguishable from losing the CAS. So the scheduler logged "CAS lease
    // failed — skipping", waited for the next reconcile tick, and did it again,
    // forever. **No step could ever be leased, so none could ever be
    // dispatched.**
    //
    // This sits upstream of F7: the step never reached a worker, let alone a
    // sandbox. It was found by wave 4 / Task 4, which is the first thing that
    // ever asked a step to actually run.
    let lease_gen = match db.lease_step(&step.step_id, &worker_id, deadline) {
        Some(g) => g,
        None => {
            tracing::warn!(
                step_id = %step.step_id,
                worker_id = %worker_id,
                "CAS lease failed for step — skipping. If this repeats every tick \
                 for the same step, check the error log: `lease_step` reports a \
                 database failure separately, because it is not the same thing."
            );
            return DispatchOutcome::RetryLater;
        }
    };

    // Record attempt
    db.record_attempt(
        &step.step_id,
        &step.run_id,
        1,
        "scheduler",
        lease_gen,
        Some(&decision.provider.to_string()),
        Some(&decision.model_id),
    );

    // Record decision + evidence
    let decision_id = Uuid::new_v4().to_string();
    let rationale_str = decision
        .rationale
        .iter()
        .map(|r| format!("{:?}", r))
        .collect::<Vec<_>>()
        .join(", ");

    db.record_decision(
        &decision_id,
        &step.user_id,
        Some(&step.run_id),
        Some(&step.step_id),
        &format!("{:?}", evidence.intent.intent),
        &step.risk,
        &step.tier,
        &decision.provider.to_string(),
        &decision.model_id,
        None,
        &rationale_str,
        "auto",
    );

    // Record score evidence per evaluator
    record_evidence(db, &decision_id, &evidence);

    // Build step context from predecessors, then orient it in the repository.
    let context = build_step_context(db, &step.run_id, &step.step_id);
    let context = with_repo_map(context, state, decision.provider, decision.tier);

    // Workspace context: use run_id as logical workspace, look up latest commit
    // from predecessor steps, and pass file_paths from the run's goal
    let base_commit = db.get_run_latest_commit(&step.run_id);
    let allowed_paths = db.get_run_file_paths(&step.run_id);
    let planner_seed = db
        .get_step_recipe_seed_json(&step.step_id)
        .and_then(|raw| serde_json::from_str::<WorkRecipeSeed>(&raw).ok());

    // Build and persist the dispatch-time work contract before handing work to a worker.
    let mut task = cortex_core::task::TaskContract::new(step.objective.clone(), tier, risk)
        .with_dispatch_contract(allowed_paths.clone(), base_commit.clone());
    // Derive once, then use the result twice: frozen for verification, and
    // downgraded to display strings for the worker contract. Deriving twice
    // would risk the exam differing from the one the worker was shown.
    let check_specs =
        derive_step_check_specs(step.kind, risk, &allowed_paths, &state.workspace_dir);

    // Freeze the exam here, at dispatch, before the worker sees the task.
    // Verification happens after delivery, and the `CheckSpec` argv needed to
    // run it does not survive the downgrade to `RequiredCheck` below — so if
    // it is not persisted now it is gone.
    // Only for steps that change trees. An empty set means this step is not
    // one verification attaches to, and freezing an empty row would make it
    // indistinguishable from a step whose checks failed to derive.
    if !check_specs.is_empty() {
        if let Err(e) = db.save_check_specs(&step.run_id, &step.step_id, &check_specs) {
            tracing::warn!(
                run_id = %step.run_id,
                step_id = %step.step_id,
                error = %e,
                "could not freeze check specs; this step will not be verified"
            );
        }
    }

    // Freeze the price here too, and for the same reason the exam is frozen
    // here: a price resolved after execution is a price the work could have
    // influenced, and a customer quoted before dispatch has to be charged what
    // they were quoted.
    //
    // The class carries `verifiable`, taken from the exam that was just frozen
    // rather than from an intention — invariant 22 says work with no executable
    // ground truth is never priced as though it had been proven, and the only
    // honest source for "was there executable ground truth" is the check set
    // itself.
    freeze_step_quote(
        db,
        &step.run_id,
        &step.step_id,
        step.work_kind
            .unwrap_or_else(|| work_kind_for_step(step.kind, &step.objective)),
        risk,
        !cortex_core::check_derivation::is_unverified_by_construction(&check_specs),
    );

    task.required_checks = check_specs.iter().map(as_required_check).collect();
    let recipe = build_work_recipe(
        step.work_kind
            .unwrap_or_else(|| work_kind_for_step(step.kind, &step.objective)),
        &step.objective,
        risk,
        tier,
        &allowed_paths,
        base_commit.as_deref(),
        &task.required_checks,
        planner_seed.as_ref(),
    );
    task.acceptance_criteria = recipe
        .acceptance
        .iter()
        .map(|criterion| criterion.text.clone())
        .collect();
    task = task.with_work_recipe(recipe);
    if !db.record_step_work_contract(&step.step_id, &step.run_id, lease_gen, &task) {
        tracing::error!(
            "failed to persist work contract for step {} lease {}; unleasing before dispatch",
            step.step_id,
            lease_gen
        );
        db.unlease_step(&step.step_id, lease_gen);
        return DispatchOutcome::RetryLater;
    }

    // Capture values before decision is moved into msg
    let mc_provider = decision.provider.to_string();
    let mc_model = decision.model_id.clone();
    let mc_pressure = evidence.budget.pressure_for(decision.provider, tier);

    // Issue a step-scoped sub-delegation from Cortex's heart to the worker
    let step_delegation = issue_step_delegation(state, &step.step_id, deadline, &worker_id);

    // What this step may reach, decided here and only here.
    //
    // Derived from the same two facts the check floor is derived from — the
    // repository on disk and the kind of step — so a grant is justified by
    // something the task did not author. The task contract has no say: an
    // objective that says "install the dependencies" does not open npm; a
    // `package.json` does.
    let egress = derive_step_egress(step.kind, &state.workspace_dir);
    if !egress.is_deny() {
        tracing::info!(
            step_id = %step.step_id,
            registries = ?egress.granted_registries(),
            hosts = ?egress.network_policy.allowed_hosts(),
            "granting scoped egress for this step"
        );
    }

    // What the *routing decision* justifies, derived separately and from a
    // different input: the provider the router already chose. This is what
    // lets the CLI inside the sandbox reach the model at all — without it a
    // sandboxed step runs an agent with no route to any API, which is F7.
    //
    // Deliberately not folded into `derive_step_egress`. That function reads
    // the repository, and nothing a customer can put in a repository may
    // influence which provider is reachable.
    let provider_egress = derive_provider_egress(decision.provider);
    let provider_gateway = crate::provider_gateway_http::issue_stub_access(
        db,
        &step.user_id,
        &step.run_id,
        &attempt_id,
        decision.provider,
        &decision.model_id,
        deadline,
        chrono::Utc::now().timestamp_millis(),
    );
    if provider_egress.is_deny() {
        // Every `ProviderId` has an endpoint, so this is unreachable today. It
        // is a log line rather than an assert because the honest failure is a
        // step that cannot reach its model and says so, not a panicking
        // scheduler.
        tracing::error!(
            step_id = %step.step_id,
            provider = %decision.provider,
            "no endpoint is known for the routed provider; the sandbox will \
             have no route to the model API"
        );
    } else {
        tracing::info!(
            step_id = %step.step_id,
            provider = ?provider_egress.granted_provider(),
            hosts = ?provider_egress.network_policy.allowed_hosts(),
            "granting provider egress for this step"
        );
    }

    let msg = BrainMessage::ExecuteStep {
        run_id: step.run_id.clone(),
        step_id: step.step_id.clone(),
        attempt_id,
        lease_gen,
        lease_deadline_ms: deadline,
        workspace_id: step.run_id.clone(),
        base_commit,
        allowed_paths,
        task,
        decision,
        context,
        egress,
        provider_egress,
        provider_gateway,
        delegation: step_delegation,
    };

    match worker_tx.send(msg).await {
        Ok(()) => {
            tracing::info!(
                "dispatched step {} to worker for user {}",
                step.step_id,
                step.user_id
            );

            // Emit MC events for dispatch and routing decision
            state
                .emit_mc_event(
                    &step.user_id,
                    MissionControlEvent::StepDispatched {
                        run_id: step.run_id.clone(),
                        step_id: step.step_id.clone(),
                        provider: mc_provider.clone(),
                        model: mc_model.clone(),
                        tier: step.tier.clone(),
                    },
                )
                .await;
            state
                .emit_mc_event(
                    &step.user_id,
                    MissionControlEvent::RoutingDecision {
                        step_id: step.step_id.clone(),
                        provider: mc_provider,
                        model: mc_model,
                        rationale: rationale_str,
                        pressure: mc_pressure,
                    },
                )
                .await;

            DispatchOutcome::Dispatched
        }
        Err(_) => {
            tracing::error!(
                "worker channel closed for user {} — unleasing step {}",
                step.user_id,
                step.step_id
            );
            db.unlease_step(&step.step_id, lease_gen);
            DispatchOutcome::RetryLater
        }
    }
}

/// Whether this kind of step can change the delivered tree.
///
/// Verification attaches to steps that change trees; gating read-only work on
/// it would spend sandbox time to prove nothing (VERIFIER.md, "what not to
/// do"). This is the *only* remaining reason to derive no checks — risk level
/// is no longer one, which is the entire point of V2.
fn step_changes_the_tree(kind: StepKind) -> bool {
    match kind {
        StepKind::Execute | StepKind::Test | StepKind::Build | StepKind::Lint | StepKind::Heal => {
            true
        }
        StepKind::Search | StepKind::Think | StepKind::Review | StepKind::Gate => false,
    }
}

/// Derive the egress this step justifies.
///
/// Same shape as [`derive_step_check_specs`] and for the same reason: every
/// rule lives in a pure function in `cortex_core` that is unit tested without a
/// disk, and this is the thin piece that needs a filesystem.
///
/// [`step_changes_the_tree`] is reused rather than re-expressed. A step that
/// cannot change the tree cannot have resolved a dependency into it, so the two
/// questions — "does verification attach here" and "may this reach a registry"
/// — happen to have the same answer, and writing the predicate twice is how
/// they would stop having it.
fn derive_step_egress(kind: StepKind, workspace_dir: &Path) -> EgressPlan {
    derive_egress(
        &crate::ecosystem_probe::probe_manifests(workspace_dir),
        step_changes_the_tree(kind),
    )
}

/// Derive the required checks for a step.
///
/// Delegates every rule to `cortex_core::check_derivation`, which is pure and
/// unit tested. What used to live here — an intent switch that returned an
/// empty vec for any Execute step below High risk, and a single check chosen
/// by sniffing file extensions — is gone. That rule is why "verified" has so
/// far meant "the CLI exited 0, says the CLI".
///
/// The returned [`RequiredCheck`] is the wire shape the worker contract
/// already speaks. `CheckSpec` carries argv; `RequiredCheck.command` is a
/// display string, so the two are joined here and nowhere else — execution
/// uses the argv, never this string.
fn derive_step_check_specs(
    kind: StepKind,
    risk: RiskLevel,
    allowed_paths: &[String],
    workspace_dir: &Path,
) -> Vec<cortex_core::verification::CheckSpec> {
    if !step_changes_the_tree(kind) {
        return Vec::new();
    }

    let input = DerivationInput {
        facts: crate::ecosystem_probe::probe_ecosystem(workspace_dir),
        contract_commands: Vec::new(),
        risk,
        has_allowed_paths: !allowed_paths.is_empty(),
    };

    derive_checks(&input)
}

/// Freeze what this step costs, before it runs.
///
/// Every failure here is a warning and no quote, never a guessed price. That is
/// the same rule `verification_driver` already follows at the other end — a
/// billable verdict with no reachable quote records the verdict and leaves the
/// ledger alone — and the two halves have to agree, because a price invented on
/// either side is a customer charged for a number nobody published.
fn freeze_step_quote(
    db: &crate::db::Database,
    run_id: &str,
    step_id: &str,
    work_kind: WorkKind,
    risk: RiskLevel,
    verifiable: bool,
) {
    let Some(list) = db.active_price_list() else {
        tracing::warn!(
            run_id,
            step_id,
            "no price list is published; this step is dispatched without a quote and \
             cannot be charged for"
        );
        return;
    };

    let class = cortex_core::task_class::TaskClass::new(work_kind, risk, verifiable);
    let Some((credits, billable)) = crate::pricing::quote(&list, &class) else {
        tracing::warn!(
            run_id, step_id, class = %class.key(), price_list_version = list.version,
            "the published price list does not price this class; dispatched without a quote"
        );
        return;
    };

    let quote = crate::pricing::StepQuote {
        quote_id: uuid::Uuid::new_v4().to_string(),
        run_id: run_id.to_string(),
        step_id: step_id.to_string(),
        task_class: class.key(),
        quoted_credits: credits,
        price_list_id: list.id.clone(),
        price_list_version: list.version,
        // Decided now and stored, never recomputed at verdict time. A class
        // that graduates between dispatch and verdict must not retroactively
        // make a step billable that the customer was told was free.
        billable,
        frozen_at: chrono::Utc::now().timestamp(),
    };

    if let Err(e) = db.freeze_step_quote(&quote) {
        tracing::warn!(
            run_id, step_id, error = %e,
            "could not freeze the quote; this step cannot be charged for"
        );
        return;
    }

    tracing::info!(
        run_id, step_id,
        class = %class.key(),
        credits,
        billable,
        price_list_version = list.version,
        "quote frozen at dispatch"
    );
}

/// Downgrade a spec to the wire shape the worker contract speaks.
///
/// Lossy on purpose and lossy in one place: `command` becomes a display
/// string, and `source`/`timeout_secs` are dropped. Execution uses the argv
/// off the frozen `CheckSpec`, never this string.
fn as_required_check(spec: &cortex_core::verification::CheckSpec) -> RequiredCheck {
    RequiredCheck {
        name: spec.id.clone(),
        command: spec.command.join(" "),
        required: spec.required,
    }
}

/// The `RequiredCheck` shape of the derived specs.
///
/// `#[cfg(test)]` because only tests call it: production derives
/// `CheckSpec`s and freezes those, and converting them back to
/// `RequiredCheck` is something only the assertions want. It was plain dead
/// code to `cargo clippy` on a non-test build, which is a different claim.
#[cfg(test)]
fn infer_required_checks(
    kind: StepKind,
    risk: RiskLevel,
    allowed_paths: &[String],
    workspace_dir: &Path,
) -> Vec<RequiredCheck> {
    derive_step_check_specs(kind, risk, allowed_paths, workspace_dir)
        .iter()
        .map(as_required_check)
        .collect()
}

fn build_work_recipe(
    work_kind: WorkKind,
    objective: &str,
    risk: RiskLevel,
    tier: Tier,
    allowed_paths: &[String],
    expected_base_commit: Option<&str>,
    required_checks: &[RequiredCheck],
    planner_seed: Option<&WorkRecipeSeed>,
) -> WorkRecipe {
    let mut constraints = vec![
        format!("risk={risk:?}"),
        format!("tier={tier:?}"),
        "Do not change files outside the allowed path set unless explicitly required by the task"
            .to_string(),
    ];
    if let Some(base) = expected_base_commit {
        constraints.push(format!("expected_base_commit={base}"));
    }
    if let Some(seed) = planner_seed {
        constraints.extend(seed.constraints.iter().cloned());
    }

    let acceptance = if !required_checks.is_empty() {
        required_checks
            .iter()
            .map(|check| AcceptanceCriterion {
                id: format!("required-check-{}", check.name),
                text: format!("Required check `{}` passes", check.name),
                verification: AcceptanceVerification::RequiredCheck {
                    check_name: check.name.clone(),
                },
            })
            .collect()
    } else if let Some(seed) = planner_seed.filter(|seed| !seed.acceptance.is_empty()) {
        seed.acceptance.clone()
    } else {
        vec![AcceptanceCriterion {
            id: "manual-objective-satisfied".to_string(),
            text: format!(
                "{} work satisfies the objective without violating the dispatch constraints",
                work_kind.as_str()
            ),
            verification: AcceptanceVerification::Manual,
        }]
    };
    let target_paths = planner_seed
        .filter(|seed| !seed.target_paths.is_empty())
        .map(|seed| seed.target_paths.clone())
        .unwrap_or_else(|| allowed_paths.to_vec());

    WorkRecipe {
        version: 1,
        kind: work_kind,
        objective: objective.to_string(),
        target_paths,
        required_checks: required_checks.to_vec(),
        acceptance,
        constraints,
    }
}

fn work_kind_for_step(kind: StepKind, objective: &str) -> WorkKind {
    match kind {
        StepKind::Search | StepKind::Think => WorkKind::Explore,
        StepKind::Test => WorkKind::Test,
        StepKind::Build => WorkKind::Build,
        StepKind::Lint => WorkKind::Lint,
        StepKind::Review => WorkKind::Review,
        StepKind::Heal => WorkKind::Heal,
        StepKind::Gate => WorkKind::Gate,
        StepKind::Execute => infer_execute_work_kind(objective),
    }
}

fn infer_execute_work_kind(objective: &str) -> WorkKind {
    let lower = objective.to_ascii_lowercase();
    if lower.contains("ship")
        || lower.contains("deploy")
        || lower.contains("release")
        || lower.contains("pull request")
        || lower.contains(" pr")
    {
        WorkKind::Ship
    } else if lower.contains("refactor")
        || lower.contains("restructure")
        || lower.contains("rework")
    {
        WorkKind::Refactor
    } else if lower.contains("add ")
        || lower.contains("create ")
        || lower.contains("implement ")
        || lower.contains("new ")
    {
        WorkKind::Add
    } else {
        WorkKind::Modify
    }
}

// --- Evaluator integration ---

fn route_step(
    db: &Database,
    user_id: &str,
    step: &StepRef,
    tier: Tier,
    risk: RiskLevel,
    ucb_scorer: Option<&cortex_engine::bandit::UcbScorer>,
) -> (RoutingDecision, DecisionEvidence) {
    let intent_ev = IntentEvidence {
        intent: kind_to_intent(step.kind),
        confidence: 0.95,
        default_tier: tier,
    };

    let risk_ev = RiskEvidence {
        level: risk,
        basis: cortex_core::evaluator::RiskBasis::Static,
        static_level: risk,
        file_risk: None,
        history_success_rate: None,
    };

    // Load real pressure from usage_events
    let window_ms = WINDOW_SECS * 1000;
    let raw_pressures = db.pressure_for_user(user_id, window_ms);
    let mut pressures = HashMap::new();
    for (provider_str, tier_str, tokens) in &raw_pressures {
        if let (Some(p), Some(t)) = (parse_provider_id(provider_str), parse_tier_opt(tier_str)) {
            let budget = token_budget(p, t) as f64;
            let ratio = if budget > 0.0 {
                *tokens as f64 / budget
            } else {
                0.0
            };
            pressures.insert((p, t), PressureState::from_ratio(ratio));
        }
    }

    let budget_ev = BudgetEvidence { pressures };

    // Load provider reliability
    let reliability = db.provider_reliability(user_id, 24);
    let reliability_map: HashMap<String, (i64, i64)> = reliability
        .into_iter()
        .map(|(p, total, successes)| (p, (total, successes)))
        .collect();

    // Build candidate scores for all available providers × requested tier
    let providers = [
        ProviderId::Claude,
        ProviderId::Openai,
        ProviderId::Gemini,
        ProviderId::Zen,
    ];
    let mut candidates = Vec::new();

    for &provider in &providers {
        let provider_str = provider.to_string();

        // Check if any worker has this provider
        // For now, assume all detected providers are available
        let pressure = budget_ev.pressure_for(provider, tier);

        let (success_rate, sample_count) =
            if let Some(&(total, successes)) = reliability_map.get(&provider_str) {
                if total > 0 {
                    (Some(successes as f64 / total as f64), total as u64)
                } else {
                    (None, 0)
                }
            } else {
                (None, 0)
            };

        // Check capability status from DB
        // No capability record = not available (unknown providers default to unauthenticated)
        let cap_status = db.get_provider_status(user_id, &provider_str);
        let authenticated = cap_status
            .as_deref()
            .map(|s| s != "unavailable")
            .unwrap_or(false);

        let estimated_duration = match step.kind {
            StepKind::Search => 30_000,
            StepKind::Execute | StepKind::Heal => 120_000,
            StepKind::Think | StepKind::Review => 180_000,
            StepKind::Test | StepKind::Build | StepKind::Lint => 90_000,
            StepKind::Gate => 30_000,
        };

        // Blend UCB bandit history into success rate for adaptive learning.
        // The bandit tracks contamination-weighted outcomes per (TaskFamily, RiskLevel, Provider),
        // which is more granular than the DB's per-provider reliability.
        let (blended_rate, blended_count) = if let Some(scorer) = ucb_scorer {
            use cortex_engine::bandit::{ArmKey, TaskFamily};
            let intent = kind_to_intent(step.kind);
            let task_family = TaskFamily::from_intent(&intent);
            let arm_key = ArmKey {
                task_family,
                risk_level: risk,
                provider,
            };
            if let Some(arm_stats) = scorer.arms.get(&arm_key) {
                let bandit_rate = arm_stats.mean_reward();
                let bandit_count = arm_stats.trials as u64;
                // Weighted blend: bandit history + DB reliability
                match (success_rate, sample_count) {
                    (Some(db_rate), db_count) if db_count > 0 => {
                        let total = db_count + bandit_count;
                        let blended = (db_rate * db_count as f64
                            + bandit_rate * bandit_count as f64)
                            / total as f64;
                        (Some(blended), total)
                    }
                    _ => (Some(bandit_rate), bandit_count),
                }
            } else {
                (success_rate, sample_count)
            }
        } else {
            (success_rate, sample_count)
        };

        candidates.push(CandidateScore {
            provider,
            tier,
            worker_id: None,
            authenticated,
            pressure,
            success_rate: blended_rate,
            sample_count: blended_count,
            estimated_duration_ms: estimated_duration,
        });
    }

    let fit_ev = ProviderFitEvidence { candidates };

    let evidence = DecisionEvidence {
        intent: intent_ev,
        risk: risk_ev,
        budget: budget_ev,
        provider_fit: fit_ev,
    };

    // Get user profile (or default to auto)
    let profile = db
        .get_user_profile(user_id)
        .and_then(|p| Profile::from_alias(&p))
        .unwrap_or(Profile::Auto);

    let auto_mode = db
        .get_user_auto_mode(user_id)
        .and_then(|m| parse_auto_mode(&m))
        .unwrap_or(AutoMode::Normal);

    let policy = DefaultPolicy;
    let decision = policy.decide(&evidence, profile, auto_mode);

    (decision, evidence)
}

// `cost_estimate` is read only under `#[cfg(feature = "soma")]`, so a
// default-features build sees it as unused. Naming it `_cost_estimate` to
// silence that -- which is what `cargo clippy --fix` did here -- breaks the
// soma build, because clippy fixes what it compiled and it did not compile
// that feature. Keep the name; allow the warning only where it applies.
#[cfg_attr(not(feature = "soma"), allow(unused_variables))]
async fn update_bandit_from_outcome(
    state: &AppState,
    db: &Database,
    step_id: &str,
    success: bool,
    cost_estimate: Option<f64>,
) {
    use cortex_engine::bandit::{ArmKey, TaskFamily};

    let step_info = db.get_step_info(step_id);
    let (provider_str, kind_str, risk_str) = match step_info {
        Some(info) => info,
        None => return,
    };

    let provider = match parse_provider_id(&provider_str) {
        Some(p) => p,
        None => return,
    };
    let risk = parse_risk(&risk_str);
    let kind = parse_step_kind(&kind_str);
    let intent = kind_to_intent(kind);
    let task_family = TaskFamily::from_intent(&intent);

    // Contamination: AI-executed steps have base contamination of 0.3.
    // Success from compiler/test evidence would be lower, but we don't have
    // signal details at this point. Use a conservative default.
    let contamination = 0.3;

    let arm_key = ArmKey {
        task_family,
        risk_level: risk,
        provider,
    };

    let reward = if success { 1.0 } else { 0.0 };

    let mut scorer = state.ucb_scorer.write().await;
    scorer.update(arm_key, reward, contamination);

    // Persist to store
    if let Some(store_mutex) = &state.cortex_store {
        if let Ok(store) = store_mutex.lock() {
            if let Some(stats) = scorer.arms.get(&arm_key) {
                let _ = store.save_arm_stats(&arm_key, stats);
            }
        }
    }

    let trials = scorer.arms.get(&arm_key).map(|s| s.trials).unwrap_or(0);
    let mean_reward = scorer
        .arms
        .get(&arm_key)
        .map(|s| s.mean_reward())
        .unwrap_or(0.0);

    tracing::debug!(
        "bandit update: {:?}/{:?}/{:?} reward={:.2} contamination={:.2} trials={}",
        task_family,
        risk,
        provider,
        reward,
        contamination,
        trials,
    );

    // Record Soma spend receipt with real cost (falls back to 1.0 credit if no
    // estimate). This is Soma's own ledger, not the Cortex credit ledger — the
    // Cortex ledger is written in `verification_driver.rs` and does not read
    // this. Compiled out by default.
    #[cfg(feature = "soma")]
    if success {
        if let Some(heart) = &state.soma_heart {
            let cost = cost_estimate.unwrap_or(1.0);
            if let Err(e) = heart.record_spend(step_id, cost, &format!("route:{:?}", intent)) {
                tracing::warn!("soma spend receipt failed: {e}");
            }
        }
    }

    // Emit MC event for real-time routing intelligence visibility
    if let Some(db) = &state.db {
        let user_id = db
            .get_step_run_id(step_id)
            .and_then(|run_id| db.get_run_user_id(&run_id));
        if let Some(user_id) = user_id {
            state
                .emit_mc_event(
                    &user_id,
                    MissionControlEvent::BanditUpdate {
                        provider: provider.to_string(),
                        task_family: format!("{:?}", task_family),
                        risk: format!("{:?}", risk),
                        trials,
                        mean_reward,
                        success,
                    },
                )
                .await;
        }
    }
}

fn record_evidence(db: &Database, decision_id: &str, evidence: &DecisionEvidence) {
    let now = chrono::Utc::now().timestamp_millis();

    // Intent evidence
    if let Ok(json) = serde_json::to_string(&evidence.intent) {
        db.record_score_evidence(
            decision_id,
            "intent",
            &json,
            Some(evidence.intent.confidence),
            now,
        );
    }

    // Risk evidence
    if let Ok(json) = serde_json::to_string(&evidence.risk) {
        db.record_score_evidence(decision_id, "risk", &json, None, now);
    }

    // Budget evidence (just store pressure ratios)
    let budget_summary: HashMap<String, f64> = evidence
        .budget
        .pressures
        .iter()
        .map(|((p, t), s)| (format!("{}:{}", p, t), s.ratio()))
        .collect();
    if let Ok(json) = serde_json::to_string(&budget_summary) {
        db.record_score_evidence(decision_id, "budget", &json, None, now);
    }

    // Provider fit evidence (candidate count)
    if let Ok(json) = serde_json::to_string(&evidence.provider_fit) {
        db.record_score_evidence(decision_id, "provider_fit", &json, None, now);
    }
}

// --- Sub-delegation for worker steps ---

/// No delegation to issue: a default build has no heart to sign one with.
///
/// The worker's matching fence is what makes this safe. A worker built without
/// the `soma` feature does not ask for a delegation; a worker built *with* it
/// rejects every step this returns `None` for. The two features must be set
/// the same way on both binaries — see `docs/adr/ADR-0003-soma-feature-fence.md`.
#[cfg(not(feature = "soma"))]
fn issue_step_delegation(
    _state: &AppState,
    _step_id: &str,
    _lease_deadline_ms: i64,
    _worker_id: &str,
) -> Option<serde_json::Value> {
    None
}

#[cfg(feature = "soma")]
fn issue_step_delegation(
    state: &AppState,
    step_id: &str,
    lease_deadline_ms: i64,
    worker_id: &str,
) -> Option<serde_json::Value> {
    let heart = state.soma_heart.as_ref()?;

    let caveats = vec![
        soma::delegation::Caveat::ExpiresAt {
            timestamp: lease_deadline_ms as u64,
        },
        soma::delegation::Caveat::Capabilities {
            allow: vec![format!("execute:step:{step_id}")],
        },
        soma::delegation::Caveat::MaxInvocations { count: 1 },
        soma::delegation::Caveat::Audience {
            did: heart.identity.did.clone(),
        },
    ];

    // Use the worker_id as subject — workers don't have DIDs yet,
    // so we use a deterministic DID-like identifier derived from the worker_id.
    let worker_subject = format!("did:cortex:worker:{worker_id}");

    match soma::delegation::create_delegation(
        &heart.identity.secret_key,
        &heart.identity.public_key,
        &heart.identity.did,
        &worker_subject,
        vec![format!("execute:step:{step_id}")],
        caveats,
        None,
    ) {
        Ok(delegation) => {
            heart.record_heartbeat(
                soma::heartbeat::HeartbeatEventType::DelegationIssued,
                &serde_json::json!({
                    "delegation_id": delegation.id,
                    "step_id": step_id,
                    "worker_id": worker_id,
                    "subject": worker_subject,
                    "capability": format!("execute:step:{step_id}"),
                })
                .to_string(),
            );
            serde_json::to_value(&delegation).ok()
        }
        Err(e) => {
            tracing::warn!("failed to issue step delegation: {e}");
            None
        }
    }
}

// --- Step context builder ---

/// Share of a step's token budget the repo map may occupy.
///
/// The map is orientation, not content: it should tell the model what the
/// repository *is* and then get out of the way. Five percent of a 500k
/// Execute budget is ~25k tokens, which fits a skeleton of a large monorepo
/// while leaving the step's actual context untouched.
const REPO_MAP_BUDGET_FRACTION: u64 = 20;

fn build_step_context(db: &Database, run_id: &str, step_id: &str) -> StepContext {
    let goal = db
        .get_run_goal(run_id)
        .unwrap_or_else(|| "unknown goal".into());

    // Get predecessor step IDs from dependencies
    let predecessors = db.get_step_predecessors(step_id);

    let summaries: Vec<PredecessorSummary> = predecessors
        .into_iter()
        .filter_map(|pred_id| {
            let (kind, _, _, _, _) = db.get_step_details(&pred_id)?;
            let summary = db.get_step_output_summary(&pred_id).unwrap_or_default();
            let files = db
                .get_step_files_changed(&pred_id)
                .and_then(|json| serde_json::from_str::<Vec<String>>(&json).ok())
                .unwrap_or_default();

            if summary.is_empty() && files.is_empty() {
                return None;
            }

            Some(PredecessorSummary {
                step_id: pred_id,
                kind,
                summary,
                files_changed: files,
            })
        })
        .collect();

    StepContext {
        predecessor_summaries: summaries,
        user_goal: goal,
        conversation_excerpt: None,
        repo_map: None,
    }
}

/// Attach the repo map (CONTEXT.md C1) so planning is not blind to code the
/// user did not think to name.
///
/// Failure is silent by design. A repository we cannot parse, or a workspace
/// that is not there yet, should cost a step its orientation and nothing
/// else — never its dispatch.
fn with_repo_map(
    mut context: StepContext,
    state: &AppState,
    provider: ProviderId,
    tier: Tier,
) -> StepContext {
    let budget = token_budget(provider, tier) / REPO_MAP_BUDGET_FRACTION;
    context.repo_map = state
        .repo_map_cache
        .get(&state.workspace_dir, budget as usize)
        .map(|rendered| rendered.to_string());
    context
}

// --- Load ready steps ---

async fn load_ready_steps_for_run(state: &AppState, sched: &mut SchedulerState, run_id: &str) {
    let db = match &state.db {
        Some(db) => db,
        None => return,
    };

    let ready_ids = db.find_ready_steps(run_id);
    let user_id = get_run_user(db, run_id);

    for step_id in ready_ids {
        if let Some((kind, work_kind, tier, risk, objective)) = db.get_step_details(&step_id) {
            let step_kind = parse_step_kind(&kind);
            sched.enqueue_ready_step(StepRef {
                step_id,
                run_id: run_id.to_string(),
                user_id: user_id.clone(),
                kind: step_kind,
                work_kind: parse_work_kind(&work_kind),
                tier,
                risk,
                objective,
            });
        }
    }
}

// --- Heal insertion ---

async fn try_heal(state: &AppState, _sched: &mut SchedulerState, run_id: &str, step_id: &str) {
    let db = match &state.db {
        Some(db) => db,
        None => return,
    };

    let (kind_str, work_kind_str, tier, risk, objective) = match db.get_step_details(step_id) {
        Some(d) => d,
        None => return,
    };

    let kind = parse_step_kind(&kind_str);
    if !kind.is_healable() {
        return;
    }

    let heal_count = db.get_run_heal_count(run_id);

    // Get max heals from profile
    let profile = db
        .get_run_profile(run_id)
        .and_then(|p| Profile::from_alias(&p))
        .unwrap_or(Profile::Auto);
    let max_heals = profile.heal_attempts() as i32;

    if heal_count >= max_heals {
        tracing::info!(
            "run {run_id} exhausted heal budget ({heal_count}/{max_heals}), not healing step {step_id}"
        );
        let skipped = db.cascade_failure(step_id);
        if !skipped.is_empty() {
            tracing::info!(
                "cascaded failure from step {step_id}: skipped {} downstream steps",
                skipped.len()
            );
        }
        return;
    }

    let last_error = db.get_step_last_error(step_id);

    let plan = plan_heal(
        step_id,
        kind,
        &objective,
        &tier,
        &risk,
        last_error.as_deref(),
        heal_count as u32 + 1,
    );

    let now_ms = chrono::Utc::now().timestamp_millis();

    // Calculate exponential backoff with jitter before re-dispatching heal steps.
    // base_delay * 2^(attempt-1), capped at 120s, with ±25% hash-based jitter.
    let attempt = heal_count as u32 + 1;
    let base_delay_ms: i64 = 5_000;
    let max_delay_ms: i64 = 120_000;
    let raw_delay = base_delay_ms.saturating_mul(1_i64 << (attempt - 1).min(20));
    let capped_delay = raw_delay.min(max_delay_ms);

    // Hash-based jitter: use step_id bytes to get a deterministic ±25% offset
    let hash_val: u64 = step_id.bytes().fold(0xcbf29ce484222325_u64, |h, b| {
        h.wrapping_mul(0x100000001b3).wrapping_add(b as u64)
    });
    let jitter_range = capped_delay / 4; // 25%
    let jitter = if jitter_range > 0 {
        (hash_val % (jitter_range as u64 * 2)) as i64 - jitter_range
    } else {
        0
    };
    let backoff_delay = (capped_delay + jitter).max(1_000); // at least 1s
    let earliest_dispatch_at = now_ms + backoff_delay;

    db.create_step_with_id(
        &plan.heal_step_id,
        run_id,
        "heal",
        WorkKind::Heal.as_str(),
        &tier,
        &risk,
        &plan.heal_objective,
        now_ms,
    );
    db.set_step_earliest_dispatch(&plan.heal_step_id, earliest_dispatch_at);
    db.add_step_dependency(
        &plan.heal_step_id,
        step_id,
        EdgeType::CompletionRequired.as_str(),
    );

    db.create_step_with_id(
        &plan.retry_step_id,
        run_id,
        &kind_str,
        &work_kind_str,
        &tier,
        &risk,
        &plan.retry_objective,
        now_ms,
    );
    db.add_step_dependency(
        &plan.retry_step_id,
        &plan.heal_step_id,
        EdgeType::SuccessRequired.as_str(),
    );

    if !db.mark_step_recovered(step_id) {
        tracing::warn!(
            "inserted heal chain for step {step_id}, but failed to mark original step recovered"
        );
    }

    db.increment_heal_count(run_id);

    tracing::info!(
        "inserted heal chain for step {step_id}: heal={} → retry={} (backoff {backoff_delay}ms, dispatch after {})",
        plan.heal_step_id,
        plan.retry_step_id,
        earliest_dispatch_at,
    );
}

// --- Run completion ---

async fn check_run_done(state: &AppState, run_id: &str) {
    let db = match &state.db {
        Some(db) => db,
        None => return,
    };

    let step_statuses = db.get_all_step_statuses(run_id);
    let parsed: Vec<(String, StepStatus)> = step_statuses
        .into_iter()
        .filter_map(|(id, status_str)| StepStatus::from_str(&status_str).map(|s| (id, s)))
        .collect();

    if let Some(final_status) = check_run_completion(&parsed) {
        let status_str = final_status.as_str();

        // Guard against double completion: check if run is already terminal
        if let Some(run_info) = db.list_user_runs_by_id(run_id) {
            if let Some(current_status) = run_info.get("status").and_then(|s| s.as_str()) {
                if let Some(rs) = RunStatus::from_str(current_status) {
                    if rs.is_terminal() {
                        tracing::debug!(
                            "run {run_id} already in terminal state '{current_status}', skipping update"
                        );
                        return;
                    }
                }
            }
        }

        db.update_run_status(run_id, status_str, None);

        // Log branch info for PR creation if the run succeeded with changes
        if final_status == RunStatus::Succeeded {
            if let Some(branch) = db.get_run_branch(run_id) {
                tracing::info!(
                    run_id = %run_id,
                    branch = %branch,
                    "run succeeded with branch — ready for PR creation via POST /api/runs/{}/pr",
                    run_id,
                );
            }
        }

        // Emit MC event for run completion
        let user_id = get_run_user(db, run_id);
        state
            .emit_mc_event(
                &user_id,
                MissionControlEvent::RunCompleted {
                    run_id: run_id.to_string(),
                    status: status_str.to_string(),
                    total_cost: None, // TODO: aggregate from step cost_estimates
                },
            )
            .await;

        tracing::info!("run {run_id} → {status_str}");
    }
}

// --- Lease expiry & reconciliation ---

async fn expire_stale_leases(state: &AppState, _sched: &mut SchedulerState) {
    let db = match &state.db {
        Some(db) => db,
        None => return,
    };

    let expired = db.expire_stale_leases();
    if !expired.is_empty() {
        tracing::warn!("expired {} stale leases: {:?}", expired.len(), expired);
    }
}

async fn expire_stale_resource_leases(state: &AppState) {
    let db = match &state.db {
        Some(db) => db,
        None => return,
    };

    let expired = db.expire_stale_resource_leases();
    if !expired.is_empty() {
        tracing::warn!(
            "expired {} stale resource leases: {:?}",
            expired.len(),
            expired
        );
    }
}

async fn reconcile_ready_steps(state: &AppState, sched: &mut SchedulerState) {
    let db = match &state.db {
        Some(db) => db,
        None => return,
    };

    // Single query fetches all ready steps across all active runs,
    // replacing the N+1 pattern of get_active_run_ids() + find_ready_steps() per run.
    let ready = db.find_all_ready_steps();
    for (step_id, run_id, user_id, kind, work_kind, tier, risk, objective) in ready {
        let step_kind = parse_step_kind(&kind);
        sched.enqueue_ready_step(StepRef {
            step_id,
            run_id,
            user_id,
            kind: step_kind,
            work_kind: parse_work_kind(&work_kind),
            tier,
            risk,
            objective,
        });
    }
}

/// Finish runs whose last step was verified out of band.
///
/// Verdicts arrive from a spawned task that holds a `Database` and nothing
/// else, so it cannot emit a scheduler event. Without this pass a run whose
/// final step became `verified` in the background would sit at `running`
/// forever. The reconcile tick is the right home: it is already the loop that
/// repairs state nothing told the scheduler about.
///
/// PR B replaces the spawned task with a durable job that can report for
/// itself; this pass stays as the backstop for a verdict that lands while the
/// process is down.
async fn reconcile_run_completion(state: &AppState) {
    let Some(db) = &state.db else {
        return;
    };
    for run_id in db.get_active_run_ids() {
        check_run_done(state, &run_id).await;
    }
}

async fn recover_from_db(state: &AppState, sched: &mut SchedulerState) {
    let db = match &state.db {
        Some(db) => db,
        None => return,
    };

    let expired = db.expire_stale_leases();
    if !expired.is_empty() {
        tracing::info!("recovery: expired {} stale leases", expired.len());
    }
    let expired_resources = db.expire_stale_resource_leases();
    if !expired_resources.is_empty() {
        tracing::info!(
            "recovery: expired {} stale resource leases",
            expired_resources.len()
        );
    }

    let active_runs = db.get_active_run_ids();
    tracing::info!("recovery: {} active runs found", active_runs.len());

    for run_id in active_runs {
        load_ready_steps_for_run(state, sched, &run_id).await;
    }
}

// --- Grace period expiry ---

async fn expire_grace_periods(state: &AppState, sched: &mut SchedulerState) {
    let db = match &state.db {
        Some(db) => db,
        None => return,
    };

    let expired = db.workers_past_grace();
    if expired.is_empty() {
        return;
    }

    let mut orphaned_steps = Vec::new();
    for (worker_id, step_id) in &expired {
        db.orphan_step(step_id);
        orphaned_steps.push(step_id.clone());
        tracing::warn!("grace period expired for worker {worker_id} — orphaning step {step_id}");
    }

    // Re-enqueue orphaned steps from active runs
    for step_id in &orphaned_steps {
        if let Some(run_id) = db.get_step_run_id(step_id) {
            load_ready_steps_for_run(state, sched, &run_id).await;
        }
    }
}

fn cleanup_expired_keys(state: &AppState) {
    if let Some(db) = &state.db {
        let cleaned = db.cleanup_expired_idempotency();
        if cleaned > 0 {
            tracing::debug!("cleaned {cleaned} expired idempotency keys");
        }
    }
}

// --- Helpers ---

fn get_run_user(db: &Database, run_id: &str) -> String {
    db.get_run_user_id(run_id)
        .unwrap_or_else(|| "unknown".into())
}

fn parse_tier(s: &str) -> Tier {
    match s {
        "search" => Tier::Search,
        "think" => Tier::Think,
        _ => Tier::Execute,
    }
}

fn parse_tier_opt(s: &str) -> Option<Tier> {
    match s {
        "search" => Some(Tier::Search),
        "execute" => Some(Tier::Execute),
        "think" => Some(Tier::Think),
        _ => None,
    }
}

fn parse_risk(s: &str) -> RiskLevel {
    match s {
        "low" => RiskLevel::Low,
        "high" => RiskLevel::High,
        "critical" => RiskLevel::Critical,
        _ => RiskLevel::Medium,
    }
}

fn parse_provider_id(s: &str) -> Option<ProviderId> {
    match s {
        "claude" => Some(ProviderId::Claude),
        "openai" => Some(ProviderId::Openai),
        "gemini" => Some(ProviderId::Gemini),
        "zen" => Some(ProviderId::Zen),
        _ => None,
    }
}

fn parse_auto_mode(s: &str) -> Option<AutoMode> {
    match s {
        "normal" => Some(AutoMode::Normal),
        "protect_budget" => Some(AutoMode::ProtectBudget),
        "protect_quality" => Some(AutoMode::ProtectQuality),
        "recovering" => Some(AutoMode::Recovering),
        _ => None,
    }
}

fn parse_step_kind(s: &str) -> StepKind {
    match s {
        "search" => StepKind::Search,
        "execute" => StepKind::Execute,
        "think" => StepKind::Think,
        "test" => StepKind::Test,
        "build" => StepKind::Build,
        "lint" => StepKind::Lint,
        "heal" => StepKind::Heal,
        "review" => StepKind::Review,
        "gate" => StepKind::Gate,
        _ => StepKind::Execute,
    }
}

fn parse_work_kind(s: &str) -> Option<WorkKind> {
    WorkKind::from_str(s)
}

fn kind_to_intent(kind: StepKind) -> Intent {
    match kind {
        StepKind::Search => Intent::Explore,
        StepKind::Execute | StepKind::Heal => Intent::Fix,
        StepKind::Think | StepKind::Review => Intent::Think,
        StepKind::Test | StepKind::Build | StepKind::Lint | StepKind::Gate => Intent::Test,
    }
}

// --- Public API for creating runs ---

pub fn build_resource_lease_requests(
    file_paths: &[String],
    repo_key: Option<&str>,
    task_id: Option<&str>,
    group_id: Option<&str>,
) -> Vec<ResourceLeaseRequest> {
    build_resource_lease_requests_extended(file_paths, repo_key, task_id, group_id, None, None)
}

pub fn build_resource_lease_requests_extended(
    file_paths: &[String],
    repo_key: Option<&str>,
    task_id: Option<&str>,
    group_id: Option<&str>,
    branch_name: Option<&str>,
    deploy_environment: Option<&str>,
) -> Vec<ResourceLeaseRequest> {
    let mut seen = HashSet::new();
    let mut requests = Vec::new();
    let repo_key = normalize_repo_key(repo_key);

    if let (Some(group_id), Some(task_id)) = (group_id, task_id) {
        let key = format!("{group_id}:{task_id}");
        if seen.insert(format!("task:{key}")) {
            requests.push(ResourceLeaseRequest {
                resource_type: "task".to_string(),
                repo_key: repo_key.clone(),
                resource_key: key,
                mode: "exclusive".to_string(),
                reason: Some("task-bound run".to_string()),
                metadata: serde_json::json!({
                    "repo_key": repo_key,
                    "group_id": group_id,
                    "task_id": task_id,
                }),
            });
        }
    }

    if file_paths.is_empty() {
        if seen.insert("path:.".to_string()) {
            requests.push(ResourceLeaseRequest {
                resource_type: "path".to_string(),
                repo_key: repo_key.clone(),
                resource_key: ".".to_string(),
                mode: "write".to_string(),
                reason: Some("repo-wide run without explicit file paths".to_string()),
                metadata: serde_json::json!({ "repo_key": repo_key, "repo_wide": true }),
            });
        }
    } else {
        for path in file_paths {
            // Normalised rather than merely trimmed. `src/` and `src` are the
            // same directory but different strings, and `path_keys_overlap`
            // tests a prefix followed by `/` — so on the unnormalised forms
            // `src/` and `src/a.rs` are reported as NOT overlapping, and two
            // steps writing the same directory both get a lease.
            let key = cortex_core::write_set::normalise(path);
            if key.is_empty() {
                continue;
            }
            if seen.insert(format!("path:{key}")) {
                requests.push(ResourceLeaseRequest {
                    resource_type: "path".to_string(),
                    repo_key: repo_key.clone(),
                    resource_key: key.clone(),
                    mode: "write".to_string(),
                    reason: Some("run file path scope".to_string()),
                    metadata: serde_json::json!({ "repo_key": repo_key, "path": key }),
                });
            }
        }
    }

    // Branch lease — exclusive lock on the branch name
    if let Some(branch) = branch_name {
        let branch = branch.trim().to_string();
        if !branch.is_empty() && seen.insert(format!("branch:{branch}")) {
            requests.push(ResourceLeaseRequest {
                resource_type: "branch".to_string(),
                repo_key: repo_key.clone(),
                resource_key: branch.clone(),
                mode: "exclusive".to_string(),
                reason: Some("run branch scope".to_string()),
                metadata: serde_json::json!({ "repo_key": repo_key, "branch": branch }),
            });
        }
    }

    // Environment lease — exclusive lock on the deploy environment
    if let Some(env_name) = deploy_environment {
        let env_name = env_name.trim().to_string();
        if !env_name.is_empty() && seen.insert(format!("environment:{env_name}")) {
            requests.push(ResourceLeaseRequest {
                resource_type: "environment".to_string(),
                repo_key: repo_key.clone(),
                resource_key: env_name.clone(),
                mode: "exclusive".to_string(),
                reason: Some("run deploy environment scope".to_string()),
                metadata: serde_json::json!({ "repo_key": repo_key, "environment": env_name }),
            });
        }
    }

    // Canonical acquisition order.
    //
    // Two transactions taking the same locks in opposite orders can deadlock.
    // That is latent today rather than live — acquisition happens inside one
    // transaction that checks every conflict before inserting anything — but it
    // depends on caller assembly order, which is not a property anyone is
    // maintaining. Sorting here makes every acquirer agree without needing to
    // coordinate, and costs nothing.
    requests.sort_by(|a, b| {
        (&a.resource_type, &a.repo_key, &a.resource_key).cmp(&(
            &b.resource_type,
            &b.repo_key,
            &b.resource_key,
        ))
    });

    requests
}

pub fn normalize_repo_key(repo_key: Option<&str>) -> String {
    repo_key
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .unwrap_or("default")
        .to_string()
}

pub async fn create_run_from_goal(
    state: &AppState,
    scheduler_tx: &SchedulerTx,
    user_id: &str,
    goal: &str,
    file_paths: &[String],
    repo_key: Option<&str>,
    profile: &str,
    task_id: Option<&str>,
    group_id: Option<&str>,
    conversation_id: Option<&str>,
    authority_context: Option<serde_json::Value>,
) -> Result<String, String> {
    use cortex_engine::decomposer::decompose_goal;

    let builder = decompose_goal(user_id, goal, file_paths, profile)?;

    let db = state.db.as_ref().ok_or("database not available")?;
    let now_ms = chrono::Utc::now().timestamp_millis();

    // Collect steps and edges for batch insertion in a single transaction
    let steps: Vec<(
        String,
        String,
        String,
        Option<String>,
        String,
        String,
        String,
        i64,
    )> = builder
        .steps()
        .iter()
        .map(|step| {
            (
                step.id.clone(),
                step.kind.as_str().to_string(),
                step.work_kind.as_str().to_string(),
                step.recipe_seed
                    .as_ref()
                    .and_then(|seed| serde_json::to_string(seed).ok()),
                step.tier.clone(),
                step.risk.clone(),
                step.objective.clone(),
                now_ms,
            )
        })
        .collect();

    let edges: Vec<(String, String, String)> = builder
        .edges()
        .iter()
        .map(|&(from_idx, to_idx, edge_type)| {
            let from_id = builder.step_id(from_idx);
            let to_id = builder.step_id(to_idx);
            (
                to_id.to_string(),
                from_id.to_string(),
                edge_type.as_str().to_string(),
            )
        })
        .collect();

    let normalized_repo_key = normalize_repo_key(repo_key);
    let resource_leases =
        build_resource_lease_requests(file_paths, Some(&normalized_repo_key), task_id, group_id);
    let run_id = db
        .create_run_with_steps_and_resource_leases_with_authority(
            user_id,
            goal,
            profile,
            file_paths,
            task_id,
            group_id,
            conversation_id,
            &resource_leases,
            &steps,
            &edges,
            authority_context.as_ref(),
        )
        .map_err(|err| err.message())?;

    scheduler_tx
        .send(SchedulerEvent::RunCreated {
            run_id: run_id.clone(),
        })
        .await
        .map_err(|_| "scheduler channel closed")?;

    tracing::info!(
        "created run {run_id}: {} steps, {} edges, repo_key={}",
        builder.steps().len(),
        builder.edges().len(),
        normalized_repo_key
    );

    Ok(run_id)
}

#[cfg(all(test, not(feature = "soma")))]
mod soma_fence_tests {
    use super::*;

    /// The dispatch payload carries no Soma-issued credential.
    ///
    /// This is the load-bearing half of the fence's promise: a step leaves
    /// Cortex with `delegation: None`, so nothing a worker executes, nothing it
    /// reports, and nothing that lands on a receipt traces back to a Soma
    /// signature. If this ever returns `Some`, a value from a WIP subsystem has
    /// re-entered the execution path.
    #[tokio::test]
    async fn no_step_delegation_is_issued() {
        let temporary = tempfile::tempdir().expect("temporary workspace");
        let workspace = temporary.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".cortex")).expect("workspace metadata");
        let state = AppState::new(workspace.join(".cortex/ledger.jsonl"), workspace, None).await;

        let issued = issue_step_delegation(&state, "step-1", 1_000_000, "worker-1");

        assert!(
            issued.is_none(),
            "a build without the soma feature must not attach a delegation to a \
             dispatched step, got {issued:?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_package_json(dir: &Path, scripts: &str) {
        std::fs::write(
            dir.join("package.json"),
            format!(r#"{{"scripts":{{{scripts}}}}}"#),
        )
        .unwrap();
    }

    #[test]
    fn build_resource_lease_requests_uses_paths_and_task_binding() {
        let requests = build_resource_lease_requests(
            &[
                "src/main.rs".to_string(),
                "src/main.rs".to_string(),
                "src/lib.rs".to_string(),
            ],
            Some("github:hey-vera/heyvera"),
            Some("task-1"),
            Some("group-1"),
        );

        // Asserted by content, not position: requests come back in canonical
        // acquisition order now, and the old positional assertions were
        // encoding assembly order as though it meant something.
        assert_eq!(requests.len(), 3);

        let task = requests
            .iter()
            .find(|r| r.resource_type == "task")
            .expect("task lease");
        assert_eq!(task.repo_key, "github:hey-vera/heyvera");
        assert_eq!(task.resource_key, "group-1:task-1");
        assert_eq!(task.mode, "exclusive");

        let mut paths: Vec<&str> = requests
            .iter()
            .filter(|r| r.resource_type == "path")
            .map(|r| r.resource_key.as_str())
            .collect();
        paths.sort();
        // The duplicate `src/main.rs` collapses.
        assert_eq!(paths, vec!["src/lib.rs", "src/main.rs"]);
        assert!(requests
            .iter()
            .filter(|r| r.resource_type == "path")
            .all(|r| r.mode == "write" && r.repo_key == "github:hey-vera/heyvera"));
    }

    /// Every acquirer must derive the same order without coordinating, or two
    /// transactions can take the same locks in opposite orders.
    #[test]
    fn lease_requests_come_back_in_a_canonical_order() {
        let forward = build_resource_lease_requests(
            &["src/z.rs".to_string(), "crates/api".to_string()],
            Some("r"),
            Some("t"),
            Some("g"),
        );
        let reversed = build_resource_lease_requests(
            &["crates/api".to_string(), "src/z.rs".to_string()],
            Some("r"),
            Some("t"),
            Some("g"),
        );

        let key_of = |rs: &[ResourceLeaseRequest]| -> Vec<(String, String)> {
            rs.iter()
                .map(|r| (r.resource_type.clone(), r.resource_key.clone()))
                .collect()
        };
        assert_eq!(key_of(&forward), key_of(&reversed));
    }

    /// `src/` and `src` are the same directory. Left untrimmed, the overlap
    /// check (prefix followed by `/`) reports `src/` and `src/a.rs` as NOT
    /// overlapping, and both steps get a lease on the same directory.
    #[test]
    fn a_trailing_slash_does_not_create_a_second_key_for_one_directory() {
        let requests = build_resource_lease_requests(
            &["src/".to_string(), "src".to_string(), "/src/".to_string()],
            None,
            None,
            None,
        );
        let paths: Vec<&str> = requests
            .iter()
            .filter(|r| r.resource_type == "path")
            .map(|r| r.resource_key.as_str())
            .collect();
        assert_eq!(paths, vec!["src"], "three spellings of one directory");
    }

    #[test]
    fn build_resource_lease_requests_uses_repo_wide_path_without_file_paths() {
        let requests = build_resource_lease_requests(&[], None, None, None);

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].resource_type, "path");
        assert_eq!(requests[0].repo_key, "default");
        assert_eq!(requests[0].resource_key, ".");
        assert_eq!(requests[0].metadata["repo_wide"], true);
    }

    #[test]
    fn build_resource_lease_requests_scopes_same_paths_by_repo_key() {
        let first = build_resource_lease_requests(
            &["src/main.rs".to_string()],
            Some("github:hey-vera/heyvera"),
            None,
            None,
        );
        let second = build_resource_lease_requests(
            &["src/main.rs".to_string()],
            Some("github:claw-net/claw-net"),
            None,
            None,
        );

        assert_eq!(first[0].resource_key, second[0].resource_key);
        assert_ne!(first[0].repo_key, second[0].repo_key);
    }

    #[test]
    fn test_step_uses_existing_npm_test_script() {
        let dir = tempfile::tempdir().unwrap();
        write_package_json(
            dir.path(),
            r#""test:unit":"vitest run","test":"npm run test:unit""#,
        );

        let checks = infer_required_checks(
            StepKind::Test,
            RiskLevel::Medium,
            &["src/index.ts".to_string()],
            dir.path(),
        );

        // The floor installs from the lockfile before running anything, so
        // the verdict describes the tree that was delivered.
        let names: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["ecosystem:npm-ci", "ecosystem:npm-test"]);
        assert!(checks.iter().all(|c| c.required));
    }

    #[test]
    fn a_polyglot_tree_checks_both_ecosystems() {
        // The old rule picked ONE check by sniffing file extensions, so a Rust
        // change in a repo that also ships a frontend never built the
        // frontend. The floor is derived from what the tree is, so both run.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        write_package_json(dir.path(), r#""build":"tsc""#);

        let checks = infer_required_checks(
            StepKind::Build,
            RiskLevel::Medium,
            &["crates/api/src/main.rs".to_string()],
            dir.path(),
        );

        let names: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "ecosystem:cargo-check",
                "ecosystem:cargo-test",
                "ecosystem:npm-ci",
                "ecosystem:npm-build",
            ]
        );
        assert!(checks
            .iter()
            .any(|c| c.command == "cargo check --locked --workspace"));
    }

    #[test]
    fn high_risk_execute_adds_the_path_overlay_on_top_of_the_floor() {
        let dir = tempfile::tempdir().unwrap();
        write_package_json(dir.path(), r#""build":"tsc""#);

        let checks = infer_required_checks(
            StepKind::Execute,
            RiskLevel::High,
            &["src/index.ts".to_string()],
            dir.path(),
        );

        let names: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "ecosystem:npm-ci",
                "ecosystem:npm-build",
                "risk:allowed-paths"
            ]
        );
    }

    #[test]
    fn recipe_for_test_step_mirrors_required_checks() {
        let checks = vec![RequiredCheck {
            name: "cargo:test".to_string(),
            command: "cargo test -p cortex-api".to_string(),
            required: true,
        }];

        let recipe = build_work_recipe(
            WorkKind::Test,
            "run the API tests",
            RiskLevel::Medium,
            Tier::Execute,
            &["crates/api/src/lib.rs".to_string()],
            Some("abc123"),
            &checks,
            None,
        );

        assert_eq!(recipe.kind, WorkKind::Test);
        assert_eq!(recipe.required_checks.len(), 1);
        assert_eq!(recipe.acceptance.len(), 1);
        assert_eq!(
            recipe.acceptance[0].text,
            "Required check `cargo:test` passes"
        );
        assert!(recipe
            .constraints
            .iter()
            .any(|c| c == "expected_base_commit=abc123"));
    }

    #[test]
    fn recipe_uses_planner_seed_without_required_checks() {
        let seed = WorkRecipeSeed {
            target_paths: vec!["crates/engine/src/decomposer.rs".to_string()],
            acceptance: vec![AcceptanceCriterion {
                id: "planner-objective-satisfied".to_string(),
                text: "Planner objective is satisfied".to_string(),
                verification: AcceptanceVerification::Manual,
            }],
            constraints: vec!["planner_risk=medium".to_string()],
        };

        let recipe = build_work_recipe(
            WorkKind::Refactor,
            "refactor decomposition",
            RiskLevel::Medium,
            Tier::Execute,
            &[],
            None,
            &[],
            Some(&seed),
        );

        assert_eq!(recipe.kind, WorkKind::Refactor);
        assert_eq!(recipe.target_paths, vec!["crates/engine/src/decomposer.rs"]);
        assert_eq!(recipe.acceptance[0].text, "Planner objective is satisfied");
        assert!(recipe
            .constraints
            .iter()
            .any(|c| c == "planner_risk=medium"));
    }

    #[test]
    fn low_risk_execute_is_checked_too() {
        // This is the whole point of V2. The old rule returned nothing here,
        // which meant that for most billable work "verified" amounted to
        // "the CLI exited 0, and the CLI is the one telling us".
        let dir = tempfile::tempdir().unwrap();
        write_package_json(dir.path(), r#""build":"tsc""#);

        let checks = infer_required_checks(
            StepKind::Execute,
            RiskLevel::Low,
            &["src/index.ts".to_string()],
            dir.path(),
        );

        let names: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["ecosystem:npm-ci", "ecosystem:npm-build"]);
    }

    #[test]
    fn placeholder_npm_test_never_becomes_a_required_check() {
        // `npm test` on a generated package fails by design. A floor that
        // always fails is as useless as one that always passes — but `npm ci`
        // still applies, because the lockfile must still resolve.
        let dir = tempfile::tempdir().unwrap();
        write_package_json(
            dir.path(),
            r#""test":"echo \"Error: no test specified\" && exit 1""#,
        );

        let checks = infer_required_checks(
            StepKind::Test,
            RiskLevel::Medium,
            &["src/index.ts".to_string()],
            dir.path(),
        );

        let names: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["ecosystem:npm-ci"]);
    }

    #[test]
    fn read_only_steps_are_not_gated_on_verification() {
        // Verification attaches to steps that change trees. Gating a search
        // on it would spend sandbox time to prove nothing.
        let dir = tempfile::tempdir().unwrap();
        write_package_json(dir.path(), r#""build":"tsc""#);

        for kind in [
            StepKind::Search,
            StepKind::Think,
            StepKind::Review,
            StepKind::Gate,
        ] {
            let checks = infer_required_checks(
                kind,
                RiskLevel::Critical,
                &["src/index.ts".to_string()],
                dir.path(),
            );
            assert!(
                checks.is_empty(),
                "{kind:?} does not change the tree and must not be gated"
            );
        }
    }

    #[test]
    fn a_tree_no_ecosystem_owns_derives_nothing_and_is_sold_as_unverified() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "# docs\n").unwrap();

        let checks = infer_required_checks(
            StepKind::Execute,
            RiskLevel::Low,
            &["README.md".to_string()],
            dir.path(),
        );

        assert!(checks.is_empty());
    }
}
