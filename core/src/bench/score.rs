//! What one bench attempt produced and cost, and the sums a results file
//! reports: pass rate, pass^k per task, and cost per accepted result.

use super::task::Kind;
use crate::payload::Spend;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What one accepted result cost, spread over every attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PerAccepted {
    Usd {
        micros: u64,
    },
    /// Nothing was accepted, so no amount buys a result.
    Unbounded,
    /// Some attempt's price is not known. Never reported as 0.
    Unknown,
}

/// A check that failed on the last thing the worker produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub check_id: String,
    /// True for a check the worker never saw.
    pub hidden: bool,
    pub output_tail: String,
}

/// The plain account of one attempt, for the results page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Story {
    pub request: String,
    pub commit: Option<String>,
    pub files_changed: Vec<String>,
    pub failures: Vec<Failure>,
    pub stuck_reason: Option<String>,
}

/// One try at one task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    pub task: String,
    pub kind: Kind,
    #[serde(rename = "try")]
    pub try_no: u32,
    pub mission_id: String,
    /// The mission's own checks passed.
    pub mission_done: bool,
    /// Done, and every hidden check passed too.
    pub accepted: bool,
    pub hidden_failed: Vec<String>,
    pub sessions: u32,
    pub seconds: f64,
    /// Input plus output tokens over every model call.
    pub tokens: u64,
    /// `None` when any call's price is not known.
    pub estimated_usd_micros: Option<u64>,
    pub spent: Spend,
    pub story: Story,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub accepted: u32,
    pub total: u32,
    pub pass_rate: f64,
    /// Per task: the chance that k fresh tries all pass, k = tries.
    pub pass_hat_k: BTreeMap<String, f64>,
    pub tokens: u64,
    pub seconds: f64,
    pub cost_per_accepted: PerAccepted,
}

/// Every attempt's estimated cost, failed ones included, over the number
/// accepted.
pub fn cost_per_accepted(attempts: &[Attempt]) -> PerAccepted {
    let accepted = attempts.iter().filter(|a| a.accepted).count() as u64;
    if accepted == 0 {
        return PerAccepted::Unbounded;
    }
    match attempts
        .iter()
        .map(|a| a.estimated_usd_micros)
        .sum::<Option<u64>>()
    {
        Some(total) => PerAccepted::Usd {
            micros: total / accepted,
        },
        None => PerAccepted::Unknown,
    }
}

/// pass^k: with `c` of `n` tries passing, the chance that `k` tries drawn
/// without replacement all pass, C(c,k)/C(n,k).
pub fn pass_hat_k(c: u32, n: u32, k: u32) -> f64 {
    if c < k || k > n {
        return 0.0;
    }
    (0..k)
        .map(|i| f64::from(c - i) / f64::from(n - i))
        .product()
}

pub fn summarize(attempts: &[Attempt], tries: u32) -> Summary {
    let accepted = attempts.iter().filter(|a| a.accepted).count() as u32;
    let total = attempts.len() as u32;
    let mut per_task: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for a in attempts {
        let e = per_task.entry(a.task.clone()).or_default();
        e.0 += u32::from(a.accepted);
        e.1 += 1;
    }
    Summary {
        accepted,
        total,
        pass_rate: if total == 0 {
            0.0
        } else {
            f64::from(accepted) / f64::from(total)
        },
        pass_hat_k: per_task
            .into_iter()
            .map(|(id, (c, n))| (id, pass_hat_k(c, n, tries.min(n))))
            .collect(),
        tokens: attempts.iter().map(|a| a.tokens).sum(),
        seconds: attempts.iter().map(|a| a.seconds).sum(),
        cost_per_accepted: cost_per_accepted(attempts),
    }
}

/// Usd calls add up; all-subscription stays subscription; any mix or
/// unknown is unknown. No calls at all is unknown, never free.
pub fn total_spend(spends: &[Spend]) -> Spend {
    if spends.is_empty() {
        return Spend::Unknown;
    }
    if spends.iter().all(|s| *s == Spend::Subscription) {
        return Spend::Subscription;
    }
    let mut micros = 0;
    for s in spends {
        match s {
            Spend::Usd { micros: m } => micros += m,
            _ => return Spend::Unknown,
        }
    }
    Spend::Usd { micros }
}
