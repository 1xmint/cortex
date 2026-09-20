use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{FromRef, FromRequestParts, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use cortex_core::usage::UsageLimits;
use serde::{Deserialize, Serialize};

use crate::clerk::ClerkUser;
use crate::db::Database;
use crate::routes::ErrorResponse;
use crate::state::AppState;
use crate::stripe_client::StripeClient;

// ─── PremiumUser extractor ────────────────────────────────────────────────────

/// An Axum extractor that succeeds only when the authenticated user has an active
/// premium (paid) subscription. Returns 403 otherwise.
///
/// Usage: add `_user: PremiumUser` as a handler parameter to gate a route.
#[derive(Debug, Clone)]
pub struct PremiumUser {
    pub user_id: String,
}

impl<S> FromRequestParts<S> for PremiumUser
where
    Arc<AppState>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<ErrorResponse>);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state: Arc<AppState> = Arc::from_ref(state);

        // First, extract the authenticated user via ClerkUser
        let clerk_user = ClerkUser::from_request_parts(parts, state).await?;

        // Dev/local mode: no Clerk configured, allow through
        if app_state.clerk_secret_key.is_none() {
            return Ok(PremiumUser {
                user_id: clerk_user.user_id,
            });
        }

        // Admin bypass
        if crate::admin::authorize_admin(&app_state, &clerk_user)
            .await
            .is_ok()
        {
            return Ok(PremiumUser {
                user_id: clerk_user.user_id,
            });
        }

        // Check premium status using same logic as is_premium()
        let is_premium = app_state
            .db
            .as_ref()
            .and_then(|db| db.get_subscription(&clerk_user.user_id))
            .map(|sub| matches!(sub.status.as_str(), "active" | "trialing"))
            .unwrap_or(false);

        if is_premium {
            Ok(PremiumUser {
                user_id: clerk_user.user_id,
            })
        } else {
            Err((
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    error: "active Cortex subscription required".into(),
                }),
            ))
        }
    }
}

/// Billing gate errors. Each variant carries current and limit values for diagnostics.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BillingError {
    DailyCostLimitExceeded { current: f64, limit: f64 },
    DailyStepLimitExceeded { current: i64, limit: i64 },
    MonthlyCostLimitExceeded { current: f64, limit: f64 },
}

impl std::fmt::Display for BillingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DailyCostLimitExceeded { current, limit } => {
                write!(f, "daily cost limit exceeded: ${current:.2} / ${limit:.2}")
            }
            Self::DailyStepLimitExceeded { current, limit } => {
                write!(f, "daily step limit exceeded: {current} / {limit}")
            }
            Self::MonthlyCostLimitExceeded { current, limit } => {
                write!(
                    f,
                    "monthly cost limit exceeded: ${current:.2} / ${limit:.2}"
                )
            }
        }
    }
}

/// Result of a billing gate check.
#[derive(Debug, Clone, Serialize)]
pub struct GateResult {
    /// Whether the gate passed (user is within limits).
    pub allowed: bool,
    /// If not allowed, the specific violation.
    pub violation: Option<BillingError>,
    /// Whether enforcement is active (if false, violations are warnings only).
    pub enforced: bool,
    /// Current daily cost for observability.
    pub daily_cost: f64,
    /// Current daily step count for observability.
    pub daily_steps: i64,
    /// Current monthly cost for observability.
    pub monthly_cost: f64,
}

/// Check whether a user is within usage limits.
///
/// When `enforce` is false (default for v1), violations are logged as warnings
/// but the gate returns `allowed: true` so work continues.
pub fn check_usage_gate(
    db: &Database,
    user_id: &str,
    limits: &UsageLimits,
    enforce: bool,
) -> GateResult {
    let (daily_cost, daily_steps) = db.get_user_daily_cost(user_id);
    let monthly_cost = db.get_user_monthly_cost(user_id);

    // Check daily cost
    if daily_cost > limits.daily_cost_limit {
        let violation = BillingError::DailyCostLimitExceeded {
            current: daily_cost,
            limit: limits.daily_cost_limit,
        };
        tracing::warn!(
            "billing gate: user {user_id} — {}{}",
            violation,
            if enforce {
                " [BLOCKED]"
            } else {
                " [warn-only]"
            }
        );
        return GateResult {
            allowed: !enforce,
            violation: Some(violation),
            enforced: enforce,
            daily_cost,
            daily_steps,
            monthly_cost,
        };
    }

    // Check daily step count
    if daily_steps > limits.daily_step_limit {
        let violation = BillingError::DailyStepLimitExceeded {
            current: daily_steps,
            limit: limits.daily_step_limit,
        };
        tracing::warn!(
            "billing gate: user {user_id} — {}{}",
            violation,
            if enforce {
                " [BLOCKED]"
            } else {
                " [warn-only]"
            }
        );
        return GateResult {
            allowed: !enforce,
            violation: Some(violation),
            enforced: enforce,
            daily_cost,
            daily_steps,
            monthly_cost,
        };
    }

    // Check monthly cost
    if monthly_cost > limits.monthly_cost_limit {
        let violation = BillingError::MonthlyCostLimitExceeded {
            current: monthly_cost,
            limit: limits.monthly_cost_limit,
        };
        tracing::warn!(
            "billing gate: user {user_id} — {}{}",
            violation,
            if enforce {
                " [BLOCKED]"
            } else {
                " [warn-only]"
            }
        );
        return GateResult {
            allowed: !enforce,
            violation: Some(violation),
            enforced: enforce,
            daily_cost,
            daily_steps,
            monthly_cost,
        };
    }

    GateResult {
        allowed: true,
        violation: None,
        enforced: enforce,
        daily_cost,
        daily_steps,
        monthly_cost,
    }
}

fn free_tier_usage_limits() -> UsageLimits {
    UsageLimits {
        daily_cost_limit: std::env::var("CORTEX_FREE_DAILY_COST_LIMIT")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(2.0),
        daily_step_limit: std::env::var("CORTEX_FREE_DAILY_STEP_LIMIT")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(20),
        monthly_cost_limit: std::env::var("CORTEX_FREE_MONTHLY_COST_LIMIT")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(20.0),
    }
}

fn free_tier_chat_blocked(state: &AppState, db: &Database, user_id: &str) -> bool {
    let limits = free_tier_usage_limits();
    let gate = check_usage_gate(db, user_id, &limits, state.billing_enforced);
    if !gate.allowed {
        tracing::warn!(
            "free tier billing gate blocked chat for user {user_id}: {:?}",
            gate.violation
        );
    }
    !gate.allowed
}

// ============================================================================
// Subscription & Access Gate
// ============================================================================

/// The frontend's single source of truth. Render UI based on `access_state`.
#[derive(Debug, Clone, Serialize)]
pub struct SubscriptionStatus {
    pub access_state: AccessState,
    /// True only for `active` | `trial_active` (same rules as `/api/billing/usage`).
    pub active: bool,
    pub plan: Option<PlanInfo>,
    pub trial: Option<TrialInfo>,
    pub delegation: DelegationStatus,
    pub payment_method: Option<PaymentMethodInfo>,
    pub referral: Option<ReferralInfo>,
}

/// Pure: paid or trial access matches usage rules.
pub fn is_premium_access(access_state: &AccessState) -> bool {
    matches!(access_state, AccessState::Active | AccessState::TrialActive)
}

/// Exact state machine. Frontend switches on this — no guessing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AccessState {
    SignedOut,
    NeedsPhone,
    NeedsCheckout,
    TrialActive,
    Active,
    PaymentFailed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanInfo {
    pub plan_type: PlanType,
    pub status: SubStatus,
    pub billing_period_end: String,
    pub next_charge_amount_cents: Option<i64>,
    pub next_charge_date: Option<String>,
    pub started_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PlanType {
    Monthly,
    Annual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SubStatus {
    Trialing,
    Active,
    PastDue,
    Cancelled,
    Paused,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrialInfo {
    pub trial_end: String,
    pub days_remaining: i64,
    pub auto_charge_amount_cents: i64,
    pub auto_charge_plan: PlanType,
}

#[derive(Debug, Clone, Serialize)]
pub struct DelegationStatus {
    pub status: DelegationState,
    pub budget_enforced: bool,
    pub delegation_id: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DelegationState {
    Active,
    Pending,
    Expired,
    Revoked,
    NotIssued,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaymentMethodInfo {
    pub last4: String,
    pub brand: String,
    pub exp_month: u32,
    pub exp_year: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferralInfo {
    pub code: String,
    pub uses_remaining: u32,
    pub total_uses: u32,
    pub weeks_earned: u32,
}

/// 402 Payment Required response body. Frontend handles this specially.
#[derive(Serialize)]
pub struct BillingRequiredError {
    pub error: String,
    pub code: BillingRejection,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingRejection {
    TrialExpired,
    SubscriptionInactive,
    PaymentRequired,
}

/// Check whether a user has an active premium subscription.
/// Returns true when the subscription status is "active" or "trialing".
pub fn is_premium(state: &AppState, user_id: &str) -> bool {
    let db = match state.db.as_ref() {
        Some(db) => db,
        None => return false,
    };
    match db.get_subscription(user_id) {
        Some(s) => matches!(s.status.as_str(), "active" | "trialing"),
        None => false,
    }
}

/// Quick check: can this user send chat messages?
/// Returns None if allowed, Some(AccessState) if blocked.
pub fn check_chat_access(state: &AppState, user_id: &str) -> Option<AccessState> {
    state.clerk_secret_key.as_ref()?;
    let db = state.db.as_ref()?;
    let sub = db.get_subscription(user_id);
    match sub {
        Some(s) => match s.status.as_str() {
            "active" | "trialing" => None,
            "past_due" => Some(AccessState::PaymentFailed),
            "cancelled" | "canceled" => {
                if free_tier_chat_blocked(state, db, user_id) {
                    Some(AccessState::NeedsCheckout)
                } else {
                    None
                }
            }
            _ => {
                if free_tier_chat_blocked(state, db, user_id) {
                    Some(AccessState::NeedsCheckout)
                } else {
                    None
                }
            }
        },
        None => {
            if free_tier_chat_blocked(state, db, user_id) {
                Some(AccessState::NeedsCheckout)
            } else {
                None
            }
        }
    }
}

// --- Helpers ---

fn build_delegation_status(state: &AppState) -> DelegationStatus {
    #[cfg(feature = "soma")]
    if state.soma_heart.is_some() {
        return DelegationStatus {
            status: DelegationState::Active,
            budget_enforced: true,
            delegation_id: None,
            expires_at: None,
        };
    }

    // No heart — and with the `soma` feature off there is no heart to have.
    // Reporting `NotIssued` with `budget_enforced: false` is the truthful
    // answer in both configurations: nothing is enforcing a delegation budget.
    let _ = state;
    DelegationStatus {
        status: DelegationState::NotIssued,
        budget_enforced: false,
        delegation_id: None,
        expires_at: None,
    }
}

/// No-DB / unavailable status — never invent Active/premium.
fn stub_billing_status(state: &AppState) -> SubscriptionStatus {
    SubscriptionStatus {
        access_state: AccessState::NeedsCheckout,
        active: false,
        plan: None,
        trial: None,
        delegation: build_delegation_status(state),
        payment_method: None,
        referral: None,
    }
}

// --- Endpoints ---

/// GET /api/billing/status — The single access gate endpoint.
pub async fn get_billing_status(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Json<SubscriptionStatus> {
    let delegation = build_delegation_status(&state);
    let db = match &state.db {
        Some(db) => db,
        None => return Json(stub_billing_status(&state)),
    };

    let sub = db.get_subscription(&user.user_id);

    let (access_state, plan, trial) = match &sub {
        Some(s) => {
            let plan_type = if s.plan_type == "annual" {
                PlanType::Annual
            } else {
                PlanType::Monthly
            };
            let sub_status = match s.status.as_str() {
                "trialing" => SubStatus::Trialing,
                "active" => SubStatus::Active,
                "past_due" => SubStatus::PastDue,
                "cancelled" | "canceled" => SubStatus::Cancelled,
                "paused" => SubStatus::Paused,
                _ => SubStatus::Active,
            };

            let access = match s.status.as_str() {
                "trialing" => AccessState::TrialActive,
                "active" => AccessState::Active,
                "past_due" => AccessState::PaymentFailed,
                "cancelled" | "canceled" => AccessState::Cancelled,
                _ => AccessState::Active,
            };

            let trial_info = if s.status == "trialing" {
                s.trial_end.as_ref().map(|end| {
                    let days = chrono::NaiveDateTime::parse_from_str(end, "%Y-%m-%d %H:%M:%S")
                        .or_else(|_| {
                            chrono::DateTime::parse_from_rfc3339(end).map(|d| d.naive_utc())
                        })
                        .map(|t| {
                            let now = chrono::Utc::now().naive_utc();
                            (t - now).num_days().max(0)
                        })
                        .unwrap_or(0);
                    TrialInfo {
                        trial_end: end.clone(),
                        days_remaining: days,
                        auto_charge_amount_cents: if plan_type == PlanType::Annual {
                            6900
                        } else {
                            699
                        },
                        auto_charge_plan: plan_type.clone(),
                    }
                })
            } else {
                None
            };

            let plan_info = PlanInfo {
                plan_type: plan_type.clone(),
                status: sub_status,
                billing_period_end: s.current_period_end.clone().unwrap_or_default(),
                next_charge_amount_cents: Some(if plan_type == PlanType::Annual {
                    6900
                } else {
                    699
                }),
                next_charge_date: s.current_period_end.clone(),
                started_at: s.current_period_start.clone().unwrap_or_default(),
            };

            (access, Some(plan_info), trial_info)
        }
        None => (AccessState::NeedsCheckout, None, None),
    };

    let referral = if sub.is_some() {
        let ref_code = db.create_user_referral_code(&user.user_id);
        Some(ReferralInfo {
            code: ref_code.code,
            uses_remaining: ref_code.uses_remaining as u32,
            total_uses: ref_code.total_uses as u32,
            weeks_earned: ref_code.weeks_earned as u32,
        })
    } else {
        None
    };

    Json(SubscriptionStatus {
        access_state: access_state.clone(),
        active: is_premium_access(&access_state),
        plan,
        trial,
        delegation,
        payment_method: None,
        referral,
    })
}

/// POST /api/billing/checkout — Create Stripe Checkout session.
pub async fn create_checkout(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<CheckoutRequest>,
) -> Result<Json<CheckoutResponse>, (StatusCode, Json<ErrorResponse>)> {
    let stripe = require_stripe(&state)?;

    let db = state.db.as_ref().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "database unavailable".into(),
        }),
    ))?;

    let mut extra_trial_days: i64 = 0;
    let mut coupon_percent_off: Option<f64> = None;

    if let (Some(code), Some(choice_idx_str)) = (&req.referral_code, &req.referral_choice) {
        let promo = db.get_promo_code(code).ok_or((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid promo code".into(),
            }),
        ))?;
        if !promo.active {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "promo code is no longer active".into(),
                }),
            ));
        }
        if promo.current_uses >= promo.max_uses {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "promo code has been fully redeemed".into(),
                }),
            ));
        }

        let parsed_options: Vec<serde_json::Value> = promo
            .discount_options
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        let choice_idx: usize = choice_idx_str.parse().unwrap_or(0);

        let (dtype, dvalue) = if !parsed_options.is_empty() {
            let opt = parsed_options.get(choice_idx).ok_or((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "invalid discount option".into(),
                }),
            ))?;
            (
                opt.get("discount_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                opt.get("discount_value")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
            )
        } else {
            (promo.discount_type.clone(), promo.discount_value)
        };

        match dtype.as_str() {
            "percent_off" => {
                if req.plan != PlanType::Annual {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: "percent-off discount requires the annual plan".into(),
                        }),
                    ));
                }
                coupon_percent_off = Some(dvalue);
            }
            "trial_extension" | "free_trial" => {
                extra_trial_days = dvalue as i64;
            }
            _ => {}
        }
    }

    let customer_id = match db.get_subscription(&user.user_id) {
        Some(sub) => sub.stripe_customer_id,
        None => {
            let email = req.email.as_deref().unwrap_or(&user.user_id);
            let customer = stripe
                .create_customer(email, &user.user_id)
                .await
                .map_err(|e| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(ErrorResponse {
                            error: format!("failed to create Stripe customer: {e}"),
                        }),
                    )
                })?;
            customer.id
        }
    };

    let price_id = match req.plan {
        PlanType::Monthly => &stripe.price_monthly,
        PlanType::Annual => &stripe.price_annual,
    };

    let has_had_trial = db.get_subscription(&user.user_id).is_some();
    let base_trial = if has_had_trial { 0 } else { 7 };
    let total_trial = base_trial + extra_trial_days;
    let trial_days = if total_trial > 0 {
        Some(total_trial as u32)
    } else {
        None
    };
    let _ = coupon_percent_off;

    let session = stripe
        .create_checkout_session(
            &customer_id,
            price_id,
            trial_days,
            None,
            Some(&user.user_id),
        )
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("failed to create checkout: {e}"),
                }),
            )
        })?;

    Ok(Json(CheckoutResponse {
        checkout_url: session.url.unwrap_or_default(),
        session_id: session.id,
    }))
}

#[derive(Deserialize, Serialize)]
pub struct CheckoutRequest {
    pub plan: PlanType,
    pub email: Option<String>,
    pub referral_code: Option<String>,
    pub referral_choice: Option<String>,
}

#[derive(Serialize)]
pub struct CheckoutResponse {
    pub checkout_url: String,
    pub session_id: String,
}

/// POST /api/billing/portal — Stripe Customer Portal.
pub async fn create_portal(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Result<Json<PortalResponse>, (StatusCode, Json<ErrorResponse>)> {
    let stripe = require_stripe(&state)?;
    let db = state.db.as_ref().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "database unavailable".into(),
        }),
    ))?;

    let sub = db.get_subscription(&user.user_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error: "no subscription found".into(),
        }),
    ))?;

    let portal = stripe
        .create_portal_session(&sub.stripe_customer_id, &stripe.success_url)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("failed to create portal: {e}"),
                }),
            )
        })?;

    Ok(Json(PortalResponse {
        portal_url: portal.url,
    }))
}

#[derive(Serialize)]
pub struct PortalResponse {
    pub portal_url: String,
}

/// POST /api/billing/referral/validate — Check promo code validity.
pub async fn validate_referral(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<ReferralValidateRequest>,
) -> Json<ReferralValidateResponse> {
    let db = match &state.db {
        Some(db) => db,
        None => {
            return Json(ReferralValidateResponse {
                valid: false,
                discount_type: None,
                discount_value: None,
                description: None,
                options: vec![],
                uses_remaining: None,
                error: Some("database unavailable".into()),
            })
        }
    };

    match db.validate_promo_code(&req.code, &user.user_id) {
        Ok(promo) => {
            let parsed_options: Vec<serde_json::Value> = promo
                .discount_options
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();

            let options: Vec<serde_json::Value> = if parsed_options.is_empty() {
                vec![serde_json::json!({
                    "label": match promo.discount_type.as_str() {
                        "percent_off" => format!("{}% off annual", promo.discount_value),
                        "trial_extension" => format!("{} extra free days", promo.discount_value),
                        "free_trial" => format!("{}-day free trial", promo.discount_value),
                        _ => "Discount applied".to_string(),
                    },
                    "discount_type": promo.discount_type,
                    "discount_value": promo.discount_value,
                })]
            } else {
                parsed_options
            };

            Json(ReferralValidateResponse {
                valid: true,
                discount_type: Some(promo.discount_type),
                discount_value: Some(promo.discount_value),
                description: promo.description,
                options,
                uses_remaining: Some((promo.max_uses - promo.current_uses) as u32),
                error: None,
            })
        }
        Err(e) => Json(ReferralValidateResponse {
            valid: false,
            discount_type: None,
            discount_value: None,
            description: None,
            options: vec![],
            uses_remaining: None,
            error: Some(e),
        }),
    }
}

#[derive(Deserialize, Serialize)]
pub struct ReferralValidateRequest {
    pub code: String,
}

#[derive(Serialize, Deserialize)]
pub struct ReferralValidateResponse {
    pub valid: bool,
    pub discount_type: Option<String>,
    pub discount_value: Option<f64>,
    pub description: Option<String>,
    pub options: Vec<serde_json::Value>,
    pub uses_remaining: Option<u32>,
    pub error: Option<String>,
}

/// Default page size for billing history (capped separately).
pub const BILLING_HISTORY_DEFAULT_LIMIT: i64 = 20;
/// Hard max page size for billing history.
pub const BILLING_HISTORY_MAX_LIMIT: i64 = 50;

/// Pure: clamp limit query to [1, MAX], default DEFAULT.
pub fn clamp_history_limit(limit: Option<i64>) -> i64 {
    match limit {
        None => BILLING_HISTORY_DEFAULT_LIMIT,
        Some(n) if n < 1 => 1,
        Some(n) if n > BILLING_HISTORY_MAX_LIMIT => BILLING_HISTORY_MAX_LIMIT,
        Some(n) => n,
    }
}

/// Pure: clamp offset query to >= 0, default 0.
pub fn clamp_history_offset(offset: Option<i64>) -> i64 {
    match offset {
        None => 0,
        Some(n) if n < 0 => 0,
        Some(n) => n,
    }
}

/// Pure: given fetch of `limit + 1` rows, return page size + has_more.
pub fn history_page_meta(fetched_count: usize, limit: i64) -> (usize, bool) {
    let limit_usize = limit.max(0) as usize;
    if fetched_count > limit_usize {
        (limit_usize, true)
    } else {
        (fetched_count, false)
    }
}

#[derive(Debug, Deserialize)]
pub struct BillingHistoryQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub struct BillingHistoryEntry {
    pub date: String,
    pub amount_cents: i64,
    pub description: String,
    pub status: String,
}

/// Paginated purchase history page.
#[derive(Serialize, Deserialize)]
pub struct BillingHistoryPage {
    pub items: Vec<BillingHistoryEntry>,
    #[serde(rename = "hasMore")]
    pub has_more: bool,
    #[serde(rename = "nextOffset")]
    pub next_offset: Option<i64>,
    pub limit: i64,
    pub offset: i64,
}

/// GET /api/billing/history — Purchase history with limit/offset pagination.
/// Default limit 20, max 50. Returns items + hasMore + nextOffset.
pub async fn get_billing_history(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Query(query): Query<BillingHistoryQuery>,
) -> Json<BillingHistoryPage> {
    let limit = clamp_history_limit(query.limit);
    let offset = clamp_history_offset(query.offset);

    let raw = state
        .db
        .as_ref()
        .map(|db| {
            // Fetch one extra row to detect hasMore without a separate count query.
            db.get_billing_history(&user.user_id, limit + 1, offset)
                .into_iter()
                .map(|r| BillingHistoryEntry {
                    date: r.created_at,
                    amount_cents: r.amount_cents,
                    description: r.description,
                    status: r.status,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let (take, has_more) = history_page_meta(raw.len(), limit);
    let items: Vec<BillingHistoryEntry> = raw.into_iter().take(take).collect();
    let next_offset = if has_more { Some(offset + limit) } else { None };

    Json(BillingHistoryPage {
        items,
        has_more,
        next_offset,
        limit,
        offset,
    })
}

/// Usage window aggregates from `usage_events` (honest zeros when empty).
#[derive(Serialize)]
pub struct BillingUsageWindow {
    pub total_tokens_in: i64,
    pub total_tokens_out: i64,
    pub total_cost_estimate: f64,
    pub step_count: i64,
}

/// Nested usage summary for the Premium usage/credits ledger MVP.
#[derive(Serialize)]
pub struct BillingUsageSummary {
    pub last_24h: BillingUsageWindow,
    pub last_30d: BillingUsageWindow,
    pub daily_cost: f64,
    pub daily_steps: i64,
    pub monthly_cost: f64,
}

/// GET /api/billing/usage — subscription access + usage summary + billing history.
///
/// Credits balance is `null` when no ledger row exists (unmetered free path).
/// When a row exists, balance is subscription_remaining + pack_remaining.
/// Never call inventing `get_credit_balance` from this product path.
#[derive(Serialize)]
pub struct BillingUsageResponse {
    pub active: bool,
    pub access_state: AccessState,
    pub plan: Option<PlanInfo>,
    /// Real ledger total when a balance row exists; `null` when unmetered.
    /// Whole credits — JSON-wise `200` rather than `200.0`, which parses
    /// identically in JS, so this is not a breaking change for the frontend.
    #[serde(rename = "creditsBalance")]
    pub credits_balance: Option<i64>,
    pub usage: BillingUsageSummary,
    pub history: Vec<BillingHistoryEntry>,
    pub note: String,
}

/// Pure helper: map optional ledger row → API `creditsBalance`.
/// `None` row → `None` (never invent). `Some` → sub + pack remaining.
pub fn credit_balance_for_api(row: Option<&crate::db::CreditBalanceRecord>) -> Option<i64> {
    row.map(|r| r.subscription_remaining + r.pack_remaining)
}

/// Default subscription allotment when a paid subscription checkout completes.
pub const DEFAULT_SUBSCRIPTION_CREDITS: i64 = 200;

/// Pure decision: whether checkout.session.completed should init a credit ledger row.
/// Only subscription mode (not one-time payment packs).
pub fn should_init_credits_on_checkout(mode: &str) -> bool {
    mode.eq_ignore_ascii_case("subscription")
}

/// Honest note for billing/usage responses.
pub fn billing_usage_note(has_metered_balance: bool) -> &'static str {
    if has_metered_balance {
        "metered"
    } else {
        "ledger balance not metered yet"
    }
}

fn usage_window_from_summary(summary: &cortex_core::usage::UsageSummary) -> BillingUsageWindow {
    BillingUsageWindow {
        total_tokens_in: summary.total_tokens_in,
        total_tokens_out: summary.total_tokens_out,
        total_cost_estimate: summary.total_cost_estimate,
        step_count: summary.step_count,
    }
}

/// GET /api/billing/usage — Clerk-authenticated usage + billing ledger MVP.
pub async fn get_billing_usage(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Json<BillingUsageResponse> {
    let empty_usage = BillingUsageSummary {
        last_24h: BillingUsageWindow {
            total_tokens_in: 0,
            total_tokens_out: 0,
            total_cost_estimate: 0.0,
            step_count: 0,
        },
        last_30d: BillingUsageWindow {
            total_tokens_in: 0,
            total_tokens_out: 0,
            total_cost_estimate: 0.0,
            step_count: 0,
        },
        daily_cost: 0.0,
        daily_steps: 0,
        monthly_cost: 0.0,
    };

    let db = match &state.db {
        Some(db) => db,
        None => {
            return Json(BillingUsageResponse {
                active: false,
                access_state: AccessState::NeedsCheckout,
                plan: None,
                credits_balance: None,
                usage: empty_usage,
                history: vec![],
                note: billing_usage_note(false).into(),
            });
        }
    };

    // Reuse the same access/plan derivation as GET /api/billing/status (no stub active).
    let sub = db.get_subscription(&user.user_id);
    let (access_state, plan) = match &sub {
        Some(s) => {
            let plan_type = if s.plan_type == "annual" {
                PlanType::Annual
            } else {
                PlanType::Monthly
            };
            let sub_status = match s.status.as_str() {
                "trialing" => SubStatus::Trialing,
                "active" => SubStatus::Active,
                "past_due" => SubStatus::PastDue,
                "cancelled" | "canceled" => SubStatus::Cancelled,
                "paused" => SubStatus::Paused,
                _ => SubStatus::Active,
            };
            let access = match s.status.as_str() {
                "trialing" => AccessState::TrialActive,
                "active" => AccessState::Active,
                "past_due" => AccessState::PaymentFailed,
                "cancelled" | "canceled" => AccessState::Cancelled,
                _ => AccessState::Active,
            };
            let plan_info = PlanInfo {
                plan_type: plan_type.clone(),
                status: sub_status,
                billing_period_end: s.current_period_end.clone().unwrap_or_default(),
                next_charge_amount_cents: Some(if plan_type == PlanType::Annual {
                    6900
                } else {
                    699
                }),
                next_charge_date: s.current_period_end.clone(),
                started_at: s.current_period_start.clone().unwrap_or_default(),
            };
            (access, Some(plan_info))
        }
        None => (AccessState::NeedsCheckout, None),
    };

    let active = is_premium_access(&access_state);

    let now_ms = chrono::Utc::now().timestamp_millis();
    let last_24h_ms = now_ms - 86_400_000;
    let last_30d_ms = now_ms - (30 * 86_400_000);
    let summary_24h = db.get_user_usage_summary(&user.user_id, last_24h_ms);
    let summary_30d = db.get_user_usage_summary(&user.user_id, last_30d_ms);
    let (daily_cost, daily_steps) = db.get_user_daily_cost(&user.user_id);
    let monthly_cost = db.get_user_monthly_cost(&user.user_id);

    let usage = BillingUsageSummary {
        last_24h: usage_window_from_summary(&summary_24h),
        last_30d: usage_window_from_summary(&summary_30d),
        daily_cost,
        daily_steps,
        monthly_cost,
    };

    // Usage embeds a first page only; full pagination is GET /api/billing/history.
    let history = db
        .get_billing_history(&user.user_id, BILLING_HISTORY_DEFAULT_LIMIT, 0)
        .into_iter()
        .map(|r| BillingHistoryEntry {
            date: r.created_at,
            amount_cents: r.amount_cents,
            description: r.description,
            status: r.status,
        })
        .collect();

    // Honest balance: only from a real ledger row — never invent 200.
    let balance_row = db.get_credit_balance_row(&user.user_id);
    let credits_balance = credit_balance_for_api(balance_row.as_ref());
    let note = billing_usage_note(credits_balance.is_some());

    Json(BillingUsageResponse {
        active,
        access_state,
        plan,
        credits_balance,
        usage,
        history,
        note: note.into(),
    })
}

#[cfg(test)]
mod credit_balance_api_tests {
    use super::{
        billing_usage_note, credit_balance_for_api, should_init_credits_on_checkout,
        DEFAULT_SUBSCRIPTION_CREDITS,
    };

    #[test]
    fn should_init_credits_only_for_subscription_mode() {
        assert!(should_init_credits_on_checkout("subscription"));
        assert!(should_init_credits_on_checkout("Subscription"));
        assert!(!should_init_credits_on_checkout("payment"));
        assert!(!should_init_credits_on_checkout(""));
        assert_eq!(DEFAULT_SUBSCRIPTION_CREDITS, 200);
    }
    use crate::db::CreditBalanceRecord;

    #[test]
    fn credit_balance_for_api_none_is_null() {
        assert_eq!(credit_balance_for_api(None), None);
        assert_eq!(billing_usage_note(false), "ledger balance not metered yet");
    }

    #[test]
    fn credit_balance_for_api_sums_sub_and_pack() {
        let row = CreditBalanceRecord {
            subscription_remaining: 12,
            subscription_total: 200,
            pack_remaining: 8,
        };
        assert_eq!(credit_balance_for_api(Some(&row)), Some(20));
        assert_eq!(billing_usage_note(true), "metered");
    }

    #[test]
    fn credit_balance_for_api_zero_row_is_zero_not_null() {
        let row = CreditBalanceRecord {
            subscription_remaining: 0,
            subscription_total: 200,
            pack_remaining: 0,
        };
        // Zero is a real metered balance — not "unmetered".
        assert_eq!(credit_balance_for_api(Some(&row)), Some(0));
    }
}

#[cfg(test)]
mod history_pagination_tests {
    use super::{
        clamp_history_limit, clamp_history_offset, history_page_meta, is_premium_access,
        AccessState, BILLING_HISTORY_DEFAULT_LIMIT, BILLING_HISTORY_MAX_LIMIT,
    };

    #[test]
    fn clamp_history_limit_defaults_and_caps() {
        assert_eq!(clamp_history_limit(None), BILLING_HISTORY_DEFAULT_LIMIT);
        assert_eq!(clamp_history_limit(Some(0)), 1);
        assert_eq!(clamp_history_limit(Some(-5)), 1);
        assert_eq!(clamp_history_limit(Some(20)), 20);
        assert_eq!(clamp_history_limit(Some(50)), 50);
        assert_eq!(clamp_history_limit(Some(100)), BILLING_HISTORY_MAX_LIMIT);
    }

    #[test]
    fn clamp_history_offset_non_negative() {
        assert_eq!(clamp_history_offset(None), 0);
        assert_eq!(clamp_history_offset(Some(-1)), 0);
        assert_eq!(clamp_history_offset(Some(40)), 40);
    }

    #[test]
    fn history_page_meta_detects_has_more_from_extra_row() {
        assert_eq!(history_page_meta(21, 20), (20, true));
        assert_eq!(history_page_meta(20, 20), (20, false));
        assert_eq!(history_page_meta(0, 20), (0, false));
        assert_eq!(history_page_meta(5, 20), (5, false));
    }

    #[test]
    fn is_premium_access_only_active_or_trial() {
        assert!(is_premium_access(&AccessState::Active));
        assert!(is_premium_access(&AccessState::TrialActive));
        assert!(!is_premium_access(&AccessState::NeedsCheckout));
        assert!(!is_premium_access(&AccessState::PaymentFailed));
        assert!(!is_premium_access(&AccessState::Cancelled));
        assert!(!is_premium_access(&AccessState::SignedOut));
        assert!(!is_premium_access(&AccessState::NeedsPhone));
    }
}

// --- Stripe Webhook ---

/// POST /api/stripe/webhook — Stripe event ingestion.
/// No ClerkUser auth — verified by Stripe webhook signature.
pub async fn stripe_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let webhook_secret = state.stripe_webhook_secret.as_deref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            error: "webhook not configured".into(),
        }),
    ))?;

    let sig = headers
        .get("stripe-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "missing Stripe-Signature header".into(),
            }),
        ))?;

    StripeClient::verify_webhook_signature(&body, sig, webhook_secret).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("signature verification failed: {e}"),
            }),
        )
    })?;

    let event: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("invalid JSON: {e}"),
            }),
        )
    })?;

    let event_type = event["type"].as_str().unwrap_or("");
    let event_id = event["id"].as_str().unwrap_or("");
    let obj = &event["data"]["object"];

    let db = state.db.as_ref().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "database unavailable".into(),
        }),
    ))?;

    match event_type {
        "checkout.session.completed" => {
            let clerk_user_id = obj["metadata"]["clerk_user_id"]
                .as_str()
                .or_else(|| obj["client_reference_id"].as_str());
            let customer_id = obj["customer"].as_str();
            let subscription_id = obj["subscription"].as_str();
            let mode = obj["mode"].as_str().unwrap_or("");

            if let (Some(user_id), Some(cust_id)) = (clerk_user_id, customer_id) {
                if should_init_credits_on_checkout(mode) {
                    let sub = crate::db::SubscriptionRecord {
                        clerk_user_id: user_id.to_string(),
                        stripe_customer_id: cust_id.to_string(),
                        stripe_subscription_id: subscription_id.map(String::from),
                        plan_type: "monthly".to_string(),
                        status: "active".to_string(),
                        trial_end: None,
                        current_period_start: None,
                        current_period_end: None,
                    };
                    db.upsert_subscription(&sub);
                    // Batch B3 — meter paid subscribers; free/unmetered stays null until row exists.
                    if let Err(e) = db.init_credit_balance(user_id, DEFAULT_SUBSCRIPTION_CREDITS) {
                        // Do not panic in a Stripe webhook handler — a failure
                        // here must not stop the subscription being recorded.
                        tracing::error!(user_id, "failed to init credit balance: {e}");
                    }
                    db.record_billing_event(
                        user_id,
                        event_id,
                        0,
                        "Subscription created",
                        "completed",
                    );
                    tracing::info!("subscription created + credits init for user {user_id}");
                }
            }
        }
        "customer.subscription.updated" => {
            let customer_id = obj["customer"].as_str().unwrap_or("");
            if let Some(mut sub) = find_sub_by_customer(db, customer_id) {
                sub.status = obj["status"].as_str().unwrap_or("active").to_string();
                if let Some(plan) = obj["items"]["data"][0]["price"]["lookup_key"].as_str() {
                    if plan.contains("annual") {
                        sub.plan_type = "annual".to_string();
                    } else {
                        sub.plan_type = "monthly".to_string();
                    }
                }
                if let Some(end) = obj["current_period_end"].as_i64() {
                    sub.current_period_end = Some(
                        chrono::DateTime::from_timestamp(end, 0)
                            .map(|d| d.to_rfc3339())
                            .unwrap_or_default(),
                    );
                }
                if let Some(start) = obj["current_period_start"].as_i64() {
                    sub.current_period_start = Some(
                        chrono::DateTime::from_timestamp(start, 0)
                            .map(|d| d.to_rfc3339())
                            .unwrap_or_default(),
                    );
                }
                if let Some(trial_end) = obj["trial_end"].as_i64() {
                    sub.trial_end = Some(
                        chrono::DateTime::from_timestamp(trial_end, 0)
                            .map(|d| d.to_rfc3339())
                            .unwrap_or_default(),
                    );
                }
                db.upsert_subscription(&sub);
                tracing::info!("subscription updated for customer {customer_id}");
            }
        }
        "customer.subscription.deleted" => {
            let customer_id = obj["customer"].as_str().unwrap_or("");
            if let Some(mut sub) = find_sub_by_customer(db, customer_id) {
                sub.status = "cancelled".to_string();
                db.upsert_subscription(&sub);
                db.record_billing_event(
                    &sub.clerk_user_id,
                    event_id,
                    0,
                    "Subscription cancelled",
                    "completed",
                );
                tracing::info!("subscription cancelled for customer {customer_id}");
            }
        }
        "invoice.paid" => {
            let customer_id = obj["customer"].as_str().unwrap_or("");
            let amount = obj["amount_paid"].as_i64().unwrap_or(0);
            if let Some(sub) = find_sub_by_customer(db, customer_id) {
                if db.record_billing_event(
                    &sub.clerk_user_id,
                    event_id,
                    amount,
                    "Invoice paid",
                    "paid",
                ) {
                    tracing::info!("invoice paid for customer {customer_id}");
                } else {
                    tracing::debug!("duplicate webhook ignored: {event_id}");
                }
            }
        }
        "invoice.payment_failed" => {
            let customer_id = obj["customer"].as_str().unwrap_or("");
            if let Some(mut sub) = find_sub_by_customer(db, customer_id) {
                sub.status = "past_due".to_string();
                db.upsert_subscription(&sub);
                db.record_billing_event(
                    &sub.clerk_user_id,
                    event_id,
                    0,
                    "Payment failed",
                    "failed",
                );
                tracing::warn!("payment failed for customer {customer_id}");
            }
        }
        _ => {
            tracing::debug!("unhandled stripe event: {event_type}");
        }
    }

    Ok(StatusCode::OK)
}

fn require_stripe(
    state: &AppState,
) -> Result<&crate::stripe_client::StripeClient, (StatusCode, Json<ErrorResponse>)> {
    state.stripe_client.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            error: "Stripe integration not yet configured".into(),
        }),
    ))
}

fn find_sub_by_customer(db: &Database, customer_id: &str) -> Option<crate::db::SubscriptionRecord> {
    db.get_subscription_by_customer(customer_id)
}
