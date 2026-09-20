//! Context-Flow Pipeline - The Missing Wire in Cortex
//!
//! Implements the ContextBus system that lets AI models actually feed each other.
//! This is the core gap identified by Opus - currently steps get StepContext::default()
//! with empty predecessor_summaries. This module fills that gap.

use chrono::{DateTime, Utc};
use cortex_core::provenance::Provenance;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::db::Database;
use crate::lock::LockRecovering;
use cortex_core::protocol::{PredecessorSummary, StepContext};

/// Health status for Context-Flow Pipeline monitoring
#[derive(Debug, Clone, serde::Serialize)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    CircuitOpen,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ContextFlowHealth {
    pub status: HealthStatus,
    pub failure_count: u32,
    pub last_failure: Option<DateTime<Utc>>,
    pub circuit_breaker_open: bool,
    pub config: ContextFlowHealthConfig,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ContextFlowHealthConfig {
    pub max_predecessors: usize,
    pub max_total_tokens: u32,
    pub target_tokens: Option<u32>,
    pub summarize_code: bool,
}

/// Artifact produced by a completed step that can be consumed by downstream steps
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub producer_step_id: String,
    pub producer_run_id: String,
    pub kind: ArtifactKind,
    pub content: String,
    pub summary: String,
    pub files_changed: Vec<String>,
    pub confidence: f32,
    /// Where this artifact's content came from, so a downstream step that
    /// consumes it renders it as what it is.
    ///
    /// Always `AgentOutput` at creation: an artifact is a step's own account of
    /// its own work, which is unverified by construction however confident the
    /// summary sounds. Raising it to `VerifiedEvidence` is the verifier's job,
    /// once there is a verdict to raise it on.
    pub provenance: Provenance,
    pub tokens: u32,
    pub created_at: DateTime<Utc>,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Type of artifact produced by a step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArtifactKind {
    /// Final answer or result
    Answer,
    /// Code changes or implementation
    Code,
    /// Analysis or research findings
    Analysis,
    /// Plan or strategy
    Plan,
    /// Review or critique of previous work
    Review,
    /// Error or failure information
    Error,
    /// Intermediate working notes
    Working,
}

impl ArtifactKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ArtifactKind::Answer => "answer",
            ArtifactKind::Code => "code",
            ArtifactKind::Analysis => "analysis",
            ArtifactKind::Plan => "plan",
            ArtifactKind::Review => "review",
            ArtifactKind::Error => "error",
            ArtifactKind::Working => "working",
        }
    }
}

/// Configuration for how to transform an artifact for a downstream step
#[derive(Debug, Clone)]
pub struct ContextTransform {
    pub target_tokens: Option<u32>,
    pub include_metadata: bool,
    pub summarize_code: bool,
}

impl Default for ContextTransform {
    fn default() -> Self {
        Self {
            target_tokens: Some(500), // Reasonable default for context
            include_metadata: false,
            summarize_code: true,
        }
    }
}

/// Circuit breaker state for Context-Flow operations
#[derive(Debug, Clone)]
struct CircuitBreakerState {
    failure_count: Arc<std::sync::atomic::AtomicU32>,
    last_failure: Arc<std::sync::Mutex<Option<chrono::DateTime<Utc>>>>,
    is_open: Arc<std::sync::atomic::AtomicBool>,
}

impl Default for CircuitBreakerState {
    fn default() -> Self {
        Self {
            failure_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            last_failure: Arc::new(std::sync::Mutex::new(None)),
            is_open: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

impl CircuitBreakerState {
    fn record_success(&self) {
        self.failure_count
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.is_open
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }

    fn record_failure(&self) {
        let count = self
            .failure_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        *self.last_failure.lock_recovering() = Some(Utc::now());

        // Open circuit after 5 consecutive failures
        if count >= 5 {
            self.is_open
                .store(true, std::sync::atomic::Ordering::Relaxed);
            tracing::error!(
                "context-flow circuit breaker opened after {} consecutive failures",
                count
            );
        }
    }

    fn should_attempt(&self) -> bool {
        if !self.is_open.load(std::sync::atomic::Ordering::Relaxed) {
            return true;
        }

        // Check if we should try again (half-open state)
        if let Some(last_failure) = *self.last_failure.lock_recovering() {
            let recovery_window = chrono::Duration::minutes(5);
            if Utc::now().signed_duration_since(last_failure) > recovery_window {
                tracing::info!("context-flow circuit breaker attempting recovery");
                return true;
            }
        }

        false
    }
}

/// Bus that collects artifacts from completed steps and provides context for new steps
pub struct ContextBus {
    /// Configuration for context assembly
    pub config: ContextBusConfig,
    /// Circuit breaker for failure tracking
    circuit_breaker: CircuitBreakerState,
}

#[derive(Debug, Clone)]
pub struct ContextBusConfig {
    /// Maximum number of predecessor summaries to include
    pub max_predecessors: usize,
    /// Maximum tokens for all predecessor summaries combined
    pub max_total_tokens: u32,
    /// Default transform to apply when assembling context
    pub default_transform: ContextTransform,
}

impl Default for ContextBusConfig {
    fn default() -> Self {
        Self {
            max_predecessors: 5,
            max_total_tokens: 2000,
            default_transform: ContextTransform::default(),
        }
    }
}

impl ContextBusConfig {
    /// Create config from environment variables with sensible defaults
    pub fn from_env() -> Self {
        let max_predecessors = std::env::var("CORTEX_CONTEXT_MAX_PREDECESSORS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(5);

        let max_total_tokens = std::env::var("CORTEX_CONTEXT_MAX_TOKENS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(2000);

        let target_tokens = std::env::var("CORTEX_CONTEXT_TARGET_TOKENS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok());

        let summarize_code = std::env::var("CORTEX_CONTEXT_SUMMARIZE_CODE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);

        let default_transform = ContextTransform {
            target_tokens,
            include_metadata: false, // Keep default
            summarize_code,
        };

        Self {
            max_predecessors,
            max_total_tokens,
            default_transform,
        }
    }
}

impl ContextBus {
    pub fn new(config: ContextBusConfig) -> Self {
        Self {
            config,
            circuit_breaker: CircuitBreakerState::default(),
        }
    }

    /// Add an artifact from a completed step
    pub async fn add_artifact(&self, db: Option<&Database>, artifact: Artifact) {
        if let Some(db) = db {
            match std::panic::catch_unwind(|| {
                db.store_context_artifact(
                    &artifact.id,
                    &artifact.producer_step_id,
                    &artifact.producer_run_id,
                    artifact.kind.as_str(),
                    &artifact.content,
                    &artifact.summary,
                    &artifact.files_changed,
                    artifact.confidence,
                    artifact.tokens,
                    artifact.created_at.timestamp_millis(),
                    &serde_json::to_value(&artifact.metadata).unwrap_or(serde_json::json!({})),
                )
            }) {
                Ok(()) => {
                    tracing::debug!(
                        "artifact stored successfully: id={}, step_id={}, run_id={}, kind={}, tokens={}",
                        artifact.id,
                        artifact.producer_step_id,
                        artifact.producer_run_id,
                        artifact.kind.as_str(),
                        artifact.tokens
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "failed to store context artifact: id={}, step_id={}, run_id={}, error={:?}",
                        artifact.id,
                        artifact.producer_step_id,
                        artifact.producer_run_id,
                        e
                    );
                    // Continue execution - artifact storage failure should not break step completion
                }
            }
        } else {
            tracing::debug!(
                "skipping artifact storage (no database): step_id={}, run_id={}",
                artifact.producer_step_id,
                artifact.producer_run_id
            );
        }
    }

    /// Assemble context for a new step from previous artifacts in the same run
    ///
    /// This method includes comprehensive error handling and will return a valid StepContext
    /// even if parts of the context assembly fail. This ensures step execution continues.
    pub async fn assemble_context(
        &self,
        db: Option<&Database>,
        run_id: &str,
        user_goal: &str,
        conversation_excerpt: Option<String>,
    ) -> StepContext {
        // Add timeout to prevent hanging
        let timeout_duration = std::time::Duration::from_secs(30);

        // Check circuit breaker
        if !self.circuit_breaker.should_attempt() {
            tracing::warn!(
                "context assembly skipped due to circuit breaker for run_id={}, using empty context",
                run_id
            );
            return StepContext {
                predecessor_summaries: Vec::new(),
                user_goal: user_goal.to_string(),
                conversation_excerpt,
                repo_map: None,
            };
        }

        match tokio::time::timeout(
            timeout_duration,
            self.assemble_context_inner(db, run_id, user_goal, conversation_excerpt.clone()),
        )
        .await
        {
            Ok(context_result) => match context_result {
                Ok(context) => {
                    self.circuit_breaker.record_success();
                    context
                }
                Err(e) => {
                    tracing::error!(
                        "context assembly failed for run_id={}, falling back to empty context: {}",
                        run_id,
                        e
                    );
                    self.circuit_breaker.record_failure();
                    StepContext {
                        predecessor_summaries: Vec::new(),
                        user_goal: user_goal.to_string(),
                        conversation_excerpt,
                        repo_map: None,
                    }
                }
            },
            Err(_) => {
                tracing::error!(
                    "context assembly timed out after {}s for run_id={}, falling back to empty context",
                    timeout_duration.as_secs(),
                    run_id
                );
                self.circuit_breaker.record_failure();
                StepContext {
                    predecessor_summaries: Vec::new(),
                    user_goal: user_goal.to_string(),
                    conversation_excerpt,
                    repo_map: None,
                }
            }
        }
    }

    /// Internal context assembly implementation with detailed error handling
    async fn assemble_context_inner(
        &self,
        db: Option<&Database>,
        run_id: &str,
        user_goal: &str,
        conversation_excerpt: Option<String>,
    ) -> Result<StepContext, String> {
        let artifacts = if let Some(db) = db {
            // Load artifacts from database with error handling
            match std::panic::catch_unwind(|| db.get_context_artifacts_for_run(run_id)) {
                Ok(db_artifacts) => {
                    tracing::debug!(
                        "loaded {} artifacts from database for run_id={}",
                        db_artifacts.len(),
                        run_id
                    );

                    db_artifacts
                        .into_iter()
                        .filter_map(
                            |(
                                id,
                                producer_step_id,
                                kind,
                                content,
                                summary,
                                files_changed,
                                confidence,
                                tokens,
                                created_at,
                            )| {
                                // Validate artifact data and handle corrupted entries
                                if content.is_empty() && summary.is_empty() {
                                    tracing::warn!(
                                        "skipping empty artifact: id={}, step_id={}, run_id={}",
                                        id,
                                        producer_step_id,
                                        run_id
                                    );
                                    return None;
                                }

                                // Validate confidence range
                                let validated_confidence = if !(0.0..=1.0).contains(&confidence) {
                                    tracing::warn!(
                                    "artifact has invalid confidence {}, clamping to 0.5: id={}",
                                    confidence, id
                                );
                                    0.5
                                } else {
                                    confidence
                                };

                                // Validate tokens
                                let validated_tokens = if tokens == 0 && !content.is_empty() {
                                    // Estimate tokens if missing
                                    (content.len() / 4).max(1) as u32
                                } else {
                                    tokens
                                };

                                // Parse artifact kind safely
                                let artifact_kind = match kind.as_str() {
                                    "answer" => ArtifactKind::Answer,
                                    "code" => ArtifactKind::Code,
                                    "analysis" => ArtifactKind::Analysis,
                                    "plan" => ArtifactKind::Plan,
                                    "review" => ArtifactKind::Review,
                                    "error" => ArtifactKind::Error,
                                    "working" => ArtifactKind::Working,
                                    unknown => {
                                        tracing::warn!(
                                        "unknown artifact kind '{}', defaulting to 'answer': id={}",
                                        unknown, id
                                    );
                                        ArtifactKind::Answer
                                    }
                                };

                                // Parse timestamp safely
                                let created_at = DateTime::from_timestamp_millis(created_at)
                                    .unwrap_or_else(|| {
                                        tracing::warn!(
                                        "invalid timestamp {} for artifact {}, using current time",
                                        created_at, id
                                    );
                                        Utc::now()
                                    });

                                // Reconstructed rather than read: provenance is
                                // not a stored column, and every persisted artifact
                                // is a step's own account of its own work. Deriving
                                // it here keeps the load path from producing an
                                // artifact that outranks the one that created it.
                                let provenance = Provenance::AgentOutput {
                                    producer_step_id: producer_step_id.clone(),
                                };
                                Some(Artifact {
                                    id,
                                    producer_step_id,
                                    producer_run_id: run_id.to_string(),
                                    kind: artifact_kind,
                                    content,
                                    summary,
                                    files_changed,
                                    confidence: validated_confidence,
                                    provenance,
                                    tokens: validated_tokens,
                                    created_at,
                                    metadata: HashMap::new(),
                                })
                            },
                        )
                        .collect()
                }
                Err(e) => {
                    return Err(format!(
                        "database error loading artifacts for run_id={}: {:?}",
                        run_id, e
                    ));
                }
            }
        } else {
            tracing::debug!(
                "no database available, using empty context for run_id={}",
                run_id
            );
            Vec::new()
        };

        // Transform artifacts into predecessor summaries
        let mut predecessor_summaries = Vec::new();
        let mut total_tokens = 0u32;

        for artifact in artifacts {
            if predecessor_summaries.len() >= self.config.max_predecessors {
                break;
            }

            let summary_tokens = artifact
                .tokens
                .min(self.config.default_transform.target_tokens.unwrap_or(500));

            if total_tokens + summary_tokens > self.config.max_total_tokens {
                break;
            }

            let summary_content = if summary_tokens < artifact.tokens
                && self.config.default_transform.summarize_code
            {
                // TODO: Implement smart summarization (could use a lightweight model)
                self.truncate_to_tokens(&artifact.content, summary_tokens)
            } else {
                artifact.content.clone()
            };

            predecessor_summaries.push(PredecessorSummary {
                step_id: artifact.producer_step_id.clone(),
                kind: artifact.kind.as_str().to_string(),
                summary: format!("{}\n\n{}", artifact.summary, summary_content),
                files_changed: artifact.files_changed.clone(),
            });

            total_tokens += summary_tokens;
        }

        let context = StepContext {
            predecessor_summaries,
            user_goal: user_goal.to_string(),
            conversation_excerpt,
            repo_map: None,
        };

        tracing::debug!(
            "context assembled successfully for run_id={}: {} predecessors, {} total tokens",
            run_id,
            context.predecessor_summaries.len(),
            context
                .predecessor_summaries
                .iter()
                .map(|s| s.summary.len() / 4)
                .sum::<usize>()
        );

        Ok(context)
    }

    /// Simple token-aware truncation (rough approximation)
    fn truncate_to_tokens(&self, text: &str, target_tokens: u32) -> String {
        // Rough approximation: 4 characters per token
        let target_chars = (target_tokens * 4) as usize;
        if text.len() <= target_chars {
            text.to_string()
        } else {
            format!("{}...", &text[..target_chars.saturating_sub(3)])
        }
    }

    /// Create an artifact from step completion data
    pub fn create_artifact(
        step_id: &str,
        run_id: &str,
        kind: ArtifactKind,
        content: &str,
        summary: &str,
        files_changed: Vec<String>,
        confidence: f32,
    ) -> Artifact {
        Artifact {
            id: Uuid::new_v4().to_string(),
            producer_step_id: step_id.to_string(),
            producer_run_id: run_id.to_string(),
            kind,
            content: content.to_string(),
            summary: summary.to_string(),
            files_changed,
            confidence,
            provenance: Provenance::AgentOutput {
                producer_step_id: step_id.to_string(),
            },
            tokens: Self::estimate_tokens(content),
            created_at: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    /// Rough token estimation (4 chars per token)
    fn estimate_tokens(text: &str) -> u32 {
        (text.len() / 4).max(1) as u32
    }

    /// Clear artifacts for a completed run to save memory
    pub async fn cleanup_run(&self, db: Option<&Database>, run_id: &str) {
        if let Some(db) = db {
            match std::panic::catch_unwind(|| db.cleanup_context_artifacts_for_run(run_id)) {
                Ok(()) => {
                    tracing::debug!("cleaned up artifacts for run_id={}", run_id);
                }
                Err(e) => {
                    tracing::error!("failed to cleanup artifacts for run_id={}: {:?}", run_id, e);
                }
            }
        }
    }

    /// Get health status of the Context-Flow Pipeline
    pub fn get_health_status(&self) -> ContextFlowHealth {
        let is_circuit_open = self
            .circuit_breaker
            .is_open
            .load(std::sync::atomic::Ordering::Relaxed);
        let failure_count = self
            .circuit_breaker
            .failure_count
            .load(std::sync::atomic::Ordering::Relaxed);
        let last_failure = *self.circuit_breaker.last_failure.lock_recovering();

        let status = if is_circuit_open {
            HealthStatus::CircuitOpen
        } else if failure_count > 0 {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        ContextFlowHealth {
            status,
            failure_count,
            last_failure,
            circuit_breaker_open: is_circuit_open,
            config: ContextFlowHealthConfig {
                max_predecessors: self.config.max_predecessors,
                max_total_tokens: self.config.max_total_tokens,
                target_tokens: self.config.default_transform.target_tokens,
                summarize_code: self.config.default_transform.summarize_code,
            },
        }
    }
}
