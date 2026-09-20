//! Configuration & context resolution for the Cortex TUI client.
//!
//! Resolves three things:
//!   1. The API base URL (env override, persisted config, or sensible default).
//!   2. The auth token (env, persisted config, or none → server local fallback).
//!   3. The project context — derived from the current working directory so chat
//!      is scoped to the project the user is standing in.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Default production API endpoint (matches the frontend's non-dev default).
const DEFAULT_API_BASE: &str = "https://api.heyvera.org";

/// Persisted, user-level configuration (`~/.config/cortex/config.json`).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StoredConfig {
    /// Clerk JWT (or other bearer token) used for authenticated requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Override for the API base URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_base: Option<String>,
}

impl StoredConfig {
    fn config_path() -> Option<PathBuf> {
        dirs_next::config_dir().map(|d| d.join("cortex").join("config.json"))
    }

    /// Load persisted config, returning defaults if the file is missing/unreadable.
    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist config to disk, creating parent directories as needed.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path().context("could not resolve config directory")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating config dir {}", parent.display()))?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, raw).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

/// Fully resolved runtime configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub api_base: String,
    pub token: Option<String>,
    pub project: ProjectContext,
}

impl Config {
    /// Resolve config from env, persisted file, and the current working directory.
    pub fn resolve() -> Result<Self> {
        let stored = StoredConfig::load();

        let api_base = std::env::var("CORTEX_API")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or(stored.api_base)
            .unwrap_or_else(|| DEFAULT_API_BASE.to_string())
            .trim_end_matches('/')
            .to_string();

        let token = std::env::var("CORTEX_TOKEN")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or(stored.token);

        let cwd = std::env::current_dir().context("reading current directory")?;
        let project = ProjectContext::detect(&cwd);

        Ok(Self {
            api_base,
            token,
            project,
        })
    }

    /// Build a full URL for an API path (which must begin with `/`).
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path)
    }
}

/// Project context derived from the working directory. Scopes chat so the
/// assistant knows which project the user is operating on.
#[derive(Debug, Clone)]
pub struct ProjectContext {
    /// Human-readable project name (the project root's directory name).
    pub name: String,
    /// Absolute path to the detected project root.
    pub root: PathBuf,
    /// True when a recognized project marker (.git, .cortex, Cargo.toml, …) was found.
    pub detected: bool,
}

impl ProjectContext {
    /// Walk upward from `start` looking for a project-root marker. Falls back to
    /// `start` itself if nothing is found.
    pub fn detect(start: &Path) -> Self {
        const MARKERS: &[&str] = &[
            ".cortex",
            ".git",
            "Cargo.toml",
            "package.json",
            "pyproject.toml",
            "go.mod",
            ".replit",
        ];

        let mut dir = Some(start);
        while let Some(current) = dir {
            for marker in MARKERS {
                if current.join(marker).exists() {
                    return Self {
                        name: dir_name(current),
                        root: current.to_path_buf(),
                        detected: true,
                    };
                }
            }
            dir = current.parent();
        }

        Self {
            name: dir_name(start),
            root: start.to_path_buf(),
            detected: false,
        }
    }
}

fn dir_name(p: &Path) -> String {
    p.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| p.display().to_string())
}
