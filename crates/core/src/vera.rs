//! The trust and compaction arithmetic behind Cortex's routing signal.
//!
//! This is Cortex-owned code. It was copied out of `soma-core` rather than
//! depended on, and the reason is a dependency-graph fact rather than a
//! preference: `soma-core` depends on `soma-crypto`, unconditionally. As long as
//! `crates/api` depended on `soma-core`, `soma-crypto` was linked into every
//! default Cortex build — including one built with `--no-default-features` —
//! which made the Soma feature fence half a fence. `soma-crypto` is also the
//! crate whose test vectors assert nothing (see EXECUTION-STATE F5), so the
//! half that leaked through was the unverified half.
//!
//! Nothing here needs a crypto crate. It is sums, products, exponential decay,
//! and an autocorrelation. There are no keys, no signatures, no identity, no
//! delegation, and no network — that claim is about *this file*, and it is
//! checkable by reading it, which is the property the old comment on
//! `crates/api/Cargo.toml` claimed for a crate where it was not true.
//!
//! The arithmetic is a verbatim copy of `soma-core`'s `trust`, `compaction`, and
//! `vera` modules as of `121e312d`, so a value computed here is the value that
//! was computed before. The type names (`HeartId`, `Capability`,
//! `SessionOutcome`) are inherited Soma vocabulary, kept as-is deliberately: a
//! rename would have made the copy unauditable against its source. Renaming them
//! to Cortex vocabulary is a separate, mechanical change.
//!
//! `crates/soma-core` itself is untouched and still builds; it simply has no
//! consumer in the Cortex binaries any more.

use serde::{Deserialize, Serialize};

// === Identifiers ===

/// A 32-byte actor identifier. Cortex derives one per user by hashing the user
/// id; it is a grouping key for interactions, not a credential.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct HeartId(pub [u8; 32]);

impl core::fmt::Display for HeartId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl From<[u8; 32]> for HeartId {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<HeartId> for [u8; 32] {
    fn from(id: HeartId) -> Self {
        id.0
    }
}

/// The kind of work an interaction exercised. A routing domain label, not an
/// authorization grant — nothing in Cortex consults this to decide whether an
/// action is permitted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Capability {
    CodeExecution,
    FileAccess,
    NetworkAccess,
    DelegateAuthority,
    SpendSoma,
    Custom(String),
}

/// How an interaction ended. Affects the *sign* of trust, never coherence.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SessionOutcome {
    Success,
    Failure { error_class: String },
    Partial { completed: f32 },
}

// === Trust ===

/// One equation: $VERA = $SOMA × C²
///
/// Coherence is observation completeness — how fully was this interaction seen?
/// Not quality. Not judgment. Observation density.
///
/// Outcome is NOT coherence. A failure is just as observed as a success.
/// Outcome affects the sign on trust, not whether Vera sees you.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Interaction {
    pub from: HeartId,
    pub to: HeartId,
    pub capability: Capability,
    pub soma_amount: u64,
    pub outcome: SessionOutcome,
    pub timestamp: u64,
    pub duration_ms: u64,
    pub participant_count: u32,
    pub bilateral: bool,
}

pub const TEMPORAL_DECAY: f64 = 0.01;
pub const HIGH_STAKES_TEMPORAL_DECAY: f64 = 0.02;

pub fn coherence(interaction: &Interaction, now: u64) -> f64 {
    let bilateral = if interaction.bilateral { 1.0 } else { 0.0 };
    let days_elapsed = (now.saturating_sub(interaction.timestamp)) as f64 / 86_400_000.0;
    let temporal = (-TEMPORAL_DECAY * days_elapsed).exp();
    bilateral * temporal
}

pub fn vera(interaction: &Interaction, now: u64) -> f64 {
    let soma = interaction.soma_amount as f64;
    let c = coherence(interaction, now);
    soma * c * c
}

pub fn compute_trust(interactions: &[Interaction], capability: &Capability, now: u64) -> f64 {
    interactions
        .iter()
        .filter(|i| i.capability == *capability)
        .map(|i| {
            let soma = i.soma_amount as f64;
            let c = coherence(i, now);
            let sign = match &i.outcome {
                SessionOutcome::Success => 1.0,
                SessionOutcome::Partial { completed } => *completed as f64,
                SessionOutcome::Failure { .. } => -1.0,
            };
            sign * soma * c * c
        })
        .sum()
}

pub fn compute_warmth(interactions: &[Interaction], capability: &Capability, now: u64) -> f64 {
    interactions
        .iter()
        .filter(|i| i.capability == *capability)
        .map(|i| vera(i, now))
        .sum()
}

pub fn is_ignited(interactions: &[Interaction], capability: &Capability, now: u64) -> bool {
    let signal = compute_warmth(interactions, capability, now);
    let noise = interactions
        .iter()
        .filter(|i| i.capability == *capability)
        .map(|i| {
            let soma = i.soma_amount as f64;
            let c = coherence(i, now);
            let v = vera(i, now);
            let mean = signal / interactions.len().max(1) as f64;
            let deviation = v - mean;
            deviation * deviation * soma * (1.0 - c)
        })
        .sum::<f64>()
        .sqrt();

    if noise <= f64::EPSILON {
        return signal > 0.0;
    }

    signal / noise > 1.0
}

pub fn compute_velocity(
    interactions: &[Interaction],
    capability: &Capability,
    now: u64,
    window_days: u64,
) -> f64 {
    let window_ms = window_days * 86_400_000;
    let cutoff = now.saturating_sub(window_ms);

    let recent_vera: f64 = interactions
        .iter()
        .filter(|i| i.capability == *capability && i.timestamp >= cutoff)
        .map(|i| vera(i, now))
        .sum();

    recent_vera / window_days.max(1) as f64
}

// === Network aggregate ===

pub struct NetworkVera {
    pub total_warmth: f64,
    pub active_hearts: u64,
    pub enrichment_ratio: f64,
}

pub fn network_warmth(interactions: &[Interaction], now: u64) -> f64 {
    interactions.iter().map(|i| vera(i, now)).sum()
}

pub fn compute_network(interactions: &[Interaction], active_hearts: u64, now: u64) -> NetworkVera {
    let total_warmth = network_warmth(interactions, now);
    NetworkVera {
        total_warmth,
        active_hearts,
        enrichment_ratio: if active_hearts > 0 {
            total_warmth / active_hearts as f64
        } else {
            0.0
        },
    }
}

// === Compaction ===

/// Compaction rooted in the Information Bottleneck principle (Tishby 1999).
///
/// The IB method: compress X while preserving information about Y.
/// minimize I(X;T) - β·I(T;Y)
///
/// For Vera: X = raw observations, Y = trust-relevant signal,
/// T = compacted representation.
///
/// A compacted observation at any level — the representation that survived the
/// bottleneck.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompactedVera {
    pub level: u32,
    /// The $VERA that survived compaction: mass × coherence²
    pub vera: f64,
    /// Coherence at this compaction level — how much of the input was
    /// trust-relevant signal vs noise.
    pub coherence: f64,
    /// How many observations were distilled into this value
    pub source_count: usize,
    /// The signal profile — what the compaction revealed
    pub signal: SignalProfile,
    pub timestamp: u64,
}

/// What the Information Bottleneck extracted — the signal that survived
/// compression.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignalProfile {
    /// Outcome consistency: do observers agree? Range [-1, 1].
    /// +1 = all success, -1 = all failure, 0 = no signal.
    pub consensus: f64,
    /// Observer diversity: how many independent sources?
    /// High diversity + high consensus = diamond.
    /// High diversity + low consensus = noise.
    /// Low diversity + high consensus = potentially gamed.
    pub diversity: f64,
    /// Temporal stability: does the signal hold over time?
    /// Measured as autocorrelation of outcomes.
    pub stability: f64,
}

impl SignalProfile {
    const fn empty() -> Self {
        Self {
            consensus: 0.0,
            diversity: 0.0,
            stability: 0.0,
        }
    }
}

/// Extract the trust-relevant signal from a set of interactions.
/// This is the "Y" in the Information Bottleneck.
fn extract_signal(interactions: &[Interaction]) -> SignalProfile {
    if interactions.is_empty() {
        return SignalProfile::empty();
    }

    let n = interactions.len() as f64;

    // Consensus: mean outcome sign
    let signs: Vec<f64> = interactions
        .iter()
        .map(|i| match &i.outcome {
            SessionOutcome::Success => 1.0,
            SessionOutcome::Failure { .. } => -1.0,
            SessionOutcome::Partial { completed } => (*completed as f64) * 2.0 - 1.0,
        })
        .collect();

    let consensus = signs.iter().sum::<f64>() / n;

    // Diversity: how many independent observers?
    // 100 interactions from 100 agents = 1.0 (fully independent).
    // 100 interactions from 1 agent = 0.01 (one observer, repeated).
    let mut unique_from = std::collections::HashSet::new();
    for i in interactions {
        unique_from.insert(i.from);
    }
    let diversity = (unique_from.len() as f64 / n).min(1.0);

    let stability = autocorrelation(&signs, consensus);

    SignalProfile {
        consensus,
        diversity,
        stability,
    }
}

/// Lag-1 autocorrelation of a series about its mean, clamped to [-1, 1].
/// A constant series is perfectly stable; a single sample is stable with itself.
fn autocorrelation(series: &[f64], mean: f64) -> f64 {
    if series.len() < 2 {
        return 1.0;
    }
    let mut num = 0.0;
    let mut den = 0.0;
    for i in 0..series.len() {
        den += (series[i] - mean) * (series[i] - mean);
        if i + 1 < series.len() {
            num += (series[i] - mean) * (series[i + 1] - mean);
        }
    }
    if den.abs() < f64::EPSILON {
        1.0
    } else {
        (num / den).clamp(-1.0, 1.0)
    }
}

/// The Information Bottleneck coherence: how much of the raw observation data is
/// trust-relevant signal?
///
/// High consensus from diverse independent sources = high C.
/// High consensus from few sources = lower C (could be gamed).
/// Low consensus from any sources = low C (noise).
fn bottleneck_coherence(signal: &SignalProfile) -> f64 {
    // Consensus strength: absolute value — consistent failure is as coherent as
    // consistent success. Coherence is observation completeness, not judgment.
    let consensus_strength = signal.consensus.abs();

    // Diverse independent observers make the consensus more trustworthy.
    let diversity_factor = signal.diversity;

    // Signal that holds over time is more coherent than signal that fluctuates.
    let stability_factor = (signal.stability + 1.0) / 2.0; // map [-1,1] to [0,1]

    // Multiplicative, not additive — you cannot compensate for low diversity
    // with high consensus. Missing any one kills the signal quadratically
    // (through C²).
    (consensus_strength * diversity_factor * stability_factor).clamp(0.0, 1.0)
}

/// Compact raw interactions through the Information Bottleneck.
/// Same function for level 0→1 as for level N→N+1.
pub fn distill(interactions: &[Interaction], now: u64) -> CompactedVera {
    if interactions.is_empty() {
        return CompactedVera {
            level: 1,
            vera: 0.0,
            coherence: 0.0,
            source_count: 0,
            signal: SignalProfile::empty(),
            timestamp: now,
        };
    }

    // Input mass: sum of $VERA from raw interactions
    let soma: f64 = interactions.iter().map(|i| vera(i, now)).sum();

    // Extract the trust-relevant signal (the Y in IB)
    let signal = extract_signal(interactions);

    // Coherence through the bottleneck
    let c = bottleneck_coherence(&signal);

    // The one equation: $VERA = $SOMA × C²
    CompactedVera {
        level: 1,
        vera: soma * c * c,
        coherence: c,
        source_count: interactions.len(),
        signal,
        timestamp: now,
    }
}

/// Compact compacted values — same mechanism, higher level. The output of
/// distillation becomes input for the next distillation.
pub fn distill_compacted(inputs: &[CompactedVera]) -> CompactedVera {
    if inputs.is_empty() {
        return CompactedVera {
            level: 1,
            vera: 0.0,
            coherence: 0.0,
            source_count: 0,
            signal: SignalProfile::empty(),
            timestamp: 0,
        };
    }

    let max_level = inputs.iter().map(|cv| cv.level).max().unwrap_or(0);
    let latest = inputs.iter().map(|cv| cv.timestamp).max().unwrap_or(0);
    let total_sources: usize = inputs.iter().map(|cv| cv.source_count).sum();

    // Input mass: sum of $VERA from lower level
    let soma: f64 = inputs.iter().map(|cv| cv.vera).sum();

    let signal = extract_signal_from_compacted(inputs);
    let c = bottleneck_coherence(&signal);

    CompactedVera {
        level: max_level + 1,
        vera: soma * c * c,
        coherence: c,
        source_count: total_sources,
        signal,
        timestamp: latest,
    }
}

/// Extract signal profile from compacted values (higher-level distillation).
fn extract_signal_from_compacted(inputs: &[CompactedVera]) -> SignalProfile {
    if inputs.is_empty() {
        return SignalProfile::empty();
    }

    let n = inputs.len() as f64;

    // Consensus: agreement of compacted consensus signals
    let consensus = inputs.iter().map(|cv| cv.signal.consensus).sum::<f64>() / n;

    // Diversity: how many distinct source populations contributed?
    let total_sources: usize = inputs.iter().map(|cv| cv.source_count).sum();
    let diversity = if total_sources > 0 {
        (inputs.len() as f64 / total_sources as f64)
            .min(1.0)
            .max(inputs.iter().map(|cv| cv.signal.diversity).sum::<f64>() / n)
    } else {
        0.0
    };

    // Stability: autocorrelation of consensus values across inputs
    let consensuses: Vec<f64> = inputs.iter().map(|cv| cv.signal.consensus).collect();
    let stability = autocorrelation(&consensuses, consensus);

    SignalProfile {
        consensus,
        diversity,
        stability,
    }
}

/// Density: $VERA per source interaction.
pub fn density(cv: &CompactedVera) -> f64 {
    if cv.source_count == 0 {
        0.0
    } else {
        cv.vera / cv.source_count as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interaction(from: u8, cap: Capability, outcome: SessionOutcome, ts: u64) -> Interaction {
        Interaction {
            from: HeartId([from; 32]),
            to: HeartId([0xff; 32]),
            capability: cap,
            soma_amount: 1,
            outcome,
            timestamp: ts,
            duration_ms: 0,
            participant_count: 2,
            bilateral: true,
        }
    }

    #[test]
    fn coherence_is_one_for_a_fresh_bilateral_interaction() {
        let i = interaction(1, Capability::CodeExecution, SessionOutcome::Success, 1_000);
        assert!((coherence(&i, 1_000) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn coherence_decays_but_never_goes_negative() {
        let i = interaction(1, Capability::CodeExecution, SessionOutcome::Success, 0);
        let ten_days = 10 * 86_400_000;
        let c = coherence(&i, ten_days);
        assert!(c > 0.0 && c < 1.0, "expected decayed coherence, got {c}");
        let year = 365 * 86_400_000u64;
        assert!(coherence(&i, year) > 0.0);
    }

    #[test]
    fn a_unilateral_interaction_has_no_coherence() {
        let mut i = interaction(1, Capability::CodeExecution, SessionOutcome::Success, 0);
        i.bilateral = false;
        assert_eq!(coherence(&i, 0), 0.0);
        assert_eq!(vera(&i, 0), 0.0);
    }

    #[test]
    fn failure_lowers_trust_but_not_warmth() {
        let now = 1_000;
        let cap = Capability::CodeExecution;
        let ok = interaction(1, cap.clone(), SessionOutcome::Success, now);
        let bad = interaction(
            2,
            cap.clone(),
            SessionOutcome::Failure {
                error_class: "x".into(),
            },
            now,
        );

        // Warmth is observation mass: a failure is as observed as a success.
        assert!((compute_warmth(&[ok.clone(), bad.clone()], &cap, now) - 2.0).abs() < 1e-9);
        // Trust is signed: one success and one failure cancel.
        assert!(compute_trust(&[ok, bad], &cap, now).abs() < 1e-9);
    }

    #[test]
    fn trust_is_scoped_to_one_capability() {
        let now = 0;
        let code = interaction(1, Capability::CodeExecution, SessionOutcome::Success, now);
        let file = interaction(2, Capability::FileAccess, SessionOutcome::Success, now);
        assert!((compute_trust(&[code, file], &Capability::FileAccess, now) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn one_repeated_observer_cannot_reach_high_coherence() {
        let now = 0;
        // Twenty successes, all from the same actor: perfect consensus, perfect
        // stability, diversity 0.05. The bottleneck is multiplicative, so this
        // is the anti-gaming property — it must not distill to a high C.
        let gamed: Vec<Interaction> = (0..20)
            .map(|_| interaction(7, Capability::CodeExecution, SessionOutcome::Success, now))
            .collect();
        let diverse: Vec<Interaction> = (0..20)
            .map(|n| {
                interaction(
                    n as u8,
                    Capability::CodeExecution,
                    SessionOutcome::Success,
                    now,
                )
            })
            .collect();

        let gamed_c = distill(&gamed, now).coherence;
        let diverse_c = distill(&diverse, now).coherence;

        assert!(gamed_c < 0.1, "one repeated observer reached C={gamed_c}");
        assert!(
            diverse_c > 0.9,
            "twenty independent observers only reached C={diverse_c}"
        );
        assert!(diverse_c > gamed_c);
    }

    #[test]
    fn distilling_nothing_is_zero_not_a_panic() {
        let empty = distill(&[], 42);
        assert_eq!(empty.source_count, 0);
        assert_eq!(empty.vera, 0.0);
        assert_eq!(empty.timestamp, 42);
        assert_eq!(distill_compacted(&[]).vera, 0.0);
        assert_eq!(density(&empty), 0.0);
    }

    #[test]
    fn compaction_raises_the_level_and_sums_the_sources() {
        let now = 0;
        let batch: Vec<Interaction> = (0..10)
            .map(|n| {
                interaction(
                    n as u8,
                    Capability::CodeExecution,
                    SessionOutcome::Success,
                    now,
                )
            })
            .collect();
        let level1: Vec<CompactedVera> = (0..3).map(|_| distill(&batch, now)).collect();
        let level2 = distill_compacted(&level1);

        assert_eq!(level1[0].level, 1);
        assert_eq!(level2.level, 2);
        assert_eq!(level2.source_count, 30);
    }

    #[test]
    fn network_enrichment_is_warmth_per_actor() {
        let now = 0;
        let interactions: Vec<Interaction> = (0..4)
            .map(|n| {
                interaction(
                    n as u8,
                    Capability::CodeExecution,
                    SessionOutcome::Success,
                    now,
                )
            })
            .collect();
        let net = compute_network(&interactions, 4, now);
        assert!((net.total_warmth - 4.0).abs() < 1e-9);
        assert!((net.enrichment_ratio - 1.0).abs() < 1e-9);

        // No actors is zero, not a division by zero.
        assert_eq!(compute_network(&interactions, 0, now).enrichment_ratio, 0.0);
    }

    #[test]
    fn a_future_timestamp_does_not_underflow() {
        // `now` behind the interaction's timestamp must saturate, not wrap into
        // an enormous elapsed time.
        let i = interaction(
            1,
            Capability::CodeExecution,
            SessionOutcome::Success,
            10_000,
        );
        assert!((coherence(&i, 0) - 1.0).abs() < f64::EPSILON);
    }
}
