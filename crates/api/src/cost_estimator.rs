use cortex_core::usage::{estimate_cost, estimate_cost_with_cache};

/// Cost estimator for tracking BYOK (bring your own key) costs.
/// This module provides more detailed cost estimation for budget tracking,
/// extending the basic estimation in cortex_core::usage.
pub struct CostEstimator;

impl CostEstimator {
    /// Estimate the cost of a request before sending it.
    /// Returns (estimated_cost, breakdown) where breakdown contains details.
    pub fn estimate_request_cost(
        provider: &str,
        model: &str,
        estimated_input_tokens: i64,
        estimated_output_tokens: i64,
        cached_tokens: Option<i64>,
    ) -> (f64, RequestCostBreakdown) {
        let cost = if let Some(cached) = cached_tokens {
            estimate_cost_with_cache(
                provider,
                model,
                estimated_input_tokens,
                cached,
                estimated_output_tokens,
            )
        } else {
            estimate_cost(
                provider,
                model,
                estimated_input_tokens,
                estimated_output_tokens,
            )
        };

        let breakdown = RequestCostBreakdown {
            provider: provider.to_string(),
            model: model.to_string(),
            input_tokens: estimated_input_tokens,
            output_tokens: estimated_output_tokens,
            cached_tokens,
            total_cost: cost,
            input_cost: Self::calculate_input_cost(
                provider,
                model,
                estimated_input_tokens,
                cached_tokens,
            ),
            output_cost: Self::calculate_output_cost(provider, model, estimated_output_tokens),
        };

        (cost, breakdown)
    }

    /// Calculate actual cost based on real token usage.
    pub fn calculate_actual_cost(
        provider: &str,
        model: &str,
        actual_input_tokens: i64,
        actual_output_tokens: i64,
        cached_tokens: Option<i64>,
    ) -> f64 {
        if let Some(cached) = cached_tokens {
            estimate_cost_with_cache(
                provider,
                model,
                actual_input_tokens,
                cached,
                actual_output_tokens,
            )
        } else {
            estimate_cost(provider, model, actual_input_tokens, actual_output_tokens)
        }
    }

    /// Estimate token count from text content.
    /// This is a heuristic - roughly 4 chars per token for most models.
    pub fn estimate_tokens_from_text(text: &str) -> i64 {
        (text.len() as f64 / 4.0).ceil() as i64
    }

    /// Estimate input tokens for a typical request based on context and prompt.
    pub fn estimate_input_tokens(
        system_prompt: &str,
        user_message: &str,
        context_size: Option<usize>,
    ) -> i64 {
        let base_tokens = Self::estimate_tokens_from_text(system_prompt)
            + Self::estimate_tokens_from_text(user_message);

        // Add context tokens if provided
        let context_tokens = context_size.unwrap_or(0) as i64;

        // Add some overhead for message formatting
        let overhead = 50;

        base_tokens + context_tokens + overhead
    }

    /// Estimate output tokens based on request type and model.
    pub fn estimate_output_tokens(request_type: &str, model: &str) -> i64 {
        match (request_type, model) {
            ("chat", m) if m.contains("haiku") => 512, // Quick responses
            ("chat", m) if m.contains("opus") => 2048, // Detailed responses
            ("chat", _) => 1024,                       // Sonnet/default
            ("code", _) => 4096,                       // Code generation
            ("search", _) => 256,                      // Search/lookup
            ("summary", _) => 512,                     // Summarization
            _ => 1024,                                 // Default estimate
        }
    }

    /// Check if a provider supports cost tracking (BYOK vs BYOS).
    pub fn is_byok_provider(provider: &str) -> bool {
        matches!(provider, "claude" | "openai" | "gemini")
    }

    /// Get cost type for a provider-model combination.
    pub fn get_cost_type(provider: &str, has_api_key: bool) -> String {
        if Self::is_byok_provider(provider) && has_api_key {
            "byok".to_string()
        } else {
            "byos".to_string()
        }
    }

    fn calculate_input_cost(
        provider: &str,
        model: &str,
        tokens: i64,
        cached_tokens: Option<i64>,
    ) -> f64 {
        let (in_rate, _) = Self::get_model_rates(provider, model);

        if let Some(cached) = cached_tokens {
            let cache_discount = match provider {
                "claude" => 0.1, // 90% discount on cached
                "openai" => 0.5, // 50% discount estimate
                _ => 0.5,
            };
            let regular_tokens = (tokens - cached).max(0);
            let cached_cost = (cached as f64 / 1000.0) * in_rate * cache_discount;
            let regular_cost = (regular_tokens as f64 / 1000.0) * in_rate;
            cached_cost + regular_cost
        } else {
            (tokens as f64 / 1000.0) * in_rate
        }
    }

    fn calculate_output_cost(provider: &str, model: &str, tokens: i64) -> f64 {
        let (_, out_rate) = Self::get_model_rates(provider, model);
        (tokens as f64 / 1000.0) * out_rate
    }

    fn get_model_rates(provider: &str, model: &str) -> (f64, f64) {
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
            // Deliberately frontier-priced until measured against a real Zen
            // invoice — overestimating COGS is the safe direction. Task 1.4.
            ("zen", _) => (0.003, 0.015),
            _ => (0.003, 0.015),
        }
    }
}

/// Detailed breakdown of request cost estimation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RequestCostBreakdown {
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: Option<i64>,
    pub total_cost: f64,
    pub input_cost: f64,
    pub output_cost: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_request_cost() {
        let (cost, breakdown) = CostEstimator::estimate_request_cost(
            "claude",
            "claude-3-sonnet-20240229",
            1000,
            500,
            None,
        );

        // Expected: (1000/1000) * 0.003 + (500/1000) * 0.015 = 0.003 + 0.0075 = 0.0105
        assert!((cost - 0.0105).abs() < 0.0001);
        assert_eq!(breakdown.provider, "claude");
        assert_eq!(breakdown.input_tokens, 1000);
        assert_eq!(breakdown.output_tokens, 500);
    }

    #[test]
    fn test_estimate_with_cache() {
        let (cost, _) = CostEstimator::estimate_request_cost(
            "claude",
            "claude-3-sonnet-20240229",
            1000,
            500,
            Some(800), // 800 tokens cached
        );

        // Expected:
        // Cached: (800/1000) * 0.003 * 0.1 = 0.00024
        // Regular: (200/1000) * 0.003 = 0.0006
        // Output: (500/1000) * 0.015 = 0.0075
        // Total: 0.00024 + 0.0006 + 0.0075 = 0.00834
        assert!((cost - 0.00834).abs() < 0.0001);
    }

    #[test]
    fn test_token_estimation() {
        assert_eq!(CostEstimator::estimate_tokens_from_text("hello world"), 3);
        assert_eq!(CostEstimator::estimate_tokens_from_text(""), 0);
        assert_eq!(CostEstimator::estimate_tokens_from_text("a"), 1);
    }

    #[test]
    fn test_cost_type_detection() {
        assert_eq!(CostEstimator::get_cost_type("claude", true), "byok");
        assert_eq!(CostEstimator::get_cost_type("claude", false), "byos");
        assert_eq!(CostEstimator::get_cost_type("unknown", true), "byos");
    }

    #[test]
    fn test_output_estimation() {
        assert_eq!(
            CostEstimator::estimate_output_tokens("chat", "claude-3-haiku"),
            512
        );
        assert_eq!(
            CostEstimator::estimate_output_tokens("code", "claude-3-sonnet"),
            4096
        );
        assert_eq!(
            CostEstimator::estimate_output_tokens("summary", "gpt-4"),
            512
        );
    }
}
