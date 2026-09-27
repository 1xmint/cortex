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
///
/// Billing is pass-through: this does not vary by `verdict_class`. The
/// declared class is a stored label surfaced on the plan receipt and the
/// verification receipt, not a pricing input.
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
    // M-D-0022/M-D-0023: pass-through means there is no margin and no
    // rounding up in anything that produces a customer charge or estimate.
    // `margin_bp` is kept at 0, and the field itself is kept on `ClassPrice`
    // only because `db/ledger.rs`'s existing store/load round-trip and the
    // legacy `PriceList`/`ClassPrice`/`quote()` callers (class-based quoting,
    // still used to seed a *provisional*, never-billed estimate row before a
    // class has committed history) still read/write it — not because a
    // margin is charged. This list is always published `Provisional`, and
    // `PriceStatus::may_bill` refuses to charge a provisional class, so these
    // numbers are quotes only, never a ledger amount.
    let margin_bp = 0;

    let classes = TaskClass::all()
        .into_iter()
        .map(|class| {
            let spend = modelled_spend_micros(&class);
            // No rounding up: floor to whole credits. A class whose modelled
            // spend is below one credit quotes 0 rather than being pushed up
            // to 1 — this list never charges (see above), so there is no
            // "free verdict" risk from a zero quote here.
            let credits = spend / SEED_MICROS_PER_CREDIT;
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
             no margin (pass-through, M-D-0022) and no rounding up — floored to whole \
             credits at \
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
        // Anthropic list prices, verified against
        // https://platform.claude.com/docs/en/about-claude/pricing, checked
        // 2026-09-27 (M-D-0023). USD per million tokens, converted to micros
        // per 1k tokens (`dollars_per_million * 1_000`). `cache_read_bp` is
        // basis points of the input rate charged for a cache read (1_000 =
        // the standard 0.1x / 90% discount; see the pinned test below for the
        // two models that discount further).
        //
        // These rows previously carried stale rates that were wrong in both
        // directions: Opus was seeded at $15/$75 (list is $5/$25, a 3x
        // customer overcharge) and Haiku at $0.8/$4 (list is $1/$5, a 20%
        // undercharge — Cortex was losing money on every Haiku call). See
        // `pinned_anthropic_rates_match_the_published_price_list` for the
        // full table this seed must match.
        //
        // Cache *writes* (5-minute 1.25x input, 1-hour 2x input) are not a
        // per-model rate — the multiplier is the same for every model — so
        // there is no field for it here; `cost_micro_usd` (below) applies the
        // fixed multiplier directly against `input_micros_per_1k`.
        m(
            "claude",
            "claude-opus-5-5",
            4_000,
            20_000,
            500,
            200_000,
            "frontier",
        ),
        m(
            "claude",
            "claude-opus-5",
            5_000,
            25_000,
            1_000,
            200_000,
            "frontier",
        ),
        m(
            "claude",
            "claude-opus-4-8",
            5_000,
            25_000,
            1_000,
            200_000,
            "frontier",
        ),
        m(
            "claude",
            "claude-opus-4-7",
            5_000,
            25_000,
            1_000,
            200_000,
            "frontier",
        ),
        m(
            "claude",
            "claude-opus-4-6",
            5_000,
            25_000,
            1_000,
            200_000,
            "frontier",
        ),
        m(
            "claude",
            "claude-opus-4-5",
            5_000,
            25_000,
            1_000,
            200_000,
            "frontier",
        ),
        m(
            "claude",
            "claude-sonnet-5",
            2_000,
            10_000,
            1_000,
            200_000,
            "balanced",
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
            "claude-sonnet-4-5",
            3_000,
            15_000,
            1_000,
            200_000,
            "balanced",
        ),
        m(
            "claude",
            "claude-haiku-4-5",
            1_000,
            5_000,
            1_000,
            200_000,
            "fast",
        ),
        m(
            "claude",
            "claude-haiku-4-5-20251001",
            1_000,
            5_000,
            1_000,
            200_000,
            "fast",
        ),
        // Fable: $10/$50 per million, the most expensive tier on the list.
        // 5.1's cache read discounts further, to 0.025x, than the 0.1x
        // standard (or 5's own, unchanged, rate).
        m(
            "claude",
            "claude-fable-5",
            10_000,
            50_000,
            1_000,
            200_000,
            "frontier",
        ),
        m(
            "claude",
            "claude-fable-5-1",
            10_000,
            50_000,
            250,
            200_000,
            "frontier",
        ),
        // Rates verified against https://developers.openai.com/api/docs/pricing
        // on 2026-09-21 (published per-1M rates divided by 1,000, in micros):
        // gpt-5.5 $5/$30 per 1M, gpt-5.4 $2.50/$15 per 1M, gpt-5-mini $0.25/$2
        // per 1M. The prior rows here were 2x the published input rate for
        // gpt-5.5 and gpt-5.4, and both rates for gpt-5-mini.
        //
        // UNVERIFIED (M-D-0023 left this alone rather than guess): whether
        // OpenAI reasoning tokens are counted inside `output_tokens` by
        // `supplier_openai.rs`'s usage parse. If they are billed separately
        // by OpenAI but folded into `output_tokens` here, this rate
        // undercharges reasoning-heavy calls. Punch-list: re-check
        // `supplier_openai.rs:168-182` against OpenAI's usage object and this
        // page before OpenAI pass-through is trusted at the same "exact"
        // standard as the Anthropic rows below.
        m(
            "openai", "gpt-5.5", 5_000, 30_000, 5_000, 400_000, "frontier",
        ),
        m(
            "openai", "gpt-5.4", 2_500, 15_000, 5_000, 400_000, "balanced",
        ),
        m("openai", "gpt-5-mini", 250, 2_000, 5_000, 400_000, "fast"),
        // Voice rows are priced per SECOND, not per token: "input" is seconds
        // of session, so `cost_micros(seconds, 0, 0)` is the cost. Reusing the
        // token-shaped row avoids a schema change, and input is the one field
        // the gateway already reserves against. `context_window` is the
        // longest session Cortex allows, in seconds.
        // gpt-live-1: $0.05/min billed per second = 833.33 micros/s, rounded
        // up so a minute never costs less than the supplier charges us.
        m("openai", "gpt-live-1", 833_334, 0, 0, 7_200, "voice"),
        // gpt-4o-mini-transcribe: $0.003/min = 50 micros/s.
        m(
            "openai",
            "gpt-4o-mini-transcribe",
            50_000,
            0,
            0,
            7_200,
            "transcribe",
        ),
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
        // Zen was bring-your-own-key only, and the OpenCode Zen BYOK chat
        // feature (the `chat_zen`/`supplier_zen`/`provider_keys` modules) was
        // removed 2026-09-25 by decision — OpenCode's terms only permit use
        // for the customer's own internal use, not on a third party's behalf
        // (see cortex/plan/CREDITS.md and EXECUTION-STATE M-D-0016). The
        // gateway's `KNOWN_PROVIDERS` already excluded `"zen"`, so these rows
        // could never back a Cortex-funded authorization even before that.
        // They stay here, inert, because a published price list is
        // immutable; removing them would be a new price-list version, not an
        // edit to this one.
        //
        // OpenCode Zen (https://opencode.ai/docs/zen, "Pricing" table) and
        // https://opencode.ai/zen/v1/models for exact model ids, both read
        // 2026-09-21. This slice covers only the `/chat/completions` family
        // (DeepSeek, GLM, Kimi, MiniMax); Zen's Claude/GPT/Gemini/Grok
        // aliases are priced under their own provider rows already above and
        // are not reachable through the (now-removed) Zen supplier path.
        // Nothing reads these rows today; a model missing here would have
        // been unable to be reserved against at all (`GatewayError::
        // MissingRate`) back when the Zen supplier path existed.
        //
        // Rates are the published per-1M-token price divided by 1,000 (so
        // $0.14/1M input becomes 140 micros/1k). `cache_read_bp` is the
        // published cached-read price as basis points of the input price.
        // None of these models publishes a context-length breakpoint tier
        // (unlike, on the same page, Claude/Gemini/Grok/GPT rows) so there is
        // no tiered pricing to apply here.
        //
        // `context_window` is not published by Zen for this family; each row
        // uses a deliberately conservative estimate of the underlying open
        // model's known public context length as of 2026-09-21, so the
        // gateway's bound check errs toward rejecting an oversized request
        // rather than admitting one Zen might refuse anyway.
        m(
            "zen",
            "deepseek-v4.1-flash",
            300,
            1_200,
            200,
            128_000,
            "fast",
        ),
        m(
            "zen",
            "deepseek-v4-pro",
            1_740,
            3_480,
            833,
            128_000,
            "frontier",
        ),
        m("zen", "deepseek-v4-flash", 140, 280, 2_000, 128_000, "fast"),
        m(
            "zen",
            "deepseek-v4-flash-vision-exp",
            140,
            280,
            2_000,
            128_000,
            "fast",
        ),
        m("zen", "glm-5.3-flash", 150, 500, 2_000, 128_000, "fast"),
        m("zen", "glm-5.3", 1_400, 4_400, 1_857, 128_000, "balanced"),
        m("zen", "glm-5.2", 1_400, 4_400, 1_857, 128_000, "balanced"),
        m("zen", "glm-5.1", 1_400, 4_400, 1_857, 128_000, "balanced"),
        m("zen", "glm-5", 1_000, 3_200, 2_000, 128_000, "balanced"),
        m("zen", "minimax-m3", 300, 1_200, 2_000, 200_000, "balanced"),
        m(
            "zen",
            "minimax-m2.7",
            300,
            1_200,
            2_000,
            200_000,
            "balanced",
        ),
        m(
            "zen",
            "minimax-m2.5",
            300,
            1_200,
            2_000,
            200_000,
            "balanced",
        ),
        m("zen", "kimi-k3", 3_000, 15_000, 1_000, 256_000, "frontier"),
        m(
            "zen",
            "kimi-k2.7-code",
            950,
            4_000,
            2_000,
            256_000,
            "balanced",
        ),
        m("zen", "kimi-k2.6", 950, 4_000, 1_684, 256_000, "balanced"),
        m("zen", "kimi-k2.5", 600, 3_000, 1_667, 256_000, "balanced"),
    ]
}

/// Index the classes of a list by key, for callers that quote many at once.
pub fn class_index(list: &PriceList) -> BTreeMap<&str, &ClassPrice> {
    list.classes
        .iter()
        .map(|c| (c.task_class.as_str(), c))
        .collect()
}

/// Basis points of `input_micros_per_1k` a 5-minute cache write costs: 1.25x.
pub const CACHE_WRITE_5M_BP: i64 = 12_500;
/// Basis points of `input_micros_per_1k` a 1-hour cache write costs: 2x.
pub const CACHE_WRITE_1H_BP: i64 = 20_000;

/// Every token count one settled model call can carry, split by the rate
/// that applies to it. Deliberately separate from [`ModelPrice::cost_micros`]
/// (which callers outside this PR's scope already call with a 3-argument
/// shape): this is the pass-through-complete accounting, cache writes
/// included, for [`cost_micro_usd`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageTokens {
    /// Total input tokens, including any that were served from cache. A
    /// cache *read* is still an input token for billing purposes — it is
    /// discounted, not free — so this is the full input count, not just the
    /// uncached remainder.
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// Of `input_tokens`, how many were served from a previously written
    /// cache entry (Anthropic's `cache_read_input_tokens`).
    pub cache_read_tokens: i64,
    /// Tokens newly written to a 5-minute cache entry this call
    /// (`cache_creation.ephemeral_5m_input_tokens`, or the whole of
    /// `cache_creation_input_tokens` when the supplier does not split it).
    pub cache_write_5m_tokens: i64,
    /// Tokens newly written to a 1-hour cache entry this call
    /// (`cache_creation.ephemeral_1h_input_tokens`).
    pub cache_write_1h_tokens: i64,
}

/// The exact cost of one settled call, in micro-USD, integer arithmetic end
/// to end (see [`MICROS_PER_USD`]'s reasoning: money that round-trips through
/// an `f64` disagrees with itself, and these numbers get multiplied by token
/// counts in the millions).
///
/// Pure and DB-free by design (M-D-0023): this is the one place pass-through
/// cost math lives, so it can be pinned by a unit test without a database,
/// and so `charge` below can be tested against it without a gateway.
///
/// A cache write is priced as a multiple of the model's own
/// `input_micros_per_1k` (1.25x for a 5-minute write, 2x for a 1-hour write —
/// the same multiplier for every model on the published list, so there is no
/// per-model field for it). A cache read is priced at `cache_read_bp` of the
/// input rate, same as [`ModelPrice::cost_micros`]. Regular (uncached, not a
/// write) input is `input_tokens - cache_read_tokens`, floored at zero so a
/// caller's mismatched counts cannot underflow.
pub fn cost_micro_usd(rate: &ModelPrice, usage: &UsageTokens) -> u64 {
    let regular_in = (usage.input_tokens - usage.cache_read_tokens).max(0);
    let regular = regular_in * rate.input_micros_per_1k / 1_000;
    let cached = usage.cache_read_tokens * rate.input_micros_per_1k * rate.cache_read_bp
        / (1_000 * BP_PER_WHOLE);
    let write_5m = usage.cache_write_5m_tokens * rate.input_micros_per_1k * CACHE_WRITE_5M_BP
        / (1_000 * BP_PER_WHOLE);
    let write_1h = usage.cache_write_1h_tokens * rate.input_micros_per_1k * CACHE_WRITE_1H_BP
        / (1_000 * BP_PER_WHOLE);
    let out = usage.output_tokens * rate.output_micros_per_1k / 1_000;
    let total = regular + cached + write_5m + write_1h + out;
    total.max(0) as u64
}

/// Pass-through charge arithmetic (M-D-0023): deduct exactly the whole
/// credits a settled cost is worth, given whatever fraction of a credit
/// (`carry_micro_usd`) an earlier call could not express as a whole credit,
/// and keep the new remainder for next time. A user's total deduction across
/// any sequence of calls, plus the final carry, always equals the exact sum
/// of costs — never more, and the fraction owed is never dropped in either
/// direction.
///
/// `u128` internally so a legitimate balance and a legitimate single-call
/// cost cannot overflow the intermediate sum; both inputs and both outputs
/// stay `u64`; real balances and real call costs are nowhere near the range
/// where a `u128` sum could itself overflow.
pub fn charge(carry_micro: u64, cost_micro: u64, micros_per_credit: u64) -> (u64, u64) {
    let total = u128::from(carry_micro) + u128::from(cost_micro);
    let mpc = u128::from(micros_per_credit);
    let credits = (total / mpc) as u64;
    let new_carry = (total % mpc) as u64;
    (credits, new_carry)
}

/// Formats micro-USD as a dollar string with exactly 2 decimals, e.g.
/// `1_234_567` micro-USD -> `"$1.23"`. Truncates (does not round) the third
/// decimal and beyond, matching "no rounding up in a customer-facing number".
pub fn micro_usd_to_dollars_display(micro_usd: u64) -> String {
    let cents = micro_usd / 10_000; // 1_000_000 micros per dollar / 100 cents.
    format!("${}.{:02}", cents / 100, cents % 100)
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
    fn seeded_classes_never_bill_so_a_zero_quote_is_honest_not_dangerous() {
        // Pre-M-D-0023 this list rounded every class up to at least 1 credit
        // so a billable verdict could never be a silent no-op. Under
        // pass-through there is no rounding up and no margin (M-D-0022/0023):
        // a class whose modelled spend floors to 0 credits now quotes 0
        // rather than being pushed to 1. That is safe *only* because this
        // list is always published `Provisional`, and `PriceStatus::may_bill`
        // refuses to charge a provisional class — asserted here so the two
        // facts stay tied together.
        let list = seeded();
        assert_eq!(list.status, PriceStatus::Provisional);
        assert!(list.classes.iter().all(|c| c.quoted_credits >= 0));
        assert!(
            list.classes.iter().all(|c| !c.status.may_bill()),
            "a provisional class must never be billable, zero-quote or not"
        );
    }

    #[test]
    fn model_cost_is_integer_arithmetic_with_a_cache_discount() {
        let models = seed_models();
        let model = models
            .iter()
            .find(|model| model.model_id == "claude-sonnet-5")
            .unwrap();
        assert_eq!(model.model_id, "claude-sonnet-5");

        // Sonnet 5 list price: $2/$10 per million = 2_000/10_000 micros per 1k
        // (platform.claude.com/docs/en/about-claude/pricing, 2026-09-27).
        // 1k uncached in + 1k out.
        assert_eq!(model.cost_micros(1_000, 0, 1_000), 2_000 + 10_000);

        // The same input, entirely cached, at 1_000bp = 10% of the input rate.
        assert_eq!(model.cost_micros(1_000, 1_000, 0), 200);

        // Cached tokens are not double-counted as uncached.
        assert_eq!(model.cost_micros(2_000, 1_000, 0), 200 + 2_000);
    }

    #[test]
    fn the_corrected_openai_rows_match_published_pricing() {
        // Verified against https://developers.openai.com/api/docs/pricing on
        // 2026-09-21: gpt-5.5 $5/$30, gpt-5.4 $2.50/$15, gpt-5-mini $0.25/$2,
        // all per 1M tokens (so /1000 for the per-1k micros stored here).
        let models = seed_models();
        let rate = |id: &str| {
            models
                .iter()
                .find(|m| m.provider == "openai" && m.model_id == id)
                .unwrap()
        };

        let gpt_5_5 = rate("gpt-5.5");
        assert_eq!(gpt_5_5.input_micros_per_1k, 5_000);
        assert_eq!(gpt_5_5.output_micros_per_1k, 30_000);

        let gpt_5_4 = rate("gpt-5.4");
        assert_eq!(gpt_5_4.input_micros_per_1k, 2_500);
        assert_eq!(gpt_5_4.output_micros_per_1k, 15_000);

        let gpt_5_mini = rate("gpt-5-mini");
        assert_eq!(gpt_5_mini.input_micros_per_1k, 250);
        assert_eq!(gpt_5_mini.output_micros_per_1k, 2_000);
    }

    #[test]
    fn voice_rows_charge_per_second_at_published_rates() {
        let models = seed_models();
        let rate = |id: &str| {
            models
                .iter()
                .find(|m| m.provider == "openai" && m.model_id == id)
                .unwrap()
        };
        // One minute of gpt-live-1 is $0.05 = 50_000 micros, rounded up by at
        // most one micro.
        let minute = rate("gpt-live-1").cost_micros(60, 0, 0);
        assert!((50_000..=50_001).contains(&minute), "got {minute}");
        assert_eq!(rate("gpt-4o-mini-transcribe").cost_micros(60, 0, 0), 3_000);
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

    /// M-D-0023: pins every Anthropic rate the packet specified against
    /// https://platform.claude.com/docs/en/about-claude/pricing, checked
    /// 2026-09-27. `input_micros_per_1k` / `output_micros_per_1k` are USD per
    /// million tokens * 1_000; `cache_read_bp` is basis points of the input
    /// rate (1_000 = the standard 0.1x discount). This test is meant to go
    /// red the moment a seed row drifts from the published list — it is the
    /// whole point of moving rates out of a `match` arm and into data with a
    /// test that can name its source.
    #[test]
    fn pinned_anthropic_rates_match_the_published_price_list_2026_09_27() {
        // (model_id, input $/1M * 1_000, output $/1M * 1_000, cache_read_bp)
        let table: &[(&str, i64, i64, i64)] = &[
            ("claude-opus-5-5", 4_000, 20_000, 500),
            ("claude-opus-5", 5_000, 25_000, 1_000),
            ("claude-opus-4-8", 5_000, 25_000, 1_000),
            ("claude-opus-4-7", 5_000, 25_000, 1_000),
            ("claude-opus-4-6", 5_000, 25_000, 1_000),
            ("claude-opus-4-5", 5_000, 25_000, 1_000),
            ("claude-sonnet-5", 2_000, 10_000, 1_000),
            ("claude-sonnet-4-6", 3_000, 15_000, 1_000),
            ("claude-sonnet-4-5", 3_000, 15_000, 1_000),
            ("claude-haiku-4-5", 1_000, 5_000, 1_000),
            ("claude-haiku-4-5-20251001", 1_000, 5_000, 1_000),
            ("claude-fable-5", 10_000, 50_000, 1_000),
            ("claude-fable-5-1", 10_000, 50_000, 250),
        ];
        let models = seed_models();
        for (model_id, input, output, cache_read_bp) in table {
            let row = models
                .iter()
                .find(|m| m.provider == "claude" && &m.model_id == model_id)
                .unwrap_or_else(|| panic!("no seed row for {model_id}"));
            assert_eq!(
                row.input_micros_per_1k, *input,
                "{model_id} input rate drifted from the pricing page"
            );
            assert_eq!(
                row.output_micros_per_1k, *output,
                "{model_id} output rate drifted from the pricing page"
            );
            assert_eq!(
                row.cache_read_bp, *cache_read_bp,
                "{model_id} cache-read discount drifted from the pricing page"
            );
        }
    }

    #[test]
    fn cache_write_multipliers_are_1_25x_for_5m_and_2x_for_1h_input() {
        // Not a per-model rate (see the comment on `seed_models`): the
        // multiplier over the model's own input rate is fixed across every
        // model on the published price list.
        let rate = seed_models()
            .into_iter()
            .find(|m| m.model_id == "claude-sonnet-5")
            .unwrap();
        let usage = UsageTokens {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_5m_tokens: 1_000,
            cache_write_1h_tokens: 0,
        };
        // 1k tokens at the 5m write multiplier: 2_000 micros * 1.25.
        assert_eq!(cost_micro_usd(&rate, &usage), 2_500);

        let usage_1h = UsageTokens {
            cache_write_5m_tokens: 0,
            cache_write_1h_tokens: 1_000,
            ..usage
        };
        // 1k tokens at the 1h write multiplier: 2_000 micros * 2.
        assert_eq!(cost_micro_usd(&rate, &usage_1h), 4_000);
    }

    #[test]
    fn cost_micro_usd_sums_every_token_kind_with_no_double_counting() {
        let rate = seed_models()
            .into_iter()
            .find(|m| m.model_id == "claude-sonnet-5")
            .unwrap();
        let usage = UsageTokens {
            input_tokens: 3_000,
            output_tokens: 1_000,
            cache_read_tokens: 1_000,
            cache_write_5m_tokens: 500,
            cache_write_1h_tokens: 200,
        };
        // uncached input: (3_000 - 1_000) tokens * 2_000 micros/1k = 4_000
        // cache read: 1_000 * 2_000 * 0.1 / 1k = 200
        // cache write 5m: 500 * 2_000 * 1.25 / 1k = 1_250
        // cache write 1h: 200 * 2_000 * 2 / 1k = 800
        // output: 1_000 * 10_000 / 1k = 10_000
        let expected = 4_000 + 200 + 1_250 + 800 + 10_000;
        assert_eq!(cost_micro_usd(&rate, &usage), expected as u64);
    }

    #[test]
    fn charge_pins_the_worked_example_from_the_packet() {
        // carry 0, cost 250_000, mpc 100_000 -> 2 credits, carry 50_000.
        assert_eq!(
            charge(0, 250_000, SEED_MICROS_PER_CREDIT as u64),
            (2, 50_000)
        );
        // then cost 60_000 on that carry -> 1 credit, carry 10_000.
        assert_eq!(
            charge(50_000, 60_000, SEED_MICROS_PER_CREDIT as u64),
            (1, 10_000)
        );
    }

    #[test]
    fn charge_never_deducts_more_than_the_exact_sum_across_calls() {
        // Property: summing (credits * mpc) plus the final carry always equals
        // the sum of costs fed in, for any sequence — a user is never charged
        // for more than what the calls actually cost, and the fraction of a
        // credit that could not be deducted is never lost, only carried.
        let mpc = SEED_MICROS_PER_CREDIT as u64;
        let mut carry = 0u64;
        let mut total_cost = 0u64;
        let mut total_credits = 0u64;
        // A small deterministic LCG in place of a `rand` dependency this
        // pure-function test does not need.
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        for _ in 0..1_000 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let cost = state % 1_000_000; // up to ~$1.00 per call
            total_cost += cost;
            let (credits, new_carry) = charge(carry, cost, mpc);
            total_credits += credits;
            carry = new_carry;
        }
        assert_eq!(total_credits * mpc + carry, total_cost);
        assert!(carry < mpc, "carry must always be less than one credit");
    }

    #[test]
    fn charge_is_idempotent_shaped_never_rounds_up() {
        // A cost smaller than the whole carry+cost total's remainder must
        // never be rounded up into an extra credit: pass-through charges
        // exactly, never a cent more.
        let mpc = SEED_MICROS_PER_CREDIT as u64;
        let (credits, carry) = charge(0, 99_999, mpc);
        assert_eq!((credits, carry), (0, 99_999));
    }

    #[test]
    fn micro_usd_display_formats_two_decimals_of_a_dollar() {
        assert_eq!(micro_usd_to_dollars_display(1_234_567), "$1.23");
        assert_eq!(micro_usd_to_dollars_display(0), "$0.00");
        assert_eq!(micro_usd_to_dollars_display(1_000_000), "$1.00");
        assert_eq!(micro_usd_to_dollars_display(10_000), "$0.01");
        // Truncates, does not round: just under a cent stays at zero.
        assert_eq!(micro_usd_to_dollars_display(9_999), "$0.00");
    }
}
