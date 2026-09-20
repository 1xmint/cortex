use crate::delegation::Capability;
use crate::envelope::SessionOutcome;
use crate::types::HeartId;

/// One equation: $VERA = $SOMA × C²
///
/// Coherence is observation completeness — how fully was this interaction seen?
/// Not quality. Not judgment. Observation density.
///
/// Outcome is NOT coherence. A failure is just as observed as a success.
/// Outcome affects the sign on trust, not whether Vera sees you.

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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

pub const TEMPORAL_DECAY: f64 = 0.01;
pub const HIGH_STAKES_TEMPORAL_DECAY: f64 = 0.02;

// === Backward-compatible aliases ===

pub type BilateralReceipt = Interaction;
pub const DEFAULT_DECAY_LAMBDA: f64 = TEMPORAL_DECAY;
pub const HIGH_STAKES_DECAY_LAMBDA: f64 = HIGH_STAKES_TEMPORAL_DECAY;

impl Interaction {
    pub fn from_receipt(
        from: HeartId,
        to: HeartId,
        capability: Capability,
        soma_amount: u64,
        outcome: SessionOutcome,
        timestamp: u64,
    ) -> Self {
        Self {
            from,
            to,
            capability,
            soma_amount,
            outcome,
            timestamp,
            duration_ms: 0,
            participant_count: 2,
            bilateral: true,
        }
    }
}
