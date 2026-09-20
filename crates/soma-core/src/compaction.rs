use crate::envelope::SessionOutcome;
use crate::trust::{self, Interaction};

/// Compaction rooted in the Information Bottleneck principle (Tishby 1999).
///
/// The IB method: compress X while preserving information about Y.
/// minimize I(X;T) - β·I(T;Y)
///
/// For Vera: X = raw observations, Y = trust-relevant signal,
/// T = compacted representation.
///
/// What survives compression IS signal. What doesn't IS noise.
/// Not because we defined it — because Shannon proved it.
///
/// Same mechanism at every level. Same quality. Different scale.
/// One atom of truth or a billion — same soul.
/// A compacted observation at any level. The representation that
/// survived the Information Bottleneck — the signal that passed
/// through the narrowest point and proved it carries meaning.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CompactedVera {
    pub level: u32,
    /// The $VERA that survived compaction: mass × coherence²
    pub vera: f64,
    /// Coherence at this compaction level — how much of the input
    /// was trust-relevant signal vs noise. Derived from mutual
    /// information between observations and outcomes.
    pub coherence: f64,
    /// How many observations were distilled into this value
    pub source_count: usize,
    /// The signal profile — what the compaction revealed
    pub signal: SignalProfile,
    pub timestamp: u64,
}

/// What the Information Bottleneck extracted — the signal that
/// survived compression. This IS the intelligence. Not raw data,
/// not statistics, but the irreducible truth about what was observed.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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

/// Extract the trust-relevant signal from a set of interactions.
/// This is the "Y" in the Information Bottleneck — what we're
/// trying to preserve through compression.
fn extract_signal(interactions: &[Interaction]) -> SignalProfile {
    if interactions.is_empty() {
        return SignalProfile {
            consensus: 0.0,
            diversity: 0.0,
            stability: 0.0,
        };
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
    // unique_from_hearts / interaction_count.
    // 100 interactions from 100 agents = 1.0 (fully independent).
    // 100 interactions from 1 agent = 0.01 (one observer, repeated).
    let mut unique_from = std::collections::HashSet::new();
    for i in interactions {
        unique_from.insert(i.from);
    }
    let diversity = (unique_from.len() as f64 / n).min(1.0);

    // Stability: autocorrelation of outcome signs
    // How much does sign[t] predict sign[t+1]?
    let stability = if signs.len() >= 2 {
        let mean = consensus;
        let mut num = 0.0;
        let mut den = 0.0;
        for i in 0..signs.len() {
            den += (signs[i] - mean) * (signs[i] - mean);
            if i + 1 < signs.len() {
                num += (signs[i] - mean) * (signs[i + 1] - mean);
            }
        }
        if den.abs() < f64::EPSILON {
            1.0 // constant signal = perfectly stable
        } else {
            (num / den).clamp(-1.0, 1.0)
        }
    } else {
        1.0 // single observation is stable with itself
    };

    SignalProfile {
        consensus,
        diversity,
        stability,
    }
}

/// The Information Bottleneck coherence: how much of the raw
/// observation data is trust-relevant signal?
///
/// C = f(consensus, diversity, stability)
///
/// High consensus from diverse independent sources = high C (diamond)
/// High consensus from few sources = lower C (could be gamed)
/// Low consensus from any sources = low C (noise)
/// The same quality at every level. The same soul.
fn bottleneck_coherence(signal: &SignalProfile) -> f64 {
    // Consensus strength: absolute value — consistent failure is
    // as coherent as consistent success. Coherence is observation
    // completeness, not judgment.
    let consensus_strength = signal.consensus.abs();

    // Diversity factor: diverse independent observers make the
    // consensus more trustworthy. This is the "observation OF
    // the observation" — Vera seeing all miners, not just one.
    let diversity_factor = signal.diversity;

    // Stability factor: signal that holds over time is more
    // coherent than signal that fluctuates randomly.
    let stability_factor = (signal.stability + 1.0) / 2.0; // map [-1,1] to [0,1]

    // The bottleneck: all three must be present for high coherence.
    // Missing any one kills the signal quadratically (through C²).
    // This is multiplicative, not additive — you can't compensate
    // for low diversity with high consensus.
    (consensus_strength * diversity_factor * stability_factor).clamp(0.0, 1.0)
}

/// Compact raw interactions through the Information Bottleneck.
/// Same function for level 0→1 as for level N→N+1.
/// Same soul. Different scale.
pub fn distill(interactions: &[Interaction], now: u64) -> CompactedVera {
    if interactions.is_empty() {
        return CompactedVera {
            level: 1,
            vera: 0.0,
            coherence: 0.0,
            source_count: 0,
            signal: SignalProfile {
                consensus: 0.0,
                diversity: 0.0,
                stability: 0.0,
            },
            timestamp: now,
        };
    }

    // Input mass: sum of $VERA from raw interactions
    let soma: f64 = interactions.iter().map(|i| trust::vera(i, now)).sum();

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

/// Compact compacted values — same mechanism, higher level.
/// The output of distillation becomes input for the next distillation.
/// Qualitatively identical. Quantitatively different.
pub fn distill_compacted(inputs: &[CompactedVera]) -> CompactedVera {
    if inputs.is_empty() {
        return CompactedVera {
            level: 1,
            vera: 0.0,
            coherence: 0.0,
            source_count: 0,
            signal: SignalProfile {
                consensus: 0.0,
                diversity: 0.0,
                stability: 0.0,
            },
            timestamp: 0,
        };
    }

    let max_level = inputs.iter().map(|cv| cv.level).max().unwrap_or(0);
    let latest = inputs.iter().map(|cv| cv.timestamp).max().unwrap_or(0);
    let total_sources: usize = inputs.iter().map(|cv| cv.source_count).sum();

    // Input mass: sum of $VERA from lower level
    let soma: f64 = inputs.iter().map(|cv| cv.vera).sum();

    // Extract signal from the compacted values themselves.
    // At higher levels, consensus = agreement among compacted signals,
    // diversity = how many distinct compaction sources,
    // stability = consistency across compacted values.
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
        return SignalProfile {
            consensus: 0.0,
            diversity: 0.0,
            stability: 0.0,
        };
    }

    let n = inputs.len() as f64;

    // Consensus: agreement of compacted consensus signals
    let consensus = inputs.iter().map(|cv| cv.signal.consensus).sum::<f64>() / n;

    // Diversity: how many distinct source populations contributed?
    // More distinct compaction batches = more diverse meta-observation.
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
    let mean = consensus;
    let stability = if consensuses.len() >= 2 {
        let mut num = 0.0;
        let mut den = 0.0;
        for i in 0..consensuses.len() {
            den += (consensuses[i] - mean) * (consensuses[i] - mean);
            if i + 1 < consensuses.len() {
                num += (consensuses[i] - mean) * (consensuses[i + 1] - mean);
            }
        }
        if den.abs() < f64::EPSILON {
            1.0
        } else {
            (num / den).clamp(-1.0, 1.0)
        }
    } else {
        1.0
    };

    SignalProfile {
        consensus,
        diversity,
        stability,
    }
}

/// Recursive distillation: interactions → windowed compaction → recursive
/// compaction through N levels. Same soul at every level.
pub fn recursive_distill(
    interactions: &[Interaction],
    depth: u32,
    window_size: usize,
    now: u64,
) -> CompactedVera {
    if interactions.is_empty() {
        return CompactedVera {
            level: depth,
            vera: 0.0,
            coherence: 0.0,
            source_count: 0,
            signal: SignalProfile {
                consensus: 0.0,
                diversity: 0.0,
                stability: 0.0,
            },
            timestamp: now,
        };
    }

    let window = window_size.max(1);

    // Level 0→1: distill raw interactions in windows
    let mut current_level: Vec<CompactedVera> = interactions
        .chunks(window)
        .map(|chunk| distill(chunk, now))
        .collect();

    // Level N→N+1: distill compacted values
    for _level in 1..depth {
        if current_level.len() <= 1 {
            break;
        }
        current_level = current_level
            .chunks(window)
            .map(distill_compacted)
            .collect();
    }

    if current_level.len() == 1 {
        current_level.into_iter().next().unwrap()
    } else {
        distill_compacted(&current_level)
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

// === Backward-compatible aliases ===

pub fn compact_interactions(
    interactions: &[Interaction],
    _total_available: usize,
    now: u64,
) -> CompactedVera {
    distill(interactions, now)
}

pub fn compact(inputs: &[CompactedVera], _total_available: usize) -> CompactedVera {
    distill_compacted(inputs)
}

pub fn recursive_compact(
    interactions: &[Interaction],
    depth: u32,
    window_size: usize,
    now: u64,
) -> CompactedVera {
    recursive_distill(interactions, depth, window_size, now)
}
