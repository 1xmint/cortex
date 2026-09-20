use crate::delegation::Capability;
use crate::trust::{self, Interaction};
use crate::types::HeartId;

pub struct NetworkVera {
    pub total_warmth: f64,
    pub active_hearts: u64,
    pub enrichment_ratio: f64,
}

pub fn network_warmth(interactions: &[Interaction], now: u64) -> f64 {
    interactions.iter().map(|i| trust::vera(i, now)).sum()
}

pub fn domain_warmth(interactions: &[Interaction], capability: &Capability, now: u64) -> f64 {
    trust::compute_warmth(interactions, capability, now)
}

pub fn enrichment_ratio(interactions: &[Interaction], active_hearts: u64, now: u64) -> f64 {
    if active_hearts == 0 {
        return 0.0;
    }
    network_warmth(interactions, now) / active_hearts as f64
}

pub fn heart_warmth(interactions: &[Interaction], heart_id: &HeartId, now: u64) -> f64 {
    interactions
        .iter()
        .filter(|i| i.from == *heart_id || i.to == *heart_id)
        .map(|i| trust::vera(i, now))
        .sum()
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
