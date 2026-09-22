use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cortex_core::protocol::BrainMessage;
use cortex_core::provider::{ProviderId, ProviderStatus};
use cortex_core::routing::RoutingDecision;
use cortex_core::task::TaskContract;
use cortex_core::usage::UsageLimits;
use cortex_engine::bandit::UcbScorer;
use cortex_engine::captain::SchedulerEvent;
use cortex_engine::ledger::Ledger;
use cortex_engine::store::CortexStore;
use std::sync::Mutex;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::clerk::{JwksCache, JwksStampedeGuard};
use crate::db::Database;
use crate::github::GitHubClient;
use crate::mission_control::{McSubscriber, MissionControlEvent, SubscriberId};
// Removed problematic module imports
use crate::ratelimit::RateLimiter;

use crate::context_flow::{ContextBus, ContextBusConfig};
#[cfg(feature = "soma")]
use crate::soma::CortexHeart;
use crate::storage::Storage;
use crate::vera::VeraTracker;

/// The local identifier `VeraTracker` groups interactions under when there is
/// no Soma heart to borrow a DID from — which is every default build.
///
/// Deterministic on purpose: restarting the process must not fragment a
/// tracker's history into two apparent Cortexes.
fn default_cortex_heart_id() -> cortex_core::vera::HeartId {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    "cortex-heart-v1".hash(&mut hasher);
    let hash = hasher.finish().to_le_bytes();
    let mut id = [0u8; 32];
    id[..8].copy_from_slice(&hash);
    id[8..16].copy_from_slice(&hash);
    id[16..24].copy_from_slice(&hash);
    id[24..32].copy_from_slice(&hash);
    cortex_core::vera::HeartId(id)
}

pub struct ConnectedWorker {
    pub worker_id: String,
    pub user_id: String,
    pub available_providers: Vec<ProviderId>,
    pub disabled_providers: HashSet<ProviderId>,
    pub tx: mpsc::Sender<BrainMessage>,
}

/// Where the Cortex database lives.
///
/// A function rather than a literal because background work — verification in
/// particular — opens its own connection instead of borrowing `AppState`'s,
/// and two spellings of this path that drift apart would silently split the
/// database in two. That is exactly why the `CORTEX_DB_PATH` override belongs
/// here and not at the single call site that introduced it: in production the
/// database is deliberately *outside* the workspace, so a second caller
/// defaulting to `workspace_dir` would write verdicts into a different file
/// than the API reads.
///
/// The override exists because the workspace is a git checkout on the VPS and
/// deploys reset it hard — a database living under it is destroyed on every
/// deploy.
pub fn cortex_db_path(workspace_dir: &std::path::Path) -> PathBuf {
    let db_path = std::env::var("CORTEX_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace_dir.join(".cortex").join("cortex.db"));
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    db_path
}

pub struct AppState {
    pub providers: RwLock<Vec<ProviderStatus>>,
    pub ledger: Ledger,
    pub workspace_dir: PathBuf,
    /// Rendered repo maps, cached per workspace and token budget so the steps
    /// of one run share a single parse of the tree (CONTEXT.md C1). Replaced
    /// by the persistent incremental index in C2.
    pub repo_map_cache: cortex_context::cache::RepoMapCache,
    pub clerk_secret_key: Option<String>,
    /// Whether a worker with no credential may connect as `local`.
    ///
    /// Read once from `CORTEX_ALLOW_ANONYMOUS_WORKER` at construction and
    /// stored, rather than consulted per frame, so one connection cannot see
    /// two different answers and so a test can set it without mutating
    /// process-global environment. Default **off**: an unset
    /// `CLERK_SECRET_KEY` used to be enough to authenticate any worker, and it
    /// is no longer. See [`crate::worker_key`].
    pub allow_anonymous_worker: bool,
    pub jwks_cache: RwLock<JwksCache>,
    pub jwks_stampede: JwksStampedeGuard,
    pub db: Option<Database>,
    pub workers: RwLock<HashMap<String, ConnectedWorker>>,
    pub step_senders: RwLock<HashMap<String, mpsc::Sender<StepEvent>>>,
    pub scheduler_tx: RwLock<Option<mpsc::Sender<SchedulerEvent>>>,
    pub rate_limiter: Arc<RateLimiter>,
    pub billing_enforced: bool,
    pub usage_limits: UsageLimits,
    pub is_shutting_down: AtomicBool,
    /// Fingerprints of deployment states already recorded as `deploy.inspected`.
    ///
    /// Scoped to the app instance rather than the process. As a global it made
    /// integration tests order-dependent, since every test in a binary shared
    /// one set — and more importantly it meant a deployment returning to a
    /// previously seen state recorded nothing at all, silently, for the life of
    /// the process.
    pub recorded_deploy_events: Mutex<HashSet<String>>,
    /// Mission Control WebSocket subscribers, keyed by user_id.
    pub mc_subscribers: RwLock<HashMap<String, Vec<McSubscriber>>>,
    /// GitHub API client, initialized from `GITHUB_TOKEN` env var.
    pub github_client: Option<GitHubClient>,
    /// Cortex routing intelligence store (UCB bandit stats, evidence signals).
    pub cortex_store: Option<Mutex<CortexStore>>,
    /// UCB bandit scorer for adaptive provider selection.
    pub ucb_scorer: RwLock<UcbScorer>,
    /// Cortex's Soma heart — cryptographic identity for this agent.
    ///
    /// Absent from the struct entirely unless the `soma` feature is on, so a
    /// default build has no field for the execution path to read. See
    /// `crate::soma_fence`.
    #[cfg(feature = "soma")]
    pub soma_heart: Option<CortexHeart>,
    /// Stripe API client for billing operations.
    pub stripe_client: Option<crate::stripe_client::StripeClient>,
    /// Stripe webhook signing secret for verifying incoming events.
    pub stripe_webhook_secret: Option<String>,
    /// Vera observation layer — every Cortex interaction flows through here.
    pub vera_tracker: VeraTracker,
    /// Context-Flow Pipeline — enables AI models to feed each other.
    pub context_bus: ContextBus,
    /// Docker container manager for workspace/task-run container isolation.
    pub container_manager: Option<crate::docker::ContainerManager>,
    /// Live voice sessions this server is currently brokering, keyed by the
    /// OpenAI session id. Lets `DELETE /api/voice/live/sessions/{id}` find the
    /// owning user (so only they may close it) and the handle used to ask the
    /// billing task to send `session.close` on the sideband.
    pub voice_sessions: Mutex<HashMap<String, crate::voice_session::VoiceSessionHandle>>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StepEvent {
    Started {
        step_id: String,
        provider: String,
        model: String,
    },
    Output {
        step_id: String,
        line: String,
    },
    Completed {
        step_id: String,
        exit_code: i32,
    },
    Failed {
        step_id: String,
        error: String,
    },
    /// The paid chat agent called a read-only tool. Emitted between model
    /// turns (`chat_paid.rs`) so the UI can show what the agent looked at
    /// without waiting for the final reply. Carries only the tool's name and
    /// whether it succeeded — never its arguments or its result, which stay
    /// server-side data (see `agent_tools::render_tool_output`).
    ToolActivity {
        step_id: String,
        tool_name: String,
        ok: bool,
    },
    /// The model asked for a `Risk::Confirm` tool. Nothing ran — a row was
    /// written to `agent_pending_actions` (`db::pending_actions`) and this is
    /// the client's only copy of `nonce`, which it must send back to
    /// `POST /api/agent/actions/{action_id}/confirm` to actually run the
    /// tool. `summary` is written by the server from the tool's arguments
    /// and the database, never from model text (see `agent_tools.rs`).
    ConfirmRequired {
        action_id: String,
        nonce: String,
        summary: String,
        expires_at: i64,
    },
}

impl AppState {
    pub async fn new(
        ledger_path: impl Into<std::path::PathBuf>,
        workspace_dir: PathBuf,
        clerk_secret_key: Option<String>,
    ) -> Arc<Self> {
        let providers: Vec<ProviderStatus> = Vec::new();
        tracing::info!(
            "provider detection disabled — credentials are per-user via user_credentials table"
        );

        tracing::info!("workspace directory: {}", workspace_dir.display());
        if clerk_secret_key.is_some() {
            tracing::info!("clerk auth enabled");
        } else {
            tracing::info!("clerk auth disabled (no CLERK_SECRET_KEY)");
        }

        let db_path = cortex_db_path(&workspace_dir);
        let db = Database::open(&db_path);
        tracing::info!("database opened at {}", db_path.display());

        // Any live-voice reservation still `reserved` here was left
        // mid-flight by a server restart — the billing task and the
        // sideband-drop cleanup that would otherwise reconcile it both died
        // with the old process. Sweep it to `unresolved` now rather than
        // leaving it `reserved` (and its authorization exposure charged
        // against) forever.
        match db.sweep_stale_voice_reservations(chrono::Utc::now().timestamp_millis()) {
            Ok(0) => {}
            Ok(swept) => tracing::warn!(
                swept,
                "voice: marked stale reservation(s) unresolved after restart"
            ),
            Err(error) => {
                tracing::error!(%error, "voice: startup sweep of stale reservations failed")
            }
        }

        // Same idea for non-voice (chat, etc.) reservations: a `reserved`
        // row older than `STALE_RESERVATION_AGE_MS` at startup was left
        // mid-flight by a server restart or a crashed request, and nothing
        // else will ever resolve it. Age-bounded (unlike the voice sweep)
        // because a genuinely fresh `reserved` row could still belong to an
        // in-flight request from just before this process started.
        match db.sweep_stale_reservations(chrono::Utc::now().timestamp_millis()) {
            Ok(0) => {}
            Ok(swept) => tracing::warn!(
                swept,
                "provider gateway: marked stale reservation(s) unresolved after restart"
            ),
            Err(error) => {
                tracing::error!(%error, "provider gateway: startup sweep of stale reservations failed")
            }
        }
        db.warn_if_holds_over_threshold(chrono::Utc::now().timestamp_millis());

        // The anonymous worker path is off unless explicitly opened. Note it is
        // NOT tied to `CORTEX_AUTH_DISABLED`: losing a Clerk secret is an
        // accident, and this has to be a decision.
        let allow_anonymous_worker = crate::worker_key::anonymous_worker_allowed_from_env();
        if allow_anonymous_worker {
            tracing::warn!(
                "{}=1: workers may connect with no credential as `{}` - development only",
                crate::worker_key::ALLOW_ANONYMOUS_WORKER_ENV,
                crate::worker_key::ANONYMOUS_WORKER_USER,
            );
        }

        // Billing enforcement: default true, unless CORTEX_AUTH_DISABLED is set
        let auth_disabled = std::env::var("CORTEX_AUTH_DISABLED")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let billing_enforced = std::env::var("CORTEX_BILLING_ENFORCE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(!auth_disabled); // default true in production, false when auth disabled

        // Configurable usage limits
        let usage_limits = UsageLimits {
            daily_cost_limit: std::env::var("CORTEX_DAILY_COST_LIMIT")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(10.0),
            daily_step_limit: std::env::var("CORTEX_DAILY_STEP_LIMIT")
                .ok()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(100),
            monthly_cost_limit: std::env::var("CORTEX_MONTHLY_COST_LIMIT")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(200.0),
        };

        tracing::info!(
            "billing: enforced={}, daily_cost=${:.0}, daily_steps={}, monthly_cost=${:.0}",
            billing_enforced,
            usage_limits.daily_cost_limit,
            usage_limits.daily_step_limit,
            usage_limits.monthly_cost_limit,
        );

        let github_client = GitHubClient::from_env();
        if github_client.is_some() {
            tracing::info!("GitHub API client initialized (GITHUB_TOKEN set)");
        } else {
            tracing::info!(
                "GitHub API client not available (no GITHUB_TOKEN), will fall back to gh CLI"
            );
        }

        // routing.db is in the same danger as cortex.db, and slightly worse:
        // it is *untracked*, so `git clean -fd` in the deploy path deletes it
        // outright rather than reverting it. Losing it silently discards every
        // UCB arm statistic the router has learned in production — the system
        // keeps working and quietly gets worse at choosing, which is the least
        // debuggable kind of loss.
        let cortex_store_path = std::env::var("CORTEX_ROUTING_DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| workspace_dir.join(".cortex").join("routing.db"));
        if let Some(parent) = cortex_store_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let (cortex_store, ucb_scorer) = match CortexStore::open(&cortex_store_path) {
            Ok(store) => {
                let arm_stats = store.load_arm_stats().unwrap_or_default();
                let total_trials = arm_stats.values().map(|s| s.trials).sum();
                let scorer = UcbScorer {
                    arms: arm_stats,
                    exploration_weight: UcbScorer::exploration_weight_for_dial(5),
                    total_trials,
                };
                tracing::info!(
                    "cortex routing store opened at {} ({} arms, {} total trials)",
                    cortex_store_path.display(),
                    scorer.arms.len(),
                    scorer.total_trials,
                );
                (Some(Mutex::new(store)), scorer)
            }
            Err(e) => {
                tracing::warn!(
                    "failed to open cortex routing store: {e} — bandit scoring disabled"
                );
                (None, UcbScorer::new(1.0))
            }
        };

        #[cfg(feature = "soma")]
        let soma_heart = match CortexHeart::new() {
            Ok(heart) => {
                tracing::info!("soma heart alive — DID: {}", heart.did());
                Some(heart)
            }
            Err(e) => {
                tracing::error!("failed to initialize soma heart: {e}");
                None
            }
        };

        let (stripe_client, stripe_webhook_secret) =
            match crate::stripe_client::StripeClient::from_env() {
                Some((client, secret)) => {
                    tracing::info!("Stripe billing configured");
                    (Some(client), Some(secret))
                }
                None => {
                    tracing::info!("STRIPE_SECRET_KEY not set — billing stubs active");
                    (None, None)
                }
            };

        // The tracker needs a stable local identifier for "this Cortex". With
        // the Soma feature on it borrows the heart's DID; otherwise it comes
        // from a constant. Either way it is a local label for grouping
        // interactions — never a credential, and it never leaves the process.
        // That is why `VeraTracker` survives the fence and the heart does not.
        #[cfg(feature = "soma")]
        let cortex_heart_id = soma_heart
            .as_ref()
            .map(|h| {
                let did_bytes = h.did().as_bytes();
                let mut id = [0u8; 32];
                for (i, b) in did_bytes.iter().enumerate().take(32) {
                    id[i] = *b;
                }
                cortex_core::vera::HeartId(id)
            })
            .unwrap_or_else(default_cortex_heart_id);
        #[cfg(not(feature = "soma"))]
        let cortex_heart_id = default_cortex_heart_id();
        let vera_tracker = VeraTracker::new(cortex_heart_id);
        tracing::info!("vera tracker alive — cortex heart: {cortex_heart_id}");

        let context_config = ContextBusConfig::from_env();
        tracing::info!(
            "context-flow pipeline config: max_predecessors={}, max_total_tokens={}, target_tokens={:?}, summarize_code={}",
            context_config.max_predecessors,
            context_config.max_total_tokens,
            context_config.default_transform.target_tokens,
            context_config.default_transform.summarize_code,
        );
        let context_bus = ContextBus::new(context_config);
        tracing::info!("context-flow pipeline initialized — AI models can now feed each other");

        // Memory system removed for Context-Flow Pipeline deployment

        let container_manager = match crate::docker::ContainerManager::new() {
            Ok(cm) => {
                tracing::info!("Docker container manager initialized for BYOS");
                Some(cm)
            }
            Err(e) => {
                tracing::warn!("Docker not available — BYOS containers disabled: {e}");
                None
            }
        };

        Arc::new(Self {
            providers: RwLock::new(providers),
            ledger: Ledger::new(ledger_path),
            workspace_dir,
            repo_map_cache: cortex_context::cache::RepoMapCache::new(),
            clerk_secret_key,
            allow_anonymous_worker,
            jwks_cache: RwLock::new(JwksCache::empty()),
            jwks_stampede: JwksStampedeGuard::new(),
            db: Some(db),
            workers: RwLock::new(HashMap::new()),
            step_senders: RwLock::new(HashMap::new()),
            scheduler_tx: RwLock::new(None),
            rate_limiter: Arc::new(RateLimiter::default_per_user()),
            billing_enforced,
            usage_limits,
            is_shutting_down: AtomicBool::new(false),
            recorded_deploy_events: Mutex::new(HashSet::new()),
            mc_subscribers: RwLock::new(HashMap::new()),
            github_client,
            cortex_store,
            ucb_scorer: RwLock::new(ucb_scorer),
            #[cfg(feature = "soma")]
            soma_heart,
            stripe_client,
            stripe_webhook_secret,
            vera_tracker,
            context_bus,
            container_manager,
            voice_sessions: Mutex::new(HashMap::new()),
        })
    }

    /// Return a reference to the database as a trait object, enabling
    /// backend-agnostic code.  Returns `None` only if the database failed
    /// to open (should not happen in practice).
    pub fn storage(&self) -> Option<&dyn Storage> {
        self.db.as_ref().map(|d| d as &dyn Storage)
    }

    pub async fn register_worker(
        &self,
        worker_id: String,
        user_id: String,
        available_providers: Vec<ProviderId>,
        tx: mpsc::Sender<BrainMessage>,
    ) {
        tracing::info!("registering worker {worker_id} for user {user_id}");
        let mut workers = self.workers.write().await;
        workers.insert(
            worker_id.clone(),
            ConnectedWorker {
                worker_id,
                user_id,
                available_providers,
                disabled_providers: HashSet::new(),
                tx,
            },
        );
        tracing::info!("active workers: {}", workers.len());
    }

    pub async fn unregister_worker(&self, worker_id: &str) {
        let mut workers = self.workers.write().await;
        workers.remove(worker_id);
        tracing::info!("worker {worker_id} removed, active: {}", workers.len());
    }

    pub async fn find_worker_for_user(
        &self,
        user_id: &str,
    ) -> Option<(String, mpsc::Sender<BrainMessage>)> {
        let workers = self.workers.read().await;
        workers
            .iter()
            .find(|(_, w)| {
                w.user_id == user_id && {
                    let has_usable = w
                        .available_providers
                        .iter()
                        .any(|p| !w.disabled_providers.contains(p));
                    has_usable || w.available_providers.is_empty()
                }
            })
            .map(|(id, w)| (id.clone(), w.tx.clone()))
    }

    /// Mark a specific provider as unhealthy on a worker (e.g. auth expired).
    /// The worker will not be selected for dispatch if all its providers are disabled.
    pub async fn mark_provider_unhealthy(&self, worker_id: &str, provider: ProviderId) {
        let mut workers = self.workers.write().await;
        if let Some(w) = workers.get_mut(worker_id) {
            w.disabled_providers.insert(provider);
            tracing::warn!(
                "provider {} disabled on worker {} — disabled: {:?}, available: {:?}",
                provider,
                worker_id,
                w.disabled_providers,
                w.available_providers
            );
        }
    }

    /// Clear all disabled providers for a worker (called on re-registration).
    pub async fn clear_disabled_providers(&self, worker_id: &str) {
        let mut workers = self.workers.write().await;
        if let Some(w) = workers.get_mut(worker_id) {
            if !w.disabled_providers.is_empty() {
                tracing::info!(
                    "clearing disabled providers for worker {} (re-registered)",
                    worker_id
                );
                w.disabled_providers.clear();
            }
        }
    }

    pub async fn dispatch_step(
        &self,
        user_id: &str,
        task: TaskContract,
        decision: RoutingDecision,
        result_tx: mpsc::Sender<StepEvent>,
    ) -> Result<String, String> {
        let (worker_id, worker_tx) = self.find_worker_for_user(user_id).await.ok_or_else(|| {
            "no connected worker — run `npx cortex connect` in your environment".to_string()
        })?;

        let attempt_id = Uuid::new_v4().to_string();
        let lease_gen = 1;
        let now = chrono::Utc::now().timestamp_millis();
        let lease_deadline_ms = now + 600_000;

        // Register run + step in DB so worker ownership verification passes
        let (run_id, step_id) = if let Some(db) = &self.db {
            let rid = db.create_run(user_id, &task.objective, "chat", &[]);
            let sid = db.create_step(
                &rid,
                "chat",
                &format!("{:?}", decision.tier),
                &format!("{:?}", task.risk),
                &task.objective,
            );
            db.lease_step(&sid, &worker_id, lease_deadline_ms);
            (rid, sid)
        } else {
            (Uuid::new_v4().to_string(), Uuid::new_v4().to_string())
        };

        self.step_senders
            .write()
            .await
            .insert(step_id.clone(), result_tx);

        // Assemble context from previous step artifacts in this run
        let context = self
            .context_bus
            .assemble_context(self.db.as_ref(), &run_id, &task.objective, None)
            .await;

        // Captured before `decision` is moved into the frame below.
        let provider_egress = cortex_core::egress::derive_provider_egress(decision.provider);
        let provider_gateway = self.db.as_ref().and_then(|db| {
            crate::provider_gateway_http::issue_access(
                db,
                user_id,
                &run_id,
                &attempt_id,
                decision.provider,
                &decision.model_id,
                lease_deadline_ms,
                now,
            )
        });

        worker_tx
            .send(BrainMessage::ExecuteStep {
                run_id,
                step_id: step_id.clone(),
                attempt_id,
                lease_gen,
                lease_deadline_ms,
                workspace_id: "default".to_string(),
                base_commit: None,
                allowed_paths: vec![],
                task,
                decision,
                context,
                // Direct dispatch, with no plan-time repository probe behind
                // it. No repository, no manifest, no grant.
                egress: cortex_core::egress::EgressPlan::deny(),
                // The provider half does not depend on a repository probe, so
                // it applies here exactly as it does on the scheduler path.
                // Omitting it would leave this path with F7 — a sandboxed CLI
                // with no route to the model — which is the bug, not the
                // conservative choice.
                provider_egress,
                provider_gateway,
                delegation: None,
            })
            .await
            .map_err(|_| "worker connection lost".to_string())?;

        Ok(step_id)
    }

    pub async fn get_step_sender(&self, step_id: &str) -> Option<mpsc::Sender<StepEvent>> {
        self.step_senders.read().await.get(step_id).cloned()
    }

    pub async fn remove_step_sender(&self, step_id: &str) {
        self.step_senders.write().await.remove(step_id);
    }

    pub async fn set_scheduler_tx(&self, tx: mpsc::Sender<SchedulerEvent>) {
        *self.scheduler_tx.write().await = Some(tx);
    }

    pub async fn emit_scheduler_event(&self, event: SchedulerEvent) {
        if let Some(tx) = self.scheduler_tx.read().await.as_ref() {
            if tx.send(event).await.is_err() {
                tracing::error!("scheduler channel closed");
            }
        }
    }

    // --- Mission Control subscriber management ---

    pub async fn subscribe_mc(
        &self,
        user_id: &str,
        id: SubscriberId,
        tx: mpsc::Sender<MissionControlEvent>,
    ) {
        let mut subs = self.mc_subscribers.write().await;
        subs.entry(user_id.to_string())
            .or_default()
            .push(McSubscriber { id, tx });
        tracing::debug!("mc: subscribed {id:?} for user {user_id}");
    }

    pub async fn unsubscribe_mc(&self, user_id: &str, id: SubscriberId) {
        let mut subs = self.mc_subscribers.write().await;
        if let Some(list) = subs.get_mut(user_id) {
            list.retain(|s| s.id != id);
            if list.is_empty() {
                subs.remove(user_id);
            }
        }
        tracing::debug!("mc: unsubscribed {id:?} for user {user_id}");
    }

    /// Emit a Mission Control event to all subscribers for a given user.
    /// Drops subscribers whose channels are closed.
    pub async fn emit_mc_event(&self, user_id: &str, event: MissionControlEvent) {
        let subs = self.mc_subscribers.read().await;
        if let Some(list) = subs.get(user_id) {
            let mut closed = Vec::new();
            for sub in list {
                if sub.tx.try_send(event.clone()).is_err() {
                    // Channel full or closed — mark for removal
                    closed.push(sub.id);
                }
            }
            drop(subs);

            if !closed.is_empty() {
                let mut subs = self.mc_subscribers.write().await;
                if let Some(list) = subs.get_mut(user_id) {
                    list.retain(|s| !closed.contains(&s.id));
                    if list.is_empty() {
                        subs.remove(user_id);
                    }
                }
            }
        }
    }

    /// Graceful shutdown: cancel active steps, wait for workers, persist state.
    pub async fn shutdown(&self) {
        tracing::info!("initiating graceful shutdown");
        self.is_shutting_down.store(true, Ordering::SeqCst);

        // Send CancelStep to all workers for their active steps
        let workers = self.workers.read().await;
        let worker_count = workers.len();
        tracing::info!("shutting down {worker_count} connected worker(s)");

        for (worker_id, worker) in workers.iter() {
            // Look up active steps from DB if available
            let active_steps: Vec<String> = if let Some(db) = &self.db {
                db.get_worker_active_steps(worker_id)
            } else {
                Vec::new()
            };

            for step_id in &active_steps {
                tracing::info!("cancelling step {step_id} on worker {worker_id}");
                let _ = worker
                    .tx
                    .send(BrainMessage::CancelStep {
                        step_id: step_id.clone(),
                        reason: "server shutting down".to_string(),
                    })
                    .await;
            }
        }
        drop(workers);

        // Wait up to 30 seconds for workers to disconnect
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let count = self.workers.read().await.len();
            if count == 0 {
                tracing::info!("all workers disconnected cleanly");
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(
                    "{count} worker(s) still connected after 30s timeout, proceeding with shutdown"
                );
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }

        // Persist final state
        if let Some(db) = &self.db {
            // Mark any remaining leased steps as cancelled
            let remaining_steps: Vec<String> =
                self.step_senders.read().await.keys().cloned().collect();
            for step_id in &remaining_steps {
                tracing::info!("marking in-flight step {step_id} as cancelled in DB");
                db.cancel_assigned_step(step_id, "server shutdown");
            }
        }

        // Persist Soma state on shutdown
        #[cfg(feature = "soma")]
        if let Some(heart) = &self.soma_heart {
            heart.persist_heartbeats();
            heart.persist_spend_logs();
            heart.persist_invocation_counts();
        }

        let final_workers = self.workers.read().await.len();
        let final_steps = self.step_senders.read().await.len();
        tracing::info!(
            "shutdown complete: workers_remaining={final_workers}, steps_remaining={final_steps}"
        );
    }
}
