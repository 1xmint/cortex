use serde::{Deserialize, Serialize};

/// Aggregated usage summary, optionally broken down by provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSummary {
    pub total_tokens_in: i64,
    pub total_tokens_out: i64,
    pub total_cost_estimate: f64,
    pub step_count: i64,
    pub by_provider: Vec<ProviderUsage>,
}

/// Usage breakdown for a single provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub provider: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cost_estimate: f64,
    pub step_count: i64,
}

/// Daily usage aggregation for charts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyUsage {
    pub date: String, // "2026-05-19"
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cost_estimate: f64,
    pub step_count: i64,
}

/// Configurable usage limits for billing gating.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageLimits {
    pub daily_cost_limit: f64,
    pub daily_step_limit: i64,
    pub monthly_cost_limit: f64,
}

impl Default for UsageLimits {
    fn default() -> Self {
        Self {
            daily_cost_limit: 10.0,    // $10/day
            daily_step_limit: 100,     // 100 steps/day
            monthly_cost_limit: 200.0, // $200/month
        }
    }
}

/// Rough cost estimation based on token counts and provider/tier.
/// These are approximate $/1K-token rates; real billing would come from provider APIs.
/// Projected cost of an entire run before execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostProjection {
    pub estimated_total_cost: f64,
    pub estimated_duration_minutes: f64,
    pub step_estimates: Vec<StepCostEstimate>,
    pub confidence: f64, // 0.0-1.0 based on sample count
}

/// Projected cost of a single step within a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepCostEstimate {
    pub kind: String,
    pub tier: String,
    pub provider: String,
    pub estimated_tokens_in: i64,
    pub estimated_tokens_out: i64,
    pub estimated_cost: f64,
    pub estimated_duration_ms: i64,
}

/// Model-aware cost estimation. Rates in $/1K tokens.
pub fn estimate_cost(provider: &str, model: &str, tokens_in: i64, tokens_out: i64) -> f64 {
    let (in_rate, out_rate) = model_rates(provider, model);
    (tokens_in as f64 / 1000.0) * in_rate + (tokens_out as f64 / 1000.0) * out_rate
}

/// Convenience wrapper when only provider is known (uses default tier pricing).
pub fn estimate_cost_by_provider(provider: &str, tokens_in: i64, tokens_out: i64) -> f64 {
    estimate_cost(provider, "", tokens_in, tokens_out)
}

/// Cache-aware cost estimation. Cached input tokens get a provider-specific discount.
pub fn estimate_cost_with_cache(
    provider: &str,
    model: &str,
    tokens_in: i64,
    cached_tokens_in: i64,
    tokens_out: i64,
) -> f64 {
    let (in_rate, out_rate) = model_rates(provider, model);
    let cache_discount = match provider {
        "claude" => 0.1, // 90% discount on cached
        "openai" => 0.5, // 50% discount estimate
        _ => 0.5,
    };
    let regular_in = (tokens_in - cached_tokens_in).max(0);
    let cached_cost = (cached_tokens_in as f64 / 1000.0) * in_rate * cache_discount;
    let regular_cost = (regular_in as f64 / 1000.0) * in_rate;
    let out_cost = (tokens_out as f64 / 1000.0) * out_rate;
    cached_cost + regular_cost + out_cost
}

fn model_rates(provider: &str, model: &str) -> (f64, f64) {
    match (provider, model) {
        // Claude models
        ("claude", m) if m.contains("haiku") => (0.0008, 0.004),
        ("claude", m) if m.contains("opus") => (0.015, 0.075),
        ("claude", _) => (0.003, 0.015), // Sonnet default
        // OpenAI models
        ("openai", m) if m.contains("mini") => (0.0004, 0.0016),
        ("openai", m) if m.contains("5.5") || m.contains("o3") => (0.010, 0.040),
        ("openai", _) => (0.005, 0.015), // GPT-5.4 default
        // Gemini models
        ("gemini", m) if m.contains("flash") => (0.00015, 0.0006),
        ("gemini", m) if m.contains("pro") => (0.00125, 0.005),
        ("gemini", _) => (0.00035, 0.001),
        _ => (0.003, 0.015),
    }
}
