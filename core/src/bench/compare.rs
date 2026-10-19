//! Paired comparison of two arms over the same tasks and tries. The margin
//! and cost cap are the numbers a comparison PR states before the run.

use super::Report;
use super::score::PerAccepted;
use crate::{Error, Result};
use serde::Serialize;
use std::collections::BTreeMap;

/// The win rule, decided in advance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rule {
    /// How many more accepted results B needs than A.
    pub win_margin: u32,
    /// The most B's cost per accepted result may be, as a multiple of A's.
    pub max_cost_ratio: f64,
}

/// B's cost per accepted result over A's.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Ratio {
    Times(f64),
    /// B accepted nothing, or A's results were free and B's were not.
    Unbounded,
    /// A price is not known on either side.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Win,
    NoWin,
    /// The margin was met but a cost was unknown, so the cap can't be judged.
    Undecided,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Comparison {
    /// Accepted results for (A, B).
    pub accepted: (u32, u32),
    /// Task-and-try pairs only A passed, and only B passed.
    pub discordant: (u32, u32),
    /// Exact two-sided sign test on the discordant pairs. Reported, not a gate.
    pub p_value: f64,
    pub cost_ratio: Ratio,
    pub verdict: Verdict,
}

pub fn compare(a: &Report, b: &Report, rule: &Rule) -> Result<Comparison> {
    let pairs = |r: &Report| -> Result<BTreeMap<(String, u32), bool>> {
        let mut out = BTreeMap::new();
        for at in &r.attempts {
            if out
                .insert((at.task.clone(), at.try_no), at.accepted)
                .is_some()
            {
                return Err(Error::Invalid(format!(
                    "{}: {} try {} appears twice",
                    r.arm, at.task, at.try_no
                )));
            }
        }
        Ok(out)
    };
    let (pa, pb) = (pairs(a)?, pairs(b)?);
    if !pa.keys().eq(pb.keys()) {
        return Err(Error::Invalid(format!(
            "{} and {} did not run the same tasks and tries; only a paired run can be compared",
            a.arm, b.arm
        )));
    }

    let count = |p: &BTreeMap<_, bool>| p.values().filter(|&&ok| ok).count() as u32;
    let accepted = (count(&pa), count(&pb));
    let mut discordant = (0, 0);
    for (key, &ok_a) in &pa {
        match (ok_a, pb[key]) {
            (true, false) => discordant.0 += 1,
            (false, true) => discordant.1 += 1,
            _ => {}
        }
    }

    let cost_ratio = ratio(&a.summary.cost_per_accepted, &b.summary.cost_per_accepted);
    let verdict = if accepted.1 < accepted.0 + rule.win_margin {
        Verdict::NoWin
    } else {
        match cost_ratio {
            Ratio::Unknown => Verdict::Undecided,
            Ratio::Times(r) if r <= rule.max_cost_ratio => Verdict::Win,
            _ => Verdict::NoWin,
        }
    };
    Ok(Comparison {
        accepted,
        discordant,
        p_value: sign_test(discordant.0, discordant.1),
        cost_ratio,
        verdict,
    })
}

/// B over A, carrying unknown and unbounded through instead of guessing.
pub fn ratio(a: &PerAccepted, b: &PerAccepted) -> Ratio {
    match (a, b) {
        (PerAccepted::Unknown, _) | (_, PerAccepted::Unknown) => Ratio::Unknown,
        (_, PerAccepted::Unbounded) => Ratio::Unbounded,
        (PerAccepted::Unbounded, PerAccepted::Usd { .. }) => Ratio::Times(0.0),
        (PerAccepted::Usd { micros: 0 }, PerAccepted::Usd { micros: 0 }) => Ratio::Times(1.0),
        (PerAccepted::Usd { micros: 0 }, PerAccepted::Usd { .. }) => Ratio::Unbounded,
        (PerAccepted::Usd { micros: x }, PerAccepted::Usd { micros: y }) => {
            Ratio::Times(*y as f64 / *x as f64)
        }
    }
}

/// Exact two-sided sign test: the chance of a split at least this uneven
/// if both arms were equally good. No discordant pairs gives 1.
pub fn sign_test(x: u32, y: u32) -> f64 {
    let n = x + y;
    let low = x.min(y);
    let mut c = 1.0_f64; // C(n, 0)
    let mut tail = 0.0;
    for i in 0..=low {
        if i > 0 {
            c = c * f64::from(n - i + 1) / f64::from(i);
        }
        tail += c;
    }
    (2.0 * tail / 2f64.powi(n as i32)).min(1.0)
}
