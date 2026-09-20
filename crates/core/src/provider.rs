use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Claude,
    Openai,
    Gemini,
    /// OpenCode Zen — an OpenAI-compatible gateway carrying GLM, Kimi, Qwen,
    /// and Grok models under one API key. API-only: it has no CLI, so any
    /// CLI/subscription code path must reject it rather than spawn.
    Zen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Search,
    Execute,
    Think,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModel {
    pub provider: ProviderId,
    pub tier: Tier,
    pub model_id: String,
    pub cli_command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub provider: ProviderId,
    pub authenticated: bool,
    pub pressure: f64,
    pub available_tiers: Vec<Tier>,
}

impl ProviderId {
    pub fn cli_name(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Openai => "codex",
            Self::Gemini => "gemini",
            // No CLI exists; kept as a label so worker capability maps stay total.
            Self::Zen => "zen",
        }
    }
}

impl Tier {
    pub fn rank(&self) -> u8 {
        match self {
            Self::Search => 0,
            Self::Execute => 1,
            Self::Think => 2,
        }
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Claude => write!(f, "claude"),
            Self::Openai => write!(f, "openai"),
            Self::Gemini => write!(f, "gemini"),
            Self::Zen => write!(f, "zen"),
        }
    }
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Search => write!(f, "search"),
            Self::Execute => write!(f, "execute"),
            Self::Think => write!(f, "think"),
        }
    }
}
