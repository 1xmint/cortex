use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use cortex_core::vera::{self, Capability, CompactedVera, HeartId, Interaction, SessionOutcome};

use crate::db::AttemptChain;
use crate::lock::LockRecovering;
use cortex_core::routing::Intent;
use cortex_core::verification::Verdict;

pub fn heart_id_from_user(user_id: &str) -> HeartId {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"soma-heart-v1:");
    hasher.update(user_id.as_bytes());
    let result = hasher.finalize();
    let mut id = [0u8; 32];
    id.copy_from_slice(&result);
    HeartId(id)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub struct VeraTracker {
    interactions: Mutex<Vec<Interaction>>,
    compacted: Mutex<Vec<CompactedVera>>,
    cortex_heart_id: HeartId,
}

#[derive(serde::Serialize)]
pub struct VeraSnapshot {
    pub total_interactions: usize,
    pub total_warmth: f64,
    pub active_hearts: u64,
    pub enrichment_ratio: f64,
    pub domains: Vec<DomainSnapshot>,
    pub compaction: CompactionSnapshot,
}

#[derive(serde::Serialize)]
pub struct DomainSnapshot {
    pub capability: String,
    pub warmth: f64,
    pub ignited: bool,
    pub interaction_count: usize,
}

#[derive(serde::Serialize)]
pub struct CompactionSnapshot {
    pub levels_computed: usize,
    pub latest: Option<CompactedVeraView>,
    pub history: Vec<CompactedVeraView>,
}

#[derive(Clone, serde::Serialize)]
pub struct CompactedVeraView {
    pub level: u32,
    pub vera: f64,
    pub coherence: f64,
    pub source_count: usize,
    pub consensus: f64,
    pub diversity: f64,
    pub stability: f64,
}

impl From<&CompactedVera> for CompactedVeraView {
    fn from(cv: &CompactedVera) -> Self {
        Self {
            level: cv.level,
            vera: cv.vera,
            coherence: cv.coherence,
            source_count: cv.source_count,
            consensus: cv.signal.consensus,
            diversity: cv.signal.diversity,
            stability: cv.signal.stability,
        }
    }
}

/// What a verdict is allowed to say about the model that produced the work.
///
/// The rule this encodes, from invariant 6: **only the independent verdict may
/// produce a positive reward.** Two of the four verdicts produce no signal at
/// all, and the reason is the same in both cases — neither is evidence about
/// the model:
///
/// - `Inconclusive` means a required check could not be run. That is our
///   infrastructure failing, not the model's work. Recording it as a failure
///   would penalise whichever model happened to be routed during our outage,
///   which is how a routing table learns to avoid a model for our reasons.
/// - `Unverified` means no executable ground truth was derivable. Nothing was
///   proven either way. It must not become a badge, and by the same argument it
///   must not become a penalty.
///
/// `None` is therefore a deliberate answer, not a missing case.
fn outcome_for_verdict(verdict: Verdict) -> Option<SessionOutcome> {
    match verdict {
        Verdict::Verified => Some(SessionOutcome::Success),
        Verdict::Failed => Some(SessionOutcome::Failure {
            error_class: "verification_failed".to_string(),
        }),
        Verdict::Inconclusive | Verdict::Unverified => None,
    }
}

const COMPACTION_THRESHOLD: usize = 10;

/// How many compacted values the tracker keeps.
///
/// `run_compaction` drains `interactions` every ten records, so that side is
/// self-limiting, but level-2 values were retained forever: one more every
/// hundred interactions, for the life of the process. Nothing read the old ones
/// — `snapshot()` serialises the whole history into every response to a public
/// endpoint, so an unbounded history is an unbounded response body as well as
/// unbounded memory. The tracker is an in-memory observability surface with no
/// persistence, so the oldest entries are droppable by construction.
const MAX_COMPACTION_HISTORY: usize = 256;

impl VeraTracker {
    pub fn new(cortex_heart_id: HeartId) -> Self {
        Self {
            interactions: Mutex::new(Vec::new()),
            compacted: Mutex::new(Vec::new()),
            cortex_heart_id,
        }
    }

    pub fn record_interaction(
        &self,
        user_heart_id: HeartId,
        capability: Capability,
        soma_amount: u64,
        outcome: SessionOutcome,
        duration_ms: u64,
    ) {
        let interaction = Interaction {
            from: user_heart_id,
            to: self.cortex_heart_id,
            capability,
            soma_amount,
            outcome,
            timestamp: now_ms(),
            duration_ms,
            participant_count: 2,
            bilateral: true,
        };

        let should_compact = {
            let mut interactions = self.interactions.lock_recovering();
            interactions.push(interaction);
            interactions.len() >= COMPACTION_THRESHOLD
        };

        if should_compact {
            self.run_compaction();
        }
    }

    fn run_compaction(&self) {
        let interactions: Vec<Interaction> = {
            let mut lock = self.interactions.lock_recovering();
            std::mem::take(&mut *lock)
        };

        if interactions.is_empty() {
            return;
        }

        let now = now_ms();
        let distilled = vera::distill(&interactions, now);

        tracing::info!(
            "vera compaction: {} interactions → level {} | vera={:.2} C={:.4} consensus={:.2} diversity={:.2} stability={:.2}",
            interactions.len(),
            distilled.level,
            distilled.vera,
            distilled.coherence,
            distilled.signal.consensus,
            distilled.signal.diversity,
            distilled.signal.stability,
        );

        let mut compacted = self.compacted.lock_recovering();
        compacted.push(distilled);

        // If we have enough level-1 compactions, distill them into level 2
        let level1_count = compacted.iter().filter(|c| c.level == 1).count();
        if level1_count >= COMPACTION_THRESHOLD {
            let level1s: Vec<CompactedVera> =
                compacted.iter().filter(|c| c.level == 1).cloned().collect();

            let level2 = vera::distill_compacted(&level1s);

            tracing::info!(
                "vera compaction level 2: {} level-1s → vera={:.2} C={:.4} consensus={:.2}",
                level1s.len(),
                level2.vera,
                level2.coherence,
                level2.signal.consensus,
            );

            compacted.retain(|c| c.level != 1);
            compacted.push(level2);
        }

        if compacted.len() > MAX_COMPACTION_HISTORY {
            let excess = compacted.len() - MAX_COMPACTION_HISTORY;
            compacted.drain(..excess);
        }
    }

    pub fn capability_for_intent(intent: Option<Intent>) -> Capability {
        match intent {
            Some(Intent::Explore) | Some(Intent::Review) => Capability::FileAccess,
            Some(Intent::Think) => Capability::Custom("Intelligence".into()),
            Some(Intent::Fix) | Some(Intent::Add) | Some(Intent::Refactor) | Some(Intent::Ship) => {
                Capability::CodeExecution
            }
            Some(Intent::Test) => Capability::Custom("Verification".into()),
            None => Capability::Custom("Conversation".into()),
        }
    }

    pub fn record_routed(&self, user_id: &str, intent: Intent) {
        self.record_interaction(
            heart_id_from_user(user_id),
            Self::capability_for_intent(Some(intent)),
            1,
            SessionOutcome::Success,
            0,
        );
    }

    /// The routing signal for one finished step, from the independent verdict.
    ///
    /// This replaces `record_step_completed`, which derived its outcome from
    /// the worker's exit code. Invariant 6 forbids a self-report from improving
    /// a score, so PR A stopped calling it and the positive signal has been
    /// suspended since. This is the restoration, and the difference that
    /// matters is the source: the verdict is produced by checks that ran
    /// against the delivered tree in a runner the task did not choose.
    ///
    /// **Spend is the whole attempt chain, not the winning attempt.** A step
    /// verified on its fourth try cost four dispatches. A signal that only sees
    /// the attempt that happened to succeed cannot tell a model that gets it
    /// right first time from one that needs coaxing — and the second is the
    /// expensive one, which is the thing the signal exists to notice.
    ///
    /// Returns whether a signal was emitted, so a caller can log the silence.
    pub fn record_verdict(
        &self,
        user_id: &str,
        verdict: Verdict,
        chain: AttemptChain,
        intent: Option<Intent>,
    ) -> bool {
        let Some(outcome) = outcome_for_verdict(verdict) else {
            return false;
        };
        self.record_interaction(
            heart_id_from_user(user_id),
            Self::capability_for_intent(intent),
            // Never zero: a step that reached a verdict was attempted at least
            // once, and a zero-mass interaction contributes nothing to V = S·C²
            // no matter how coherent it was — it would be recorded and then
            // silently ignored, which is worse than not recording it.
            chain.attempts.max(1),
            outcome,
            chain.total_duration_ms,
        );
        true
    }

    pub fn record_step_failed(&self, user_id: &str, failure_kind: &str, intent: Option<Intent>) {
        self.record_interaction(
            heart_id_from_user(user_id),
            Self::capability_for_intent(intent),
            1,
            SessionOutcome::Failure {
                error_class: failure_kind.to_string(),
            },
            0,
        );
    }

    pub fn record_conversation(&self, user_id: &str) {
        self.record_interaction(
            heart_id_from_user(user_id),
            Capability::Custom("Conversation".into()),
            1,
            SessionOutcome::Success,
            0,
        );
    }

    pub fn snapshot(&self) -> VeraSnapshot {
        let now = now_ms();

        let interactions = self.interactions.lock_recovering();
        let compacted = self.compacted.lock_recovering();

        let mut heart_ids = std::collections::HashSet::new();
        for i in interactions.iter() {
            heart_ids.insert(i.from);
            heart_ids.insert(i.to);
        }
        let active_hearts = heart_ids.len() as u64;

        let network = vera::compute_network(&interactions, active_hearts, now);

        let mut seen_capabilities: Vec<Capability> = Vec::new();
        for i in interactions.iter() {
            if !seen_capabilities.contains(&i.capability) {
                seen_capabilities.push(i.capability.clone());
            }
        }
        if seen_capabilities.is_empty() {
            seen_capabilities.push(Capability::CodeExecution);
        }

        let domains: Vec<DomainSnapshot> = seen_capabilities
            .iter()
            .map(|cap| {
                let warmth = vera::compute_warmth(&interactions, cap, now);
                let count = interactions.iter().filter(|i| i.capability == *cap).count();
                DomainSnapshot {
                    capability: match cap {
                        Capability::Custom(name) => name.clone(),
                        other => format!("{:?}", other),
                    },
                    warmth,
                    ignited: vera::is_ignited(&interactions, cap, now),
                    interaction_count: count,
                }
            })
            .collect();

        let history: Vec<CompactedVeraView> = compacted.iter().map(|cv| cv.into()).collect();
        let latest = history.last().cloned();

        VeraSnapshot {
            total_interactions: interactions.len(),
            total_warmth: network.total_warmth,
            active_hearts: network.active_hearts,
            enrichment_ratio: network.enrichment_ratio,
            domains,
            compaction: CompactionSnapshot {
                levels_computed: compacted.len(),
                latest,
                history,
            },
        }
    }

    /// The most interactions one `simulate_ecosystem` call may generate.
    ///
    /// The old caps were `agents <= 1000` and `per_agent <= 100`, applied
    /// independently — a product of 100,000 interactions, each one a SHA-256
    /// and a `format!`, run synchronously on an async handler. That occupies a
    /// Tokio worker thread for the whole run; enough concurrent calls starve the
    /// runtime, and every other request on the process — including the ones
    /// holding `Mutex<Connection>` — waits behind it.
    ///
    /// The bound is on the product, not on each factor, because it is the
    /// product that costs. 2,000 is more than enough to exercise the bottleneck
    /// (it distils 200 times and rolls up to level 2 twenty times) and completes
    /// in milliseconds.
    pub const MAX_SIMULATED_INTERACTIONS: usize = 2_000;

    /// Simulate diverse network traffic for testing the IB under realistic conditions.
    /// Interleaves agents like real traffic — different species observing at overlapping times.
    ///
    /// Bounded internally: the caller cannot ask for more than
    /// [`Self::MAX_SIMULATED_INTERACTIONS`]. Returns the number of interactions
    /// actually generated so a caller can tell that it was clamped.
    pub fn simulate_ecosystem(&self, agent_count: usize, interactions_per_agent: usize) -> usize {
        let agent_count = agent_count.clamp(1, Self::MAX_SIMULATED_INTERACTIONS);
        let interactions_per_agent =
            interactions_per_agent.clamp(1, Self::MAX_SIMULATED_INTERACTIONS / agent_count.max(1));

        let capabilities = [
            Capability::CodeExecution,
            Capability::FileAccess,
            Capability::NetworkAccess,
            Capability::Custom("Intelligence".into()),
            Capability::Custom("Verification".into()),
            Capability::Custom("Conversation".into()),
        ];

        let mut success_count = 0u64;
        let mut fail_count = 0u64;

        // Interleave: round-robin across agents, not sequential bursts.
        // Real traffic arrives mixed — the hawk and the bug observe simultaneously.
        for round in 0..interactions_per_agent {
            for agent_idx in 0..agent_count {
                let user_id = format!("agent-{agent_idx:04}");
                let heart = heart_id_from_user(&user_id);
                let cap = &capabilities[agent_idx % capabilities.len()];

                let outcome = if round % 7 == 0 {
                    fail_count += 1;
                    SessionOutcome::Failure {
                        error_class: "simulated".into(),
                    }
                } else {
                    success_count += 1;
                    SessionOutcome::Success
                };

                self.record_interaction(heart, cap.clone(), 1, outcome, (round as u64 + 1) * 100);
            }
        }

        let total = agent_count * interactions_per_agent;
        tracing::info!(
            "ecosystem simulation: {} agents × {} interactions = {} total ({} success, {} fail)",
            agent_count,
            interactions_per_agent,
            total,
            success_count,
            fail_count,
        );
        total
    }

    pub fn heart_trust(&self, heart_id: &HeartId, capability: &Capability) -> f64 {
        let now = now_ms();
        let interactions = self.interactions.lock_recovering();
        let relevant: Vec<Interaction> = interactions
            .iter()
            .filter(|i| (i.from == *heart_id || i.to == *heart_id) && i.capability == *capability)
            .cloned()
            .collect();
        vera::compute_trust(&relevant, capability, now)
    }

    /// Personal vera view — what the user sees about their contribution
    /// and what's flowing back from the network.
    pub fn personal_view(&self, user_id: &str) -> PersonalVeraView {
        let heart = heart_id_from_user(user_id);
        let now = now_ms();
        let interactions = self.interactions.lock_recovering();
        let compacted = self.compacted.lock_recovering();

        let my_interactions: Vec<&Interaction> = interactions
            .iter()
            .filter(|i| i.from == heart || i.to == heart)
            .collect();

        let my_count = my_interactions.len();
        let total_count = interactions.len();

        // What I contributed — my co-creation across capabilities
        let mut my_capabilities: Vec<PersonalCapability> = Vec::new();
        let mut seen_caps: Vec<Capability> = Vec::new();
        for i in my_interactions.iter() {
            if !seen_caps.contains(&i.capability) {
                seen_caps.push(i.capability.clone());
            }
        }
        for cap in &seen_caps {
            let cap_interactions: Vec<Interaction> = my_interactions
                .iter()
                .filter(|i| i.capability == *cap)
                .cloned()
                .cloned()
                .collect();
            let warmth = vera::compute_warmth(&cap_interactions, cap, now);
            let count = cap_interactions.len();
            my_capabilities.push(PersonalCapability {
                name: match cap {
                    Capability::Custom(name) => name.clone(),
                    other => format!("{:?}", other),
                },
                interactions: count,
                warmth,
            });
        }

        // Network totals — what everyone is building together
        let mut all_hearts = std::collections::HashSet::new();
        for i in interactions.iter() {
            all_hearts.insert(i.from);
            all_hearts.insert(i.to);
        }
        let network_hearts = all_hearts.len() as u64;
        let network = vera::compute_network(&interactions, network_hearts, now);

        // What's flowing back — the compacted intelligence I benefit from
        let latest_compaction: Option<CompactedVeraView> = compacted.last().map(|cv| cv.into());
        let network_coherence = latest_compaction
            .as_ref()
            .map(|c| c.coherence)
            .unwrap_or(0.0);

        PersonalVeraView {
            your_heart: hex::encode(heart.0),
            your_interactions: my_count,
            your_capabilities: my_capabilities,
            network_interactions: total_count,
            network_hearts,
            network_warmth: network.total_warmth,
            network_coherence,
            enrichment_ratio: network.enrichment_ratio,
            flowing_back: FlowingBack {
                intelligence_level: if network_coherence > 0.5 {
                    "diamond".into()
                } else if network_coherence > 0.1 {
                    "growing".into()
                } else {
                    "seeding".into()
                },
                your_contribution_pct: if total_count > 0 {
                    (my_count as f64 / total_count as f64) * 100.0
                } else {
                    0.0
                },
                proof: format!(
                    "V = S × C² | C = {:.4} | {} interactions compacted | formula is public, data is bilateral, computation is deterministic",
                    network_coherence,
                    total_count,
                ),
            },
        }
    }
}

#[derive(serde::Serialize)]
pub struct PersonalVeraView {
    pub your_heart: String,
    pub your_interactions: usize,
    pub your_capabilities: Vec<PersonalCapability>,
    pub network_interactions: usize,
    pub network_hearts: u64,
    pub network_warmth: f64,
    pub network_coherence: f64,
    pub enrichment_ratio: f64,
    pub flowing_back: FlowingBack,
}

#[derive(serde::Serialize)]
pub struct PersonalCapability {
    pub name: String,
    pub interactions: usize,
    pub warmth: f64,
}

#[derive(serde::Serialize)]
pub struct FlowingBack {
    pub intelligence_level: String,
    pub your_contribution_pct: f64,
    pub proof: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(attempts: u64, ms: u64) -> AttemptChain {
        AttemptChain {
            attempts,
            total_duration_ms: ms,
        }
    }

    fn tracker() -> VeraTracker {
        VeraTracker::new(heart_id_from_user("cortex"))
    }

    /// Invariant 6, stated as a test: nothing but a `Verified` verdict may
    /// produce a positive reward.
    ///
    /// The two silent verdicts are the point. `Inconclusive` is our
    /// infrastructure failing and `Unverified` is the absence of ground truth;
    /// recording either as a failure would teach the routing table to avoid a
    /// model for reasons that are not about the model.
    #[test]
    fn only_a_verified_verdict_rewards_and_only_a_failed_one_penalises() {
        assert!(matches!(
            outcome_for_verdict(Verdict::Verified),
            Some(SessionOutcome::Success)
        ));
        assert!(matches!(
            outcome_for_verdict(Verdict::Failed),
            Some(SessionOutcome::Failure { .. })
        ));
        assert!(outcome_for_verdict(Verdict::Inconclusive).is_none());
        assert!(outcome_for_verdict(Verdict::Unverified).is_none());
    }

    #[test]
    fn a_silent_verdict_records_nothing_at_all() {
        let tracker = tracker();
        for verdict in [Verdict::Inconclusive, Verdict::Unverified] {
            assert!(
                !tracker.record_verdict("user-1", verdict, chain(3, 900), None),
                "{verdict:?} must not emit a signal"
            );
        }
        assert_eq!(tracker.snapshot().total_interactions, 0);
    }

    /// The correction this task exists to make: the reward carries the cost of
    /// the whole attempt chain, not of the attempt that happened to succeed.
    #[test]
    fn the_reward_carries_the_whole_attempt_chain() {
        let tracker = tracker();
        assert!(tracker.record_verdict(
            "user-1",
            Verdict::Verified,
            chain(4, 12_000),
            Some(Intent::Fix)
        ));

        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.total_interactions, 1);
        // Mass is the attempt count, so a step that took four tries weighs four
        // times what a first-try step does — which is the whole point of a cost
        // signal.
        assert!(
            (snapshot.total_warmth - 4.0).abs() < 1e-6,
            "expected warmth 4.0 from a four-attempt chain, got {}",
            snapshot.total_warmth
        );
    }

    #[test]
    fn a_cheap_success_outweighs_nothing_it_should_not() {
        let one_try = tracker();
        one_try.record_verdict(
            "user-1",
            Verdict::Verified,
            chain(1, 500),
            Some(Intent::Fix),
        );
        let cheap = one_try.snapshot().total_warmth;

        let six_tries = tracker();
        six_tries.record_verdict(
            "user-2",
            Verdict::Verified,
            chain(6, 500),
            Some(Intent::Fix),
        );
        let expensive = six_tries.snapshot().total_warmth;

        assert!(
            expensive > cheap,
            "a six-attempt success must carry more spend than a one-attempt one"
        );
    }

    /// A step that reached a verdict was attempted at least once. Zero mass
    /// would be recorded and then contribute nothing to V = S·C², which is
    /// worse than not recording it — it looks like a signal and is not one.
    #[test]
    fn an_empty_chain_still_carries_one_unit_of_spend() {
        let tracker = tracker();
        assert!(tracker.record_verdict("user-1", Verdict::Verified, chain(0, 0), None));
        assert!(tracker.snapshot().total_warmth > 0.0);
    }

    /// The intent selects the domain the signal lands in, so an unknown intent
    /// must not be silently filed under a code-execution capability it was
    /// never classified as.
    #[test]
    fn the_intent_selects_the_domain() {
        let fixing = tracker();
        fixing.record_verdict("user-1", Verdict::Verified, chain(1, 0), Some(Intent::Fix));
        let domains = fixing.snapshot().domains;
        assert!(domains.iter().any(|d| d.capability == "CodeExecution"));

        let thinking = tracker();
        thinking.record_verdict(
            "user-1",
            Verdict::Verified,
            chain(1, 0),
            Some(Intent::Think),
        );
        let domains = thinking.snapshot().domains;
        assert!(domains.iter().any(|d| d.capability == "Intelligence"));
    }

    /// A failed verdict must move trust down while still counting as observed
    /// work — coherence is observation completeness, not judgement.
    #[test]
    fn a_failed_verdict_lowers_trust_without_erasing_the_observation() {
        let tracker = tracker();
        tracker.record_verdict(
            "user-1",
            Verdict::Failed,
            chain(2, 4_000),
            Some(Intent::Fix),
        );

        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.total_interactions, 1);
        // Warmth is unsigned observation mass, so the failure still shows up.
        assert!(snapshot.total_warmth > 0.0);
        // Trust is signed, so it does not.
        let trust = tracker.heart_trust(&heart_id_from_user("user-1"), &Capability::CodeExecution);
        assert!(
            trust < 0.0,
            "a failed verdict must lower trust, got {trust}"
        );
    }
}
