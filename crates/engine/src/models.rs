use cortex_core::provider::{ProviderId, ProviderModel, Tier};

const REGISTRY: &[(ProviderId, Tier, &str, &str)] = &[
    (
        ProviderId::Claude,
        Tier::Search,
        "claude-haiku-4-5",
        "claude",
    ),
    (
        ProviderId::Claude,
        Tier::Execute,
        "claude-sonnet-4-6",
        "claude",
    ),
    (ProviderId::Claude, Tier::Think, "claude-opus-4-6", "claude"),
    (ProviderId::Openai, Tier::Search, "gpt-4.1-mini", "codex"),
    (ProviderId::Openai, Tier::Execute, "gpt-5.4", "codex"),
    (ProviderId::Openai, Tier::Think, "gpt-5.5", "codex"),
    (
        ProviderId::Gemini,
        Tier::Search,
        "gemini-2.5-flash",
        "gemini",
    ),
    (
        ProviderId::Gemini,
        Tier::Execute,
        "gemini-2.5-pro",
        "gemini",
    ),
    (ProviderId::Gemini, Tier::Think, "gemini-2.5-pro", "gemini"),
    // OpenCode Zen model IDs (opencode.ai/docs/zen). "zen" is a label, not a
    // CLI — the provider is API-only. Picks are uncalibrated defaults; Phase
    // 2.5's eval suite owns revising them.
    (ProviderId::Zen, Tier::Search, "glm-5", "zen"),
    (ProviderId::Zen, Tier::Execute, "glm-5.2", "zen"),
    (ProviderId::Zen, Tier::Think, "kimi-k3", "zen"),
];

pub fn resolve_model(provider: ProviderId, tier: Tier) -> Option<ProviderModel> {
    REGISTRY
        .iter()
        .find(|(p, t, _, _)| *p == provider && *t == tier)
        .map(|(p, t, model_id, cli)| ProviderModel {
            provider: *p,
            tier: *t,
            model_id: model_id.to_string(),
            cli_command: cli.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_claude_execute() {
        let m = resolve_model(ProviderId::Claude, Tier::Execute).unwrap();
        assert_eq!(m.model_id, "claude-sonnet-4-6");
        assert_eq!(m.cli_command, "claude");
    }

    #[test]
    fn resolves_openai_think() {
        let m = resolve_model(ProviderId::Openai, Tier::Think).unwrap();
        assert_eq!(m.model_id, "gpt-5.5");
        assert_eq!(m.cli_command, "codex");
    }

    #[test]
    fn resolves_zen_for_every_tier() {
        for tier in [Tier::Search, Tier::Execute, Tier::Think] {
            let m = resolve_model(ProviderId::Zen, tier).unwrap();
            assert!(!m.model_id.is_empty());
            assert_eq!(m.cli_command, "zen");
        }
    }
}
