//! The catalog, the estimator, and the reason a price is never invented.
//!
//! # What was broken
//!
//! `verification_driver` reaches a billable verdict, finds `quoted_credits` is
//! `None`, logs a warning and declines to touch the ledger. That refusal is
//! correct and it has been the state of the money path since V3 shipped: Cortex
//! can verify an outcome and cannot charge for one.
//!
//! The missing piece was never the charging code. It was a price that is not
//! invented — and "not invented" is a specific, checkable property rather than
//! a good intention:
//!
//! 1. **It is data, not code.** `usage::model_rates` matches on substrings of
//!    model ids (`m.contains("haiku")`). That is invariant 11's exact
//!    prohibition — pricing code carrying hardcoded model names — and it means
//!    the price of a task changes when someone edits a `match` arm, with no
//!    version, no record, and no way for a receipt to say which rates applied.
//! 2. **It is versioned and immutable.** Invariant 23: published, never edited.
//!    Enforced by triggers in migration v66, not by convention.
//! 3. **It is labelled with how much it is worth trusting.** A seeded price
//!    with zero measured samples is `provisional`, and a provisional price
//!    **quotes but does not charge**.
//!
//! # The graduation gate is the whole design
//!
//! Phase 31.3 states the uncomfortable structural fact: outcome pricing
//! succeeds at vendors with years of outcome data, and Cortex has none. It is
//! being asked to underwrite before it can price.
//!
//! The resolution already in the plan is that the guarantee graduates the way
//! pricing graduates — a class is eligible only once its measured pass rate and
//! cost distribution clear a threshold. This module implements that as the
//! difference between two statuses:
//!
//! | Status | Quoted | Charged | Meaning |
//! |---|---|---|---|
//! | `provisional` | yes | **no** | Seeded from measured provider spend plus a margin, zero outcome samples. The number is published so it can be checked; it does not move money. |
//! | `committed` | yes | yes | Measured. Graduated through Phase 31.3's gate. |
//!
//! So this unblocks `quoted_credits: None` without inventing a price *and*
//! without silently switching on billing. Turning a class committed is a
//! deliberate, recorded, commercial act — which is what it should be.
//!
//! # What this deliberately does not do
//!
//! It does not make the router cost-aware (PR J) and it does not forecast a run
//! (PR Q). Both need this table and neither is here. Stated so the absence is a
//! decision rather than something a reader has to discover.

use std::collections::BTreeMap;

use cortex_core::routing::RiskLevel;
use cortex_core::task::WorkKind;
use cortex_core::task_class::TaskClass;
use serde::{Deserialize, Serialize};

/// Micros per US dollar. Prices are integers throughout: money that
/// round-trips through an `f64` disagrees with itself at the third decimal, and
/// these numbers get multiplied by token counts in the millions.
pub const MICROS_PER_USD: i64 = 1_000_000;

/// Basis points in one whole. A 40% margin is 4_000.
pub const BP_PER_WHOLE: i64 = 10_000;

/// What one credit is worth in micros, on a **seeded** list.
///
/// A credit is a verified task, integer-denominated, never tokens — settled in
/// `CREDITS.md` and not this module's decision. What *is* a decision is how many
/// dollars a credit stands for, and it lives on the price list rather than in a
/// constant for two reasons.
///
/// The first is invariant 11: it is a fact, so it belongs in the one versioned
/// table with the other facts, and a receipt that names a list version then
/// names this too.
///
/// The second is that this number is the **resolution of the whole price
/// space**. At a dollar a credit, every modelled class from a trivial gate to a
/// critical refactor rounds to one credit: the list stops distinguishing work
/// it exists to distinguish, and the failure is invisible because every row
/// still holds a plausible-looking number. That is not hypothetical — it is
/// what the first seeded list did, and the test below is what caught it. This
/// seed is chosen so the modelled spread survives rounding.
///
/// **What a credit is worth commercially is Josh's decision, not this seed's.**
/// The list is `provisional` precisely so the number can be published and
/// argued with without charging anyone.
pub const SEED_MICROS_PER_CREDIT: i64 = 100_000;

/// A price list's, or a class's, standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceStatus {
    /// Seeded, published, checkable — and never billed. See the module docs.
    Provisional,
    /// Measured and graduated through Phase 31.3's gate.
    Committed,
}

impl PriceStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Provisional => "provisional",
            Self::Committed => "committed",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "provisional" => Some(Self::Provisional),
            "committed" => Some(Self::Committed),
            _ => None,
        }
    }

    /// Whether a quote at this status may move the ledger.
    ///
    /// The single place that question is answered. A second answer somewhere
    /// else is a second thing that can disagree with the label a customer was
    /// shown, and the disagreement would be in the direction of charging.
    pub fn may_bill(&self) -> bool {
        matches!(self, Self::Committed)
    }
}

/// One model's facts. Invariant 11's row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPrice {
    pub provider: String,
    pub model_id: String,
    pub input_micros_per_1k: i64,
    pub output_micros_per_1k: i64,
    /// Basis points of the input rate charged for a cache read. A 90% discount
    /// is 1_000.
    pub cache_read_bp: i64,
    pub context_window: i64,
    pub capability_class: String,
}

impl ModelPrice {
    /// Cost of a completion, in micros. Integer arithmetic end to end.
    pub fn cost_micros(&self, tokens_in: i64, cached_in: i64, tokens_out: i64) -> i64 {
        let uncached_in = (tokens_in - cached_in).max(0);
        let cached =
            cached_in * self.input_micros_per_1k * self.cache_read_bp / (1_000 * BP_PER_WHOLE);
        let regular = uncached_in * self.input_micros_per_1k / 1_000;
        let out = tokens_out * self.output_micros_per_1k / 1_000;
        cached + regular + out
    }
}

/// One class's price, and the evidence behind it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassPrice {
    /// `TaskClass::key()`.
    pub task_class: String,
    pub quoted_credits: i64,
    pub status: PriceStatus,
    /// How many resolved outcomes this price was measured from. Zero on a
    /// seeded list, and visibly so — a class cannot graduate on no evidence,
    /// and the number is what makes that checkable rather than asserted.
    pub sample_count: i64,
    /// Measured provider spend per outcome, in micros. `None` on a seeded list
    /// where the figure came from a modelled estimate rather than from
    /// resolved runs.
    pub measured_cost_micros: Option<i64>,
    pub margin_bp: i64,
}

/// A published price list. Immutable — see migration v66's triggers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceList {
    pub id: String,
    pub version: i64,
    pub status: PriceStatus,
    /// What one credit is worth, in micros, on this list. A published field
    /// rather than a constant — see [`SEED_MICROS_PER_CREDIT`].
    pub micros_per_credit: i64,
    /// How the numbers were arrived at, in prose. A price whose derivation
    /// lives in a commit message is a price nobody can audit later.
    pub basis: String,
    pub published_at: i64,
    pub published_by: String,
    pub models: Vec<ModelPrice>,
    pub classes: Vec<ClassPrice>,
}

impl PriceList {
    pub fn class(&self, class: &TaskClass) -> Option<&ClassPrice> {
        let key = class.key();
        self.classes.iter().find(|c| c.task_class == key)
    }

    pub fn model(&self, provider: &str, model_id: &str) -> Option<&ModelPrice> {
        self.models
            .iter()
            .find(|m| m.provider == provider && m.model_id == model_id)
    }
}

/// A price frozen for one step, before it ran.
///
/// Frozen for the same reason the check specs are frozen: a price resolved at
/// verdict time is a price the work could have influenced, and a customer who
/// was quoted before execution must be charged what they were quoted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepQuote {
    pub quote_id: String,
    pub run_id: String,
    pub step_id: String,
    pub task_class: String,
    pub quoted_credits: i64,
    pub price_list_id: String,
    pub price_list_version: i64,
    /// Stored, not recomputed. A class that graduates between dispatch and
    /// verdict must not retroactively make a quoted-but-free step billable —
    /// the customer was told it was free.
    pub billable: bool,
    pub frozen_at: i64,
}

impl StepQuote {
    /// What the ledger may be moved by, or `None`.
    ///
    /// The only accessor `verification_driver` should use. Reading
    /// `quoted_credits` directly would charge for a provisional class, which is
    /// the one thing the graduation gate exists to prevent.
    pub fn billable_credits(&self) -> Option<i64> {
        if self.billable && self.quoted_credits > 0 {
            Some(self.quoted_credits)
        } else {
            None
        }
    }
}

/// Quote a class against a list.
///
/// Returns `None` when the class is not in the list. That is deliberate and it
/// is the honest failure: a missing class means nobody decided what this work
/// costs, and the correct behaviour is the one already in the driver — record
/// the verdict, leave the ledger alone, say so.
pub fn quote(list: &PriceList, class: &TaskClass) -> Option<(i64, bool)> {
    let priced = list.class(class)?;
    // Both statuses must permit billing. A committed class inside a
    // provisional list is still provisional: the list is the published
    // artifact, and its status is a statement about the whole of it.
    let billable = priced.status.may_bill() && list.status.may_bill();
    Some((priced.quoted_credits, billable))
}

/// Build the first price list: measured provider spend, plus a margin, marked
/// `provisional`.
///
/// # Where the numbers come from
///
/// Not from a decision about what Cortex is worth. From the only thing
/// currently measurable — what a task of each class costs in provider spend —
/// with a stated margin on top, published as `provisional` so that it quotes and
/// does not charge.
///
/// The per-class expected spend is modelled rather than measured, because there
/// are no resolved outcomes to measure yet; `sample_count` is 0 on every row and
/// that is the fact that keeps every class provisional. The model is:
///
/// - a base token profile per `WorkKind` — exploring reads a lot and writes
///   little; refactoring does both; a gate does almost nothing;
/// - a risk multiplier, because higher-risk work is retried more and verified
///   harder, and the guarantee means Cortex pays for the retries;
/// - an unverifiable discount, because invariant 22 forbids pricing unproven
///   work as though it had been proven.
///
/// Every one of those is a guess with a stated shape, which is why the result
/// is `provisional` and why `basis` records it on the row. When Phase 31.3's
/// measurement exists, a new version is published from real distributions and
/// this function stops being the source.
pub fn seed_provisional(
    version: i64,
    published_by: &str,
    now: i64,
    models: Vec<ModelPrice>,
) -> PriceList {
    let margin_bp = 4_000; // 40% over expected provider spend.

    let classes = TaskClass::all()
        .into_iter()
        .map(|class| {
            let spend = modelled_spend_micros(&class);
            let with_margin = spend * (BP_PER_WHOLE + margin_bp) / BP_PER_WHOLE;
            // Round up to a whole credit, and never to zero: a task that costs
            // nothing is not a task, and a zero-credit class would make a
            // billable verdict a silent no-op later.
            let credits =
                ((with_margin + SEED_MICROS_PER_CREDIT - 1) / SEED_MICROS_PER_CREDIT).max(1);
            ClassPrice {
                task_class: class.key(),
                quoted_credits: credits,
                status: PriceStatus::Provisional,
                sample_count: 0,
                measured_cost_micros: None,
                margin_bp,
            }
        })
        .collect();

    PriceList {
        id: uuid::Uuid::new_v4().to_string(),
        version,
        status: PriceStatus::Provisional,
        micros_per_credit: SEED_MICROS_PER_CREDIT,
        basis: format!(
            "Seeded, not measured. Per-class expected provider spend is modelled from a \
             per-WorkKind token profile, a risk multiplier, and an unverifiable discount; \
             margin {margin_bp}bp over that; rounded up to whole credits at \
             {SEED_MICROS_PER_CREDIT} micros per credit — a seed chosen so the modelled \
             spread survives rounding, NOT a commercial decision about what a credit is \
             worth. sample_count is 0 on every class, so every class is provisional: \
             quoted and checkable, never charged. Replace with a version published from \
             measured outcome distributions per Phase 31.3."
        ),
        published_at: now,
        published_by: published_by.to_string(),
        models,
        classes,
    }
}

/// Expected provider spend for one outcome of a class, in micros.
///
/// Modelled, and labelled as modelled everywhere it surfaces. See
/// [`seed_provisional`].
fn modelled_spend_micros(class: &TaskClass) -> i64 {
    let base = match class.work_kind {
        // Reads widely, writes almost nothing.
        WorkKind::Explore => 40_000,
        // Reads the relevant surface and writes a diff.
        WorkKind::Modify => 120_000,
        WorkKind::Add => 150_000,
        // The most token-expensive shape: reads broadly and rewrites broadly.
        WorkKind::Refactor => 220_000,
        WorkKind::Test => 110_000,
        // Mostly mechanical; the cost is in the check runner, not the model.
        WorkKind::Build => 30_000,
        WorkKind::Lint => 30_000,
        WorkKind::Review => 90_000,
        WorkKind::Ship => 50_000,
        // Diagnosing a failure means reading output as well as source.
        WorkKind::Heal => 180_000,
        WorkKind::Gate => 20_000,
    };

    // Higher risk means more attempts and a harder exam, and under an outcome
    // guarantee Cortex pays for every attempt that did not land.
    let risk_bp = match class.risk {
        RiskLevel::Low => 10_000,
        RiskLevel::Medium => 13_000,
        RiskLevel::High => 18_000,
        RiskLevel::Critical => 25_000,
    };

    // Invariant 22: unverifiable work is never priced as though it had been
    // proven. It is also genuinely cheaper — there is no exam to run — so this
    // is one discount doing two jobs, and both point the same way.
    let verifiable_bp = if class.verifiable { 10_000 } else { 6_000 };

    base * risk_bp / BP_PER_WHOLE * verifiable_bp / BP_PER_WHOLE
}

/// The model rows for the seeded list.
///
/// These are the rates that currently live in `usage::model_rates` as a `match`
/// on model-id substrings, moved into data. Moving them is the point: after
/// this, changing a price is publishing a version rather than editing a
/// function, and a receipt can name which version applied.
///
/// Rates are per 1k tokens in micros, so `3_000` is $0.003/1k.
pub fn seed_models() -> Vec<ModelPrice> {
    fn m(
        provider: &str,
        model_id: &str,
        input: i64,
        output: i64,
        cache_read_bp: i64,
        context_window: i64,
        capability_class: &str,
    ) -> ModelPrice {
        ModelPrice {
            provider: provider.to_string(),
            model_id: model_id.to_string(),
            input_micros_per_1k: input,
            output_micros_per_1k: output,
            cache_read_bp,
            context_window,
            capability_class: capability_class.to_string(),
        }
    }

    vec![
        // The engine's currently routed Claude model ids. These provisional
        // rows intentionally share the conservative seeded rates below; live
        // observations, not this stub wiring change, own publishing revisions.
        m(
            "claude",
            "claude-opus-4-6",
            15_000,
            75_000,
            1_000,
            200_000,
            "frontier",
        ),
        m(
            "claude",
            "claude-sonnet-4-6",
            3_000,
            15_000,
            1_000,
            200_000,
            "balanced",
        ),
        m(
            "claude",
            "claude-haiku-4-5",
            800,
            4_000,
            1_000,
            200_000,
            "fast",
        ),
        m(
            "claude",
            "claude-opus-5",
            15_000,
            75_000,
            1_000,
            200_000,
            "frontier",
        ),
        m(
            "claude",
            "claude-sonnet-5",
            3_000,
            15_000,
            1_000,
            200_000,
            "balanced",
        ),
        m(
            "claude",
            "claude-haiku-4-5-20251001",
            800,
            4_000,
            1_000,
            200_000,
            "fast",
        ),
        m(
            "openai", "gpt-5.5", 10_000, 40_000, 5_000, 400_000, "frontier",
        ),
        m(
            "openai", "gpt-5.4", 5_000, 15_000, 5_000, 400_000, "balanced",
        ),
        m("openai", "gpt-5-mini", 400, 1_600, 5_000, 400_000, "fast"),
        m(
            "gemini",
            "gemini-3-pro",
            1_250,
            5_000,
            5_000,
            1_000_000,
            "balanced",
        ),
        m(
            "gemini",
            "gemini-3-flash",
            150,
            600,
            5_000,
            1_000_000,
            "fast",
        ),
    ]
}

/// Index the classes of a list by key, for callers that quote many at once.
pub fn class_index(list: &PriceList) -> BTreeMap<&str, &ClassPrice> {
    list.classes
        .iter()
        .map(|c| (c.task_class.as_str(), c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> PriceList {
        seed_provisional(1, "test", 1_700_000_000, seed_models())
    }

    #[test]
    fn a_seeded_list_prices_every_class() {
        // A list that covers some classes and not others quotes some work and
        // says nothing about the rest, and silence is indistinguishable from a
        // decision not to price it.
        let list = seeded();
        for class in TaskClass::all() {
            assert!(list.class(&class).is_some(), "no price for {}", class.key());
        }
    }

    #[test]
    fn nothing_seeded_is_billable() {
        // The graduation gate, and the most important test in this file. A
        // seeded list has zero outcome samples behind it. It publishes a
        // number so the number can be argued with; it must not move money.
        let list = seeded();
        for class in TaskClass::all() {
            let (credits, billable) = quote(&list, &class).expect("priced");
            assert!(credits > 0, "{} quoted zero credits", class.key());
            assert!(
                !billable,
                "{} would charge from a list with no measured outcomes",
                class.key()
            );
        }
    }

    #[test]
    fn a_committed_class_inside_a_provisional_list_still_cannot_bill() {
        // The direction a mistake would go: somebody graduates a class, the
        // list itself is still seeded, and the class starts charging against
        // evidence the list as a whole does not have.
        let mut list = seeded();
        list.classes[0].status = PriceStatus::Committed;
        let class = TaskClass::all()[0];
        let (_, billable) = quote(&list, &class).expect("priced");
        assert!(!billable);
    }

    #[test]
    fn a_committed_class_in_a_committed_list_bills() {
        // The positive direction. Without this the test above passes for a
        // module that can never charge at all, which would be a different bug
        // wearing the same green tick.
        let mut list = seeded();
        list.status = PriceStatus::Committed;
        list.classes[0].status = PriceStatus::Committed;
        let class = TaskClass::all()[0];
        let (credits, billable) = quote(&list, &class).expect("priced");
        assert!(billable);
        assert!(credits > 0);
    }

    #[test]
    fn an_unpriced_class_quotes_nothing_rather_than_guessing() {
        let mut list = seeded();
        let dropped = list.classes.remove(0);
        let class = TaskClass::all()
            .into_iter()
            .find(|c| c.key() == dropped.task_class)
            .unwrap();
        assert!(
            quote(&list, &class).is_none(),
            "an unpriced class produced a price"
        );
    }

    #[test]
    fn a_provisional_quote_never_yields_billable_credits() {
        // The accessor `verification_driver` uses. Reading `quoted_credits`
        // directly would charge for a provisional class, so the guard has to
        // live on the type rather than at the call site.
        let quote = StepQuote {
            quote_id: "q".into(),
            run_id: "r".into(),
            step_id: "s".into(),
            task_class: "modify:low:verifiable".into(),
            quoted_credits: 7,
            price_list_id: "pl".into(),
            price_list_version: 1,
            billable: false,
            frozen_at: 0,
        };
        assert_eq!(quote.billable_credits(), None);

        let billable = StepQuote {
            billable: true,
            ..quote
        };
        assert_eq!(billable.billable_credits(), Some(7));
    }

    #[test]
    fn risk_and_scope_move_the_price_in_the_right_direction() {
        // Not an assertion that the numbers are right — they are modelled and
        // labelled as such. An assertion that the *shape* is right, which is
        // the part a later measured list must also satisfy.
        let list = seeded();
        let cheap = list
            .class(&TaskClass::new(WorkKind::Gate, RiskLevel::Low, true))
            .unwrap()
            .quoted_credits;
        let dear = list
            .class(&TaskClass::new(
                WorkKind::Refactor,
                RiskLevel::Critical,
                true,
            ))
            .unwrap()
            .quoted_credits;
        assert!(
            dear > cheap,
            "a critical refactor is not priced above a low-risk gate: {dear} vs {cheap}"
        );

        let verifiable = list
            .class(&TaskClass::new(WorkKind::Modify, RiskLevel::High, true))
            .unwrap()
            .quoted_credits;
        let unverifiable = list
            .class(&TaskClass::new(WorkKind::Modify, RiskLevel::High, false))
            .unwrap()
            .quoted_credits;
        assert!(
            unverifiable < verifiable,
            "unproven work is priced at or above proven work, which invariant 22 forbids: \
             {unverifiable} vs {verifiable}"
        );
    }

    #[test]
    fn the_price_list_actually_distinguishes_classes() {
        // The failure this caught, and the reason `micros_per_credit` is a
        // published field. The first seeded list valued a credit at one dollar,
        // and every modelled class — a trivial gate and a critical refactor
        // alike — rounded to exactly one credit. Every row still held a
        // plausible number; the list had simply stopped being a price list.
        //
        // Asserted as a property of the whole space rather than of two rows,
        // because two rows is what the previous test checked and it is not
        // enough to notice a collapse.
        let list = seeded();
        let mut distinct: Vec<i64> = list.classes.iter().map(|c| c.quoted_credits).collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert!(
            distinct.len() >= 5,
            "the class space collapsed to {} distinct prices: {distinct:?} — \
             micros_per_credit is too coarse for the modelled spread",
            distinct.len()
        );
    }

    #[test]
    fn no_class_is_ever_free() {
        // A zero-credit class turns a billable verdict into a silent no-op, and
        // a silent no-op in the money path is indistinguishable from the bug
        // this whole module exists to fix.
        let list = seeded();
        assert!(list.classes.iter().all(|c| c.quoted_credits >= 1));
    }

    #[test]
    fn model_cost_is_integer_arithmetic_with_a_cache_discount() {
        let models = seed_models();
        let model = models
            .iter()
            .find(|model| model.model_id == "claude-sonnet-5")
            .unwrap();
        assert_eq!(model.model_id, "claude-sonnet-5");

        // 1k uncached in + 1k out.
        assert_eq!(model.cost_micros(1_000, 0, 1_000), 3_000 + 15_000);

        // The same input, entirely cached, at 1_000bp = 10% of the input rate.
        assert_eq!(model.cost_micros(1_000, 1_000, 0), 300);

        // Cached tokens are not double-counted as uncached.
        assert_eq!(model.cost_micros(2_000, 1_000, 0), 300 + 3_000);
    }

    #[test]
    fn the_seeded_models_carry_no_duplicate_identity() {
        // Invariant 11 is "one fact, one table". Two rows for the same model
        // are two prices for the same fact, and which one applied would depend
        // on iteration order.
        let models = seed_models();
        let mut ids: Vec<String> = models
            .iter()
            .map(|m| format!("{}/{}", m.provider, m.model_id))
            .collect();
        let total = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), total, "a model is priced twice");
    }
}
