use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::provider::{ProviderId, Tier};
use crate::routing::{Intent, RationaleCode, RiskLevel, RoutingDecision, ScoredRoute};

// --- Profiles ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Auto,
    Balanced,
    CostSaver,
    QualityFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoMode {
    Normal,
    ProtectBudget,
    ProtectQuality,
    Recovering,
}

impl Profile {
    pub fn from_alias(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "balanced" => Some(Self::Balanced),
            "cost-saver" | "costsaver" | "cheap" | "fast" => Some(Self::CostSaver),
            "quality-first" | "qualityfirst" | "careful" | "thorough" | "smart" => {
                Some(Self::QualityFirst)
            }
            _ => None,
        }
    }

    pub fn heal_attempts(&self) -> u32 {
        match self {
            Self::CostSaver => 1,
            Self::Auto | Self::Balanced => 2,
            Self::QualityFirst => 3,
        }
    }
}

// --- Pressure ---

pub const WINDOW_SECS: i64 = 5 * 60 * 60;

pub const PRESSURE_WARM: f64 = 0.65;
pub const PRESSURE_HOT: f64 = 0.82;
pub const PRESSURE_THROTTLED: f64 = 0.95;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "value")]
pub enum PressureState {
    Healthy(f64),
    Warm(f64),
    Hot(f64),
    Throttled(f64),
}

impl PressureState {
    pub fn from_ratio(p: f64) -> Self {
        if p >= PRESSURE_THROTTLED {
            Self::Throttled(p)
        } else if p >= PRESSURE_HOT {
            Self::Hot(p)
        } else if p >= PRESSURE_WARM {
            Self::Warm(p)
        } else {
            Self::Healthy(p)
        }
    }

    pub fn ratio(&self) -> f64 {
        match self {
            Self::Healthy(p) | Self::Warm(p) | Self::Hot(p) | Self::Throttled(p) => *p,
        }
    }

    pub fn is_throttled(&self) -> bool {
        matches!(self, Self::Throttled(_))
    }
}

pub fn pressure_penalty(pressure: f64) -> f64 {
    let p = pressure.clamp(0.0, 1.25);

    if p < PRESSURE_WARM {
        0.0
    } else if p < PRESSURE_HOT {
        let x = (p - PRESSURE_WARM) / (PRESSURE_HOT - PRESSURE_WARM);
        5.0 + 15.0 * x
    } else if p < PRESSURE_THROTTLED {
        let x = (p - PRESSURE_HOT) / (PRESSURE_THROTTLED - PRESSURE_HOT);
        20.0 + 40.0 * x.powf(1.5)
    } else {
        85.0 + 15.0 * ((p - PRESSURE_THROTTLED) / 0.30).clamp(0.0, 1.0)
    }
}

// --- Token budgets (5-hour window) ---

pub fn token_budget(provider: ProviderId, tier: Tier) -> u64 {
    match (provider, tier) {
        (ProviderId::Claude, Tier::Search) => 1_000_000,
        (ProviderId::Claude, Tier::Execute) => 500_000,
        (ProviderId::Claude, Tier::Think) => 500_000,

        (ProviderId::Openai, Tier::Search) => 1_000_000,
        (ProviderId::Openai, Tier::Execute) => 500_000,
        (ProviderId::Openai, Tier::Think) => 500_000,

        (ProviderId::Gemini, Tier::Search) => 600_000,
        (ProviderId::Gemini, Tier::Execute) => 300_000,
        (ProviderId::Gemini, Tier::Think) => 300_000,

        // Mirrors OpenAI until measured usage says otherwise — Zen is the
        // cheap tier, so the budget should never be the binding constraint.
        (ProviderId::Zen, Tier::Search) => 1_000_000,
        (ProviderId::Zen, Tier::Execute) => 500_000,
        (ProviderId::Zen, Tier::Think) => 500_000,
    }
}

pub fn fallback_tokens(tier: Tier) -> u64 {
    match tier {
        Tier::Search => 2_500,
        Tier::Execute => 5_500,
        Tier::Think => 11_000,
    }
}

pub fn estimate_tokens(actual_in: Option<u64>, actual_out: Option<u64>, tier: Tier) -> u64 {
    match (actual_in, actual_out) {
        (Some(i), Some(o)) => i + o,
        (Some(i), None) => i + fallback_tokens(tier) / 2,
        (None, Some(o)) => fallback_tokens(tier) / 2 + o,
        (None, None) => fallback_tokens(tier),
    }
}

// --- Scoring weights ---

#[derive(Debug, Clone, Copy)]
pub struct Weights {
    pub intent: f64,
    pub risk: f64,
    pub budget: f64,
    pub provider_fit: f64,
}

pub const BALANCED_WEIGHTS: Weights = Weights {
    intent: 0.20,
    risk: 0.25,
    budget: 0.25,
    provider_fit: 0.30,
};

pub const COST_SAVER_WEIGHTS: Weights = Weights {
    intent: 0.10,
    risk: 0.25,
    budget: 0.45,
    provider_fit: 0.20,
};

pub const QUALITY_FIRST_WEIGHTS: Weights = Weights {
    intent: 0.15,
    risk: 0.35,
    budget: 0.10,
    provider_fit: 0.40,
};

const RECOVERING_WEIGHTS: Weights = Weights {
    intent: 0.15,
    risk: 0.30,
    budget: 0.20,
    provider_fit: 0.35,
};

pub fn weights_for(profile: Profile, auto_mode: AutoMode) -> Weights {
    match profile {
        Profile::Balanced => BALANCED_WEIGHTS,
        Profile::CostSaver => COST_SAVER_WEIGHTS,
        Profile::QualityFirst => QUALITY_FIRST_WEIGHTS,
        Profile::Auto => match auto_mode {
            AutoMode::Normal => BALANCED_WEIGHTS,
            AutoMode::ProtectBudget => COST_SAVER_WEIGHTS,
            AutoMode::ProtectQuality => QUALITY_FIRST_WEIGHTS,
            AutoMode::Recovering => RECOVERING_WEIGHTS,
        },
    }
}

// --- Evidence types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentEvidence {
    pub intent: Intent,
    pub confidence: f64,
    pub default_tier: Tier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskBasis {
    Static,
    FilePath,
    Historical,
    Combined,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskEvidence {
    pub level: RiskLevel,
    pub basis: RiskBasis,
    pub static_level: RiskLevel,
    pub file_risk: Option<RiskLevel>,
    pub history_success_rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetEvidence {
    pub pressures: HashMap<(ProviderId, Tier), PressureState>,
}

impl BudgetEvidence {
    pub fn pressure_for(&self, provider: ProviderId, tier: Tier) -> f64 {
        self.pressures
            .get(&(provider, tier))
            .map(|s| s.ratio())
            .unwrap_or(0.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateScore {
    pub provider: ProviderId,
    pub tier: Tier,
    pub worker_id: Option<String>,
    pub authenticated: bool,
    pub pressure: f64,
    pub success_rate: Option<f64>,
    pub sample_count: u64,
    pub estimated_duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderFitEvidence {
    pub candidates: Vec<CandidateScore>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionEvidence {
    pub intent: IntentEvidence,
    pub risk: RiskEvidence,
    pub budget: BudgetEvidence,
    pub provider_fit: ProviderFitEvidence,
}

// --- Scoring subscore functions ---

fn intent_score(candidate: &CandidateScore, evidence: &IntentEvidence) -> Option<f64> {
    if !candidate.authenticated {
        return None; // veto
    }

    // Veto providers with known poor reliability (< 60% success over 30+ samples)
    if let Some(rate) = candidate.success_rate {
        if rate < 0.60 && candidate.sample_count >= 30 {
            return None; // vetoed — unreliable provider
        }
    }

    let default_tier = evidence.default_tier;
    let candidate_rank = candidate.tier.rank();
    let default_rank = default_tier.rank();

    if candidate_rank == default_rank {
        Some(20.0)
    } else if candidate_rank > default_rank {
        Some(8.0)
    } else {
        Some(-30.0)
    }
}

fn risk_score(candidate: &CandidateScore, evidence: &RiskEvidence) -> Option<f64> {
    let min_tier = evidence.level.minimum_tier();
    let candidate_rank = candidate.tier.rank();
    let min_rank = min_tier.rank();

    if evidence.level == RiskLevel::Critical && candidate.tier != Tier::Think {
        return None; // veto
    }

    if candidate_rank >= min_rank {
        Some(20.0)
    } else if candidate_rank + 1 >= min_rank {
        Some(8.0)
    } else {
        Some(-40.0)
    }
}

fn budget_score(candidate: &CandidateScore, peer_avg_pressure: f64) -> f64 {
    let penalty = pressure_penalty(candidate.pressure);
    let bonus = underused_bonus(candidate.pressure, peer_avg_pressure);
    30.0 - penalty + bonus
}

fn underused_bonus(provider_pressure: f64, peer_avg_pressure: f64) -> f64 {
    if provider_pressure < 0.20 && peer_avg_pressure > 0.70 {
        18.0
    } else if provider_pressure < 0.30 && peer_avg_pressure > 0.55 {
        12.0
    } else {
        0.0
    }
}

fn capability_bonus(candidate_tier: Tier, default_tier: Tier) -> f64 {
    if candidate_tier.rank() == default_tier.rank() {
        22.0
    } else if candidate_tier.rank() > default_tier.rank() {
        10.0
    } else {
        -35.0
    }
}

fn reliability_bonus(success_rate: Option<f64>, sample_count: u64) -> f64 {
    if sample_count < 20 {
        return 0.0;
    }
    let rate = success_rate.unwrap_or(0.80).clamp(0.0, 1.0);
    ((rate - 0.80) / 0.20 * 12.0).clamp(-15.0, 12.0)
}

fn latency_penalty(provider: ProviderId, estimated_duration_ms: u64) -> f64 {
    match provider {
        ProviderId::Openai if estimated_duration_ms < 90_000 => 18.0,
        ProviderId::Openai if estimated_duration_ms < 180_000 => 10.0,
        ProviderId::Gemini if estimated_duration_ms < 90_000 => 8.0,
        _ => 0.0,
    }
}

fn risk_alignment_bonus(risk: RiskLevel, tier: Tier) -> f64 {
    match (risk, tier) {
        (RiskLevel::Critical, Tier::Think) => 20.0,
        (RiskLevel::High, Tier::Think) => 12.0,
        (RiskLevel::High, Tier::Execute) => 8.0,
        (RiskLevel::Medium, Tier::Execute) => 8.0,
        (RiskLevel::Low, Tier::Search) => 8.0,
        _ => 0.0,
    }
}

fn profile_bias(profile: Profile, provider: ProviderId, tier: Tier, risk: RiskLevel) -> f64 {
    match profile {
        Profile::CostSaver => match tier {
            Tier::Search => 8.0,
            Tier::Execute => 4.0,
            // Don't penalize Think tier for high/critical risk — those tasks need it
            Tier::Think if risk >= RiskLevel::High => 0.0,
            Tier::Think => -8.0,
        },
        Profile::QualityFirst => match tier {
            Tier::Think => 12.0,
            Tier::Execute => 4.0,
            Tier::Search => -6.0,
        },
        Profile::Balanced | Profile::Auto => match provider {
            ProviderId::Claude | ProviderId::Openai => 2.0,
            // Neutral until outcome data earns Zen a bias either way.
            ProviderId::Gemini | ProviderId::Zen => 0.0,
        },
    }
}

fn provider_fit_score(
    candidate: &CandidateScore,
    evidence: &DecisionEvidence,
    profile: Profile,
) -> f64 {
    let cap = capability_bonus(candidate.tier, evidence.intent.default_tier);
    let rel = reliability_bonus(candidate.success_rate, candidate.sample_count);
    let lat = latency_penalty(candidate.provider, candidate.estimated_duration_ms);
    let risk_align = risk_alignment_bonus(evidence.risk.level, candidate.tier);
    let bias = profile_bias(
        profile,
        candidate.provider,
        candidate.tier,
        evidence.risk.level,
    );

    cap + rel + lat + risk_align + bias
}

// --- Policy function ---

pub struct DefaultPolicy;

impl DefaultPolicy {
    pub fn decide(
        &self,
        evidence: &DecisionEvidence,
        profile: Profile,
        auto_mode: AutoMode,
    ) -> RoutingDecision {
        let w = weights_for(profile, auto_mode);

        let peer_avg_pressure = {
            let pressures: Vec<f64> = evidence
                .provider_fit
                .candidates
                .iter()
                .map(|c| c.pressure)
                .collect();
            if pressures.is_empty() {
                0.0
            } else {
                pressures.iter().sum::<f64>() / pressures.len() as f64
            }
        };

        let mut scored: Vec<(f64, &CandidateScore, Vec<RationaleCode>)> = Vec::new();

        for candidate in &evidence.provider_fit.candidates {
            let i_score = match intent_score(candidate, &evidence.intent) {
                Some(s) => s,
                None => continue, // vetoed
            };
            let r_score = match risk_score(candidate, &evidence.risk) {
                Some(s) => s,
                None => continue, // vetoed
            };
            let b_score = budget_score(candidate, peer_avg_pressure);
            let pf_score = provider_fit_score(candidate, evidence, profile);

            let total = 50.0
                + w.intent * i_score
                + w.risk * r_score
                + w.budget * b_score
                + w.provider_fit * pf_score;

            let mut rationale = Vec::new();
            if candidate.tier == evidence.intent.default_tier {
                rationale.push(RationaleCode::BestAvailableForTier);
            }
            if candidate.pressure < peer_avg_pressure - 0.15 {
                rationale.push(RationaleCode::LowerPressure);
            }
            if candidate.tier.rank() > evidence.intent.default_tier.rank()
                && evidence.risk.level >= RiskLevel::High
            {
                rationale.push(RationaleCode::RiskRequiresHigherTier);
            }
            if matches!(
                evidence
                    .budget
                    .pressures
                    .get(&(candidate.provider, candidate.tier)),
                Some(PressureState::Throttled(_))
            ) {
                rationale.push(RationaleCode::CostOptimized);
            }

            scored.push((total, candidate, rationale));
        }

        // Deterministic tie-breaking:
        // 1. Higher score
        // 2. Higher recent success rate
        // 3. Lower pressure
        // 4. Stable provider order: Claude < Openai < Gemini
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let a_rate = a.1.success_rate.unwrap_or(0.5);
                    let b_rate = b.1.success_rate.unwrap_or(0.5);
                    b_rate
                        .partial_cmp(&a_rate)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| {
                    a.1.pressure
                        .partial_cmp(&b.1.pressure)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| provider_order(a.1.provider).cmp(&provider_order(b.1.provider)))
        });

        let alternatives: Vec<ScoredRoute> = scored
            .iter()
            .skip(1)
            .take(3)
            .map(|(score, c, _)| ScoredRoute {
                provider: c.provider,
                tier: c.tier,
                score: *score,
            })
            .collect();

        if let Some((score, winner, rationale)) = scored.into_iter().next() {
            let model_id = default_model(winner.provider, winner.tier);
            RoutingDecision {
                provider: winner.provider,
                tier: winner.tier,
                model_id,
                rationale,
                score,
                alternatives_considered: alternatives,
            }
        } else {
            // All candidates vetoed — fallback to Claude Think
            RoutingDecision {
                provider: ProviderId::Claude,
                tier: Tier::Think,
                model_id: "claude-opus-4-6".to_string(),
                rationale: vec![RationaleCode::ProviderUnavailable],
                score: 0.0,
                alternatives_considered: vec![],
            }
        }
    }
}

fn provider_order(p: ProviderId) -> u8 {
    match p {
        ProviderId::Claude => 0,
        ProviderId::Openai => 1,
        ProviderId::Gemini => 2,
        ProviderId::Zen => 3,
    }
}

pub fn default_model_for_tier(provider: ProviderId, tier: Tier) -> String {
    default_model(provider, tier)
}

fn default_model(provider: ProviderId, tier: Tier) -> String {
    match (provider, tier) {
        (ProviderId::Claude, Tier::Search) => "claude-haiku-4-5-20251001",
        (ProviderId::Claude, Tier::Execute) => "claude-sonnet-4-6",
        (ProviderId::Claude, Tier::Think) => "claude-opus-4-6",

        (ProviderId::Openai, Tier::Search) => "gpt-4.1-mini",
        (ProviderId::Openai, Tier::Execute) => "gpt-5.4",
        (ProviderId::Openai, Tier::Think) => "gpt-5.5",

        (ProviderId::Gemini, Tier::Search) => "gemini-2.5-flash",
        (ProviderId::Gemini, Tier::Execute) => "gemini-2.5-pro",
        (ProviderId::Gemini, Tier::Think) => "gemini-2.5-pro",

        // Must agree with the engine registry (crates/engine/src/models.rs).
        (ProviderId::Zen, Tier::Search) => "glm-5",
        (ProviderId::Zen, Tier::Execute) => "glm-5.2",
        (ProviderId::Zen, Tier::Think) => "kimi-k3",
    }
    .to_string()
}

// --- Auto-mode state machine ---

impl AutoMode {
    pub fn transition(
        &self,
        max_pressure: f64,
        risk: RiskLevel,
        recent_failures: u32,
        minutes_in_state: u32,
    ) -> Self {
        match self {
            Self::Normal => {
                if max_pressure >= PRESSURE_HOT {
                    Self::ProtectBudget
                } else if risk >= RiskLevel::High {
                    Self::ProtectQuality
                } else if recent_failures >= 2 {
                    Self::Recovering
                } else {
                    Self::Normal
                }
            }
            Self::ProtectBudget => {
                if max_pressure < PRESSURE_WARM && minutes_in_state >= 30 {
                    Self::Normal
                } else {
                    Self::ProtectBudget
                }
            }
            Self::ProtectQuality => {
                if risk < RiskLevel::High && minutes_in_state >= 5 {
                    Self::Normal
                } else {
                    Self::ProtectQuality
                }
            }
            Self::Recovering => {
                if recent_failures == 0 && minutes_in_state >= 30 {
                    Self::Normal
                } else {
                    Self::Recovering
                }
            }
        }
    }
}

// --- Risk classifier (file-path based) ---

pub fn classify_file_risk(path: &str) -> RiskLevel {
    let lower = path.to_lowercase();

    if lower.contains("secret")
        || lower.contains("credential")
        || lower.contains(".env")
        || lower.contains("private_key")
        || lower.contains("deploy")
        || lower.contains("auth/")
        || lower.contains("auth.rs")
        || lower.contains("clerk")
        || lower.contains("jwt")
    {
        return RiskLevel::Critical;
    }

    if lower.contains("billing")
        || lower.contains("payment")
        || lower.contains("migration")
        || lower.contains("schema")
        || lower.contains("security")
        || lower.contains("permission")
    {
        return RiskLevel::High;
    }

    if lower.contains("test")
        || lower.contains("spec")
        || lower.contains("util")
        || lower.contains("helper")
        || lower.contains("config")
    {
        return RiskLevel::Medium;
    }

    if lower.contains("doc")
        || lower.contains("readme")
        || lower.contains("changelog")
        || lower.contains("comment")
    {
        return RiskLevel::Low;
    }

    RiskLevel::Medium
}

pub fn classify_risk(
    static_level: RiskLevel,
    file_paths: &[String],
    history_success_rate: Option<f64>,
) -> RiskEvidence {
    let file_risk = file_paths.iter().map(|p| classify_file_risk(p)).max();

    let level = [
        Some(static_level),
        file_risk,
        history_success_rate.and_then(|r| {
            if r < 0.5 {
                Some(RiskLevel::High)
            } else if r < 0.75 {
                Some(RiskLevel::Medium)
            } else {
                None
            }
        }),
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or(static_level);

    let basis = match (file_risk.is_some(), history_success_rate.is_some()) {
        (true, true) => RiskBasis::Combined,
        (true, false) => RiskBasis::FilePath,
        (false, true) => RiskBasis::Historical,
        (false, false) => RiskBasis::Static,
    };

    RiskEvidence {
        level,
        basis,
        static_level,
        file_risk,
        history_success_rate,
    }
}

// --- Intent parsing (deterministic, no LLM) ---

pub fn parse_intent(input: &str) -> IntentEvidence {
    let lower = input.to_lowercase();

    let (intent, confidence) =
        if starts_with_any(&lower, &["fix", "repair", "patch", "resolve", "debug"]) {
            (Intent::Fix, 0.95)
        } else if starts_with_any(&lower, &["add", "create", "implement", "build", "write"]) {
            (Intent::Add, 0.90)
        } else if starts_with_any(
            &lower,
            &["explore", "find", "search", "grep", "where", "what is"],
        ) {
            (Intent::Explore, 0.90)
        } else if starts_with_any(&lower, &["review", "audit", "check", "inspect"]) {
            (Intent::Review, 0.85)
        } else if starts_with_any(
            &lower,
            &[
                "should",
                "how should",
                "architecture",
                "design",
                "think about",
            ],
        ) {
            (Intent::Think, 0.85)
        } else if starts_with_any(&lower, &["test", "verify", "validate"]) {
            (Intent::Test, 0.90)
        } else if starts_with_any(&lower, &["refactor", "clean", "simplify", "restructure"]) {
            (Intent::Refactor, 0.85)
        } else if starts_with_any(&lower, &["ship", "deploy", "release", "publish", "pr"]) {
            (Intent::Ship, 0.90)
        } else if contains_any(&lower, &["fix", "bug", "broken", "error"]) {
            (Intent::Fix, 0.70)
        } else if contains_any(&lower, &["add", "feature", "implement"]) {
            (Intent::Add, 0.65)
        } else if contains_any(&lower, &["find", "search", "where"]) {
            (Intent::Explore, 0.65)
        } else if contains_any(&lower, &["test", "spec"]) {
            (Intent::Test, 0.65)
        } else {
            (Intent::Add, 0.30)
        };

    IntentEvidence {
        intent,
        confidence,
        default_tier: intent.default_tier(),
    }
}

fn starts_with_any(s: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| s.starts_with(p))
}

fn contains_any(s: &str, words: &[&str]) -> bool {
    words.iter().any(|w| s.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_penalty_healthy_is_zero() {
        assert_eq!(pressure_penalty(0.0), 0.0);
        assert_eq!(pressure_penalty(0.5), 0.0);
        assert_eq!(pressure_penalty(0.64), 0.0);
    }

    #[test]
    fn pressure_penalty_increases_monotonically() {
        let mut prev = 0.0;
        for i in 0..=125 {
            let p = i as f64 / 100.0;
            let penalty = pressure_penalty(p);
            assert!(
                penalty >= prev,
                "penalty decreased at p={p}: {penalty} < {prev}"
            );
            prev = penalty;
        }
    }

    #[test]
    fn pressure_penalty_throttled_is_high() {
        assert!(pressure_penalty(0.95) >= 85.0);
        assert!(pressure_penalty(1.0) >= 87.0);
        assert!(pressure_penalty(1.25) >= 100.0);
    }

    #[test]
    fn critical_risk_vetoes_non_think() {
        let candidate = CandidateScore {
            provider: ProviderId::Claude,
            tier: Tier::Execute,
            worker_id: None,
            authenticated: true,
            pressure: 0.1,
            success_rate: Some(0.95),
            sample_count: 50,
            estimated_duration_ms: 60_000,
        };
        let risk_ev = RiskEvidence {
            level: RiskLevel::Critical,
            basis: RiskBasis::Static,
            static_level: RiskLevel::Critical,
            file_risk: None,
            history_success_rate: None,
        };
        assert!(risk_score(&candidate, &risk_ev).is_none());
    }

    #[test]
    fn critical_risk_allows_think() {
        let candidate = CandidateScore {
            provider: ProviderId::Claude,
            tier: Tier::Think,
            worker_id: None,
            authenticated: true,
            pressure: 0.1,
            success_rate: Some(0.95),
            sample_count: 50,
            estimated_duration_ms: 60_000,
        };
        let risk_ev = RiskEvidence {
            level: RiskLevel::Critical,
            basis: RiskBasis::Static,
            static_level: RiskLevel::Critical,
            file_risk: None,
            history_success_rate: None,
        };
        assert!(risk_score(&candidate, &risk_ev).is_some());
    }

    #[test]
    fn unauthenticated_is_vetoed() {
        let candidate = CandidateScore {
            provider: ProviderId::Claude,
            tier: Tier::Execute,
            worker_id: None,
            authenticated: false,
            pressure: 0.0,
            success_rate: None,
            sample_count: 0,
            estimated_duration_ms: 0,
        };
        let intent_ev = IntentEvidence {
            intent: Intent::Fix,
            confidence: 0.9,
            default_tier: Tier::Execute,
        };
        assert!(intent_score(&candidate, &intent_ev).is_none());
    }

    #[test]
    fn policy_picks_best_candidate() {
        let evidence = DecisionEvidence {
            intent: IntentEvidence {
                intent: Intent::Fix,
                confidence: 0.9,
                default_tier: Tier::Execute,
            },
            risk: RiskEvidence {
                level: RiskLevel::Medium,
                basis: RiskBasis::Static,
                static_level: RiskLevel::Medium,
                file_risk: None,
                history_success_rate: None,
            },
            budget: BudgetEvidence {
                pressures: HashMap::new(),
            },
            provider_fit: ProviderFitEvidence {
                candidates: vec![
                    CandidateScore {
                        provider: ProviderId::Claude,
                        tier: Tier::Execute,
                        worker_id: Some("w1".into()),
                        authenticated: true,
                        pressure: 0.3,
                        success_rate: Some(0.92),
                        sample_count: 100,
                        estimated_duration_ms: 60_000,
                    },
                    CandidateScore {
                        provider: ProviderId::Openai,
                        tier: Tier::Execute,
                        worker_id: Some("w1".into()),
                        authenticated: true,
                        pressure: 0.7,
                        success_rate: Some(0.88),
                        sample_count: 80,
                        estimated_duration_ms: 90_000,
                    },
                ],
            },
        };

        let policy = DefaultPolicy;
        let decision = policy.decide(&evidence, Profile::Balanced, AutoMode::Normal);

        // Claude should win: lower pressure, higher success rate
        assert_eq!(decision.provider, ProviderId::Claude);
        assert_eq!(decision.tier, Tier::Execute);
        assert!(decision.score > 50.0);
    }

    #[test]
    fn cost_saver_prefers_lower_pressure() {
        let evidence = DecisionEvidence {
            intent: IntentEvidence {
                intent: Intent::Fix,
                confidence: 0.9,
                default_tier: Tier::Execute,
            },
            risk: RiskEvidence {
                level: RiskLevel::Low,
                basis: RiskBasis::Static,
                static_level: RiskLevel::Low,
                file_risk: None,
                history_success_rate: None,
            },
            budget: BudgetEvidence {
                pressures: HashMap::from([
                    (
                        (ProviderId::Claude, Tier::Execute),
                        PressureState::Hot(0.85),
                    ),
                    (
                        (ProviderId::Openai, Tier::Execute),
                        PressureState::Healthy(0.2),
                    ),
                ]),
            },
            provider_fit: ProviderFitEvidence {
                candidates: vec![
                    CandidateScore {
                        provider: ProviderId::Claude,
                        tier: Tier::Execute,
                        worker_id: Some("w1".into()),
                        authenticated: true,
                        pressure: 0.85,
                        success_rate: Some(0.90),
                        sample_count: 50,
                        estimated_duration_ms: 60_000,
                    },
                    CandidateScore {
                        provider: ProviderId::Openai,
                        tier: Tier::Execute,
                        worker_id: Some("w1".into()),
                        authenticated: true,
                        pressure: 0.2,
                        success_rate: Some(0.88),
                        sample_count: 50,
                        estimated_duration_ms: 90_000,
                    },
                ],
            },
        };

        let policy = DefaultPolicy;
        let decision = policy.decide(&evidence, Profile::CostSaver, AutoMode::Normal);

        // With cost-saver weights (budget=0.45), OpenAI's lower pressure should win
        assert_eq!(decision.provider, ProviderId::Openai);
    }

    #[test]
    fn file_risk_classification() {
        assert_eq!(
            classify_file_risk("src/auth/middleware.rs"),
            RiskLevel::Critical
        );
        assert_eq!(classify_file_risk("src/billing/stripe.rs"), RiskLevel::High);
        assert_eq!(classify_file_risk("src/utils/format.rs"), RiskLevel::Medium);
        assert_eq!(classify_file_risk("docs/README.md"), RiskLevel::Low);
    }

    #[test]
    fn intent_parsing() {
        assert_eq!(parse_intent("fix the login bug").intent, Intent::Fix);
        assert_eq!(parse_intent("add dark mode").intent, Intent::Add);
        assert_eq!(
            parse_intent("find where auth is defined").intent,
            Intent::Explore
        );
        assert_eq!(parse_intent("should we use Redis?").intent, Intent::Think);
        assert_eq!(parse_intent("test the payment flow").intent, Intent::Test);
        assert_eq!(parse_intent("ship it").intent, Intent::Ship);
    }

    #[test]
    fn auto_mode_transitions() {
        let normal = AutoMode::Normal;
        assert_eq!(
            normal.transition(0.90, RiskLevel::Low, 0, 0),
            AutoMode::ProtectBudget
        );
        assert_eq!(
            normal.transition(0.50, RiskLevel::Critical, 0, 0),
            AutoMode::ProtectQuality
        );
        assert_eq!(
            normal.transition(0.50, RiskLevel::Low, 3, 0),
            AutoMode::Recovering
        );
        assert_eq!(
            normal.transition(0.50, RiskLevel::Low, 0, 0),
            AutoMode::Normal
        );

        let pb = AutoMode::ProtectBudget;
        assert_eq!(pb.transition(0.60, RiskLevel::Low, 0, 35), AutoMode::Normal);
        assert_eq!(
            pb.transition(0.70, RiskLevel::Low, 0, 35),
            AutoMode::ProtectBudget
        );
    }
}
