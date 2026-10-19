//! Model profiles: the smallest model layer.
//!
//! A profile names one engine and one model, with what that model costs.
//! Roles (only the worker, for now) pick a profile by id. Cortex ships the
//! profiles in `core/profiles.toml`; a `profiles.toml` in CORTEX_HOME
//! replaces shipped entries by id and adds new ones.

use crate::payload::Tokens;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

const SHIPPED: &str = include_str!("../profiles.toml");

/// Which engine runs a profile's model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EngineKind {
    Fake,
    ClaudeCli,
    Api,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub engine: EngineKind,
    pub model: String,
    #[serde(default)]
    pub price_in_per_mtok_usd: Option<f64>,
    #[serde(default)]
    pub price_out_per_mtok_usd: Option<f64>,
    #[serde(default)]
    pub price_cache_read_per_mtok_usd: Option<f64>,
    #[serde(default)]
    pub price_cache_write_per_mtok_usd: Option<f64>,
    #[serde(default)]
    pub context_tokens: Option<u64>,
    #[serde(default)]
    pub knowledge_cutoff: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Where the prices came from, and when.
    #[serde(default)]
    pub price_source: Option<String>,
}

impl Profile {
    /// Results from this profile are provisional: Claude Code is a whole
    /// agent of its own, so they measure it as much as Cortex.
    pub fn provisional(&self) -> bool {
        self.engine == EngineKind::ClaudeCli
    }

    /// Whether a reply from `model` came from this profile's model: the
    /// same name, or the same name with a date after it.
    pub fn serves(&self, model: &str) -> bool {
        match model.strip_prefix(self.model.as_str()) {
            Some("") => true,
            Some(rest) => rest
                .strip_prefix('-')
                .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())),
            None => false,
        }
    }
}

/// Which profile each role uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roles {
    pub worker: String,
}

impl Default for Roles {
    fn default() -> Self {
        Roles {
            worker: "claude-cli-sonnet".into(),
        }
    }
}

#[derive(Deserialize)]
struct ProfilesFile {
    #[serde(default)]
    profile: Vec<Profile>,
}

fn parse(text: &str, origin: &str) -> Result<Vec<Profile>> {
    let file: ProfilesFile =
        toml::from_str(text).map_err(|e| Error::Invalid(format!("{origin}: {e}")))?;
    let mut seen = std::collections::HashSet::new();
    for p in &file.profile {
        if !seen.insert(p.id.as_str()) {
            return Err(Error::Invalid(format!(
                "{origin}: profile `{}` appears twice",
                p.id
            )));
        }
    }
    Ok(file.profile)
}

/// The shipped profiles, then `cortex_home/profiles.toml` over them.
pub fn load_profiles(cortex_home: &Path) -> Result<Vec<Profile>> {
    let mut profiles = parse(SHIPPED, "shipped profiles.toml")?;
    let path = cortex_home.join("profiles.toml");
    if !path.exists() {
        return Ok(profiles);
    }
    let text = std::fs::read_to_string(&path)?;
    for p in parse(&text, &path.display().to_string())? {
        match profiles.iter_mut().find(|q| q.id == p.id) {
            Some(q) => *q = p,
            None => profiles.push(p),
        }
    }
    Ok(profiles)
}

/// The profile with this id.
pub fn find(profiles: &[Profile], id: &str) -> Result<Profile> {
    profiles
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| {
            let known: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
            Error::Invalid(format!("no profile `{id}`; known: {}", known.join(", ")))
        })
}

/// A shipped profile, ignoring any home overrides.
pub fn builtin(id: &str) -> Result<Profile> {
    find(&parse(SHIPPED, "shipped profiles.toml")?, id)
}

/// What these tokens cost at this profile's prices, in millionths of a
/// dollar. `None` when a needed price or token count is missing: an
/// unknown cost is never reported as zero.
pub fn estimate_usd_micros(p: &Profile, t: &Tokens) -> Option<u64> {
    // Dollars per million tokens is exactly micro-dollars per token.
    let mut micros =
        t.input? as f64 * p.price_in_per_mtok_usd? + t.output? as f64 * p.price_out_per_mtok_usd?;
    // Providers leave cache counts out when there were none.
    for (count, price) in [
        (t.cache_read, p.price_cache_read_per_mtok_usd),
        (t.cache_write, p.price_cache_write_per_mtok_usd),
    ] {
        match count.unwrap_or(0) {
            0 => {}
            n => micros += n as f64 * price?,
        }
    }
    Some(micros.round() as u64)
}
