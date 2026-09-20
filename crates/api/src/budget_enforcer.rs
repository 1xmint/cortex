use crate::cost_estimator::CostEstimator;
use crate::db::{CostSession, CostWarning, Database};
use chrono::{Datelike, Utc};

/// Budget enforcer for tracking and limiting user costs.
/// Integrates with the existing billing gate system.
pub struct BudgetEnforcer<'a> {
    db: &'a Database,
}

impl<'a> BudgetEnforcer<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Check if a user can make a request without exceeding budget limits.
    /// Returns (allowed, budget_result, warning) where:
    /// - allowed: whether the request should proceed
    /// - budget_result: current budget status
    /// - warning: optional warning to record/display
    pub fn check_budget_before_request(
        &self,
        user_id: &str,
        provider: &str,
        model: &str,
        estimated_input_tokens: i64,
        estimated_output_tokens: i64,
        has_api_key: bool,
    ) -> (bool, BudgetCheckResult, Option<CostWarning>) {
        let budget = self.db.get_user_budget(user_id);
        let cost_type = CostEstimator::get_cost_type(provider, has_api_key);

        // Only track BYOK costs against budget limits
        if cost_type != "byok" {
            return (
                true,
                BudgetCheckResult {
                    daily_spent: 0.0,
                    weekly_spent: 0.0,
                    monthly_spent: 0.0,
                    daily_remaining: budget.daily_budget,
                    weekly_remaining: budget.weekly_budget,
                    monthly_remaining: budget.monthly_budget,
                    estimated_request_cost: 0.0,
                    cost_type: cost_type.clone(),
                },
                None,
            );
        }

        // Estimate request cost
        let (estimated_cost, _) = CostEstimator::estimate_request_cost(
            provider,
            model,
            estimated_input_tokens,
            estimated_output_tokens,
            None, // No cache info at request time
        );

        // Get current spending
        let (daily_spent, weekly_spent, monthly_spent) = self.get_current_spending(user_id);

        // Calculate remaining budgets
        let daily_remaining = (budget.daily_budget - daily_spent).max(0.0);
        let weekly_remaining = (budget.weekly_budget - weekly_spent).max(0.0);
        let monthly_remaining = (budget.monthly_budget - monthly_spent).max(0.0);

        let budget_result = BudgetCheckResult {
            daily_spent,
            weekly_spent,
            monthly_spent,
            daily_remaining,
            weekly_remaining,
            monthly_remaining,
            estimated_request_cost: estimated_cost,
            cost_type,
        };

        // Check if request would exceed any budget
        let daily_after = daily_spent + estimated_cost;
        let weekly_after = weekly_spent + estimated_cost;
        let monthly_after = monthly_spent + estimated_cost;

        // Hard limits check
        if daily_after > budget.daily_budget {
            return (false, budget_result, None);
        }
        if weekly_after > budget.weekly_budget {
            return (false, budget_result, None);
        }
        if monthly_after > budget.monthly_budget {
            return (false, budget_result, None);
        }

        // Warning threshold check
        let mut warning = None;
        if budget.notifications_enabled {
            // Check daily warning threshold
            if daily_after / budget.daily_budget >= budget.warning_threshold
                && daily_spent / budget.daily_budget < budget.warning_threshold
            {
                warning = Some(CostWarning {
                    id: String::new(), // Will be set by DB
                    user_id: user_id.to_string(),
                    warning_type: "daily".to_string(),
                    threshold_percent: budget.warning_threshold,
                    current_cost: daily_after,
                    budget_limit: budget.daily_budget,
                    triggered_at: Utc::now().timestamp(),
                    acknowledged_at: None,
                });
            }
            // Check weekly warning threshold
            else if weekly_after / budget.weekly_budget >= budget.warning_threshold
                && weekly_spent / budget.weekly_budget < budget.warning_threshold
            {
                warning = Some(CostWarning {
                    id: String::new(),
                    user_id: user_id.to_string(),
                    warning_type: "weekly".to_string(),
                    threshold_percent: budget.warning_threshold,
                    current_cost: weekly_after,
                    budget_limit: budget.weekly_budget,
                    triggered_at: Utc::now().timestamp(),
                    acknowledged_at: None,
                });
            }
            // Check monthly warning threshold
            else if monthly_after / budget.monthly_budget >= budget.warning_threshold
                && monthly_spent / budget.monthly_budget < budget.warning_threshold
            {
                warning = Some(CostWarning {
                    id: String::new(),
                    user_id: user_id.to_string(),
                    warning_type: "monthly".to_string(),
                    threshold_percent: budget.warning_threshold,
                    current_cost: monthly_after,
                    budget_limit: budget.monthly_budget,
                    triggered_at: Utc::now().timestamp(),
                    acknowledged_at: None,
                });
            }
        }

        (true, budget_result, warning)
    }

    /// Create a cost session for tracking.
    pub fn start_cost_session(
        &self,
        user_id: &str,
        provider: &str,
        cost_type: &str,
        estimated_cost: f64,
        tokens_in: i64,
        model: Option<&str>,
    ) -> String {
        let session = CostSession {
            id: String::new(), // Will be set by DB
            user_id: user_id.to_string(),
            provider: provider.to_string(),
            cost_type: cost_type.to_string(),
            session_start: Utc::now().timestamp(),
            session_end: None,
            estimated_cost,
            actual_cost: None,
            tokens_in,
            tokens_out: 0,
            model: model.map(|s| s.to_string()),
            created_at: Utc::now().timestamp(),
        };

        self.db.create_cost_session(&session)
    }

    /// Finalize a cost session with actual costs.
    pub fn finalize_cost_session(
        &self,
        session_id: &str,
        actual_cost: f64,
        tokens_out: i64,
    ) -> bool {
        let session_end = Utc::now().timestamp();
        self.db
            .update_cost_session(session_id, session_end, actual_cost, tokens_out)
    }

    /// Record a cost warning in the database.
    pub fn record_warning(&self, warning: &CostWarning) -> String {
        self.db.record_cost_warning(warning)
    }

    /// Get current spending across different time periods.
    /// Returns (daily_spent, weekly_spent, monthly_spent) for BYOK costs only.
    fn get_current_spending(&self, user_id: &str) -> (f64, f64, f64) {
        let now = Utc::now();

        // Daily spending (since midnight UTC)
        let midnight = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
        let daily_start =
            chrono::DateTime::<Utc>::from_naive_utc_and_offset(midnight, Utc).timestamp();
        let (daily_byok, _) = self.db.get_user_cost_breakdown(user_id, daily_start);

        // Weekly spending (since Monday at midnight UTC)
        let days_since_monday = now.weekday().num_days_from_monday() as i64;
        let monday = now.date_naive() - chrono::Duration::days(days_since_monday);
        let weekly_start = chrono::DateTime::<Utc>::from_naive_utc_and_offset(
            monday.and_hms_opt(0, 0, 0).unwrap(),
            Utc,
        )
        .timestamp();
        let (weekly_byok, _) = self.db.get_user_cost_breakdown(user_id, weekly_start);

        // Monthly spending (since 1st of month UTC)
        let first_of_month = now
            .date_naive()
            .with_day(1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let monthly_start =
            chrono::DateTime::<Utc>::from_naive_utc_and_offset(first_of_month, Utc).timestamp();
        let (monthly_byok, _) = self.db.get_user_cost_breakdown(user_id, monthly_start);

        (daily_byok, weekly_byok, monthly_byok)
    }

    /// Get spending trends for the last 30 days.
    pub fn get_spending_history(&self, user_id: &str, days: i64) -> Vec<DailySpending> {
        let now = Utc::now();
        let mut history = Vec::new();

        for i in 0..days {
            let date = now.date_naive() - chrono::Duration::days(i);
            let day_start = chrono::DateTime::<Utc>::from_naive_utc_and_offset(
                date.and_hms_opt(0, 0, 0).unwrap(),
                Utc,
            )
            .timestamp();
            let day_end = chrono::DateTime::<Utc>::from_naive_utc_and_offset(
                date.and_hms_opt(23, 59, 59).unwrap(),
                Utc,
            )
            .timestamp();

            let sessions = self
                .db
                .get_user_cost_sessions(user_id, day_start)
                .into_iter()
                .filter(|s| s.session_start <= day_end)
                .collect::<Vec<_>>();

            let byok_cost = sessions
                .iter()
                .filter(|s| s.cost_type == "byok")
                .map(|s| s.actual_cost.unwrap_or(s.estimated_cost))
                .sum::<f64>();

            let byos_cost = sessions
                .iter()
                .filter(|s| s.cost_type == "byos")
                .map(|s| s.estimated_cost)
                .sum::<f64>();

            history.push(DailySpending {
                date: date.format("%Y-%m-%d").to_string(),
                byok_cost,
                byos_cost,
                total_sessions: sessions.len(),
            });
        }

        history.reverse(); // Oldest first
        history
    }

    /// Check if a user should receive budget notifications.
    pub fn should_notify_user(&self, user_id: &str, warning_type: &str) -> bool {
        let budget = self.db.get_user_budget(user_id);
        if !budget.notifications_enabled {
            return false;
        }

        // Check if we've already sent this type of warning recently (within 24 hours)
        let last_24h = Utc::now().timestamp() - 24 * 60 * 60;
        let recent_warnings = self.db.get_user_cost_warnings(user_id, last_24h);

        !recent_warnings
            .iter()
            .any(|w| w.warning_type == warning_type && w.acknowledged_at.is_none())
    }
}

/// Result of a budget check before making a request.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BudgetCheckResult {
    pub daily_spent: f64,
    pub weekly_spent: f64,
    pub monthly_spent: f64,
    pub daily_remaining: f64,
    pub weekly_remaining: f64,
    pub monthly_remaining: f64,
    pub estimated_request_cost: f64,
    pub cost_type: String, // "byok" or "byos"
}

/// Daily spending breakdown for history charts.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DailySpending {
    pub date: String, // "YYYY-MM-DD"
    pub byok_cost: f64,
    pub byos_cost: f64,
    pub total_sessions: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile;
    use uuid;

    fn test_db() -> Database {
        let dir = tempfile::tempdir().unwrap().keep();
        Database::open(&dir.join("test.sqlite"))
    }

    #[test]
    fn test_budget_check_byos() {
        let db = test_db();
        let enforcer = BudgetEnforcer::new(&db);

        let (allowed, result, warning) = enforcer.check_budget_before_request(
            "user1",
            "claude",
            "claude-3-sonnet",
            1000,
            500,
            false, // No API key = BYOS
        );

        assert!(allowed);
        assert_eq!(result.cost_type, "byos");
        assert_eq!(result.estimated_request_cost, 0.0);
        assert!(warning.is_none());
    }

    #[test]
    fn test_budget_check_byok() {
        let db = test_db();
        let enforcer = BudgetEnforcer::new(&db);

        let (allowed, result, warning) = enforcer.check_budget_before_request(
            "user1",
            "claude",
            "claude-3-sonnet",
            1000,
            500,
            true, // Has API key = BYOK
        );

        assert!(allowed);
        assert_eq!(result.cost_type, "byok");
        assert!(result.estimated_request_cost > 0.0);
        assert!(warning.is_none());
    }

    #[test]
    fn test_cost_session_lifecycle() {
        let db = test_db();
        let enforcer = BudgetEnforcer::new(&db);

        // Start session
        let session_id = enforcer.start_cost_session(
            "user1",
            "claude",
            "byok",
            0.05,
            1000,
            Some("claude-3-sonnet"),
        );

        assert!(!session_id.is_empty());

        // Finalize session
        let success = enforcer.finalize_cost_session(&session_id, 0.048, 750);
        assert!(success);
    }

    #[test]
    fn test_spending_calculation() {
        let db = test_db();
        let enforcer = BudgetEnforcer::new(&db);

        // Create some test sessions.
        //
        // session_start must be `now`, not `now - 1000`. get_current_spending
        // computes daily_start as midnight UTC today, and get_user_cost_breakdown
        // filters on `session_start >= since`. Backdating by 1000s puts the row
        // in yesterday whenever the test runs within 16m40s of UTC midnight, so
        // daily comes back 0.0 and the assertion below fails. That window is
        // Dependabot's PR window, which made the one honest gate in CI look
        // unreliable.
        let now = Utc::now().timestamp();
        let session = crate::db::CostSession {
            id: uuid::Uuid::new_v4().to_string(),
            user_id: "user1".to_string(),
            provider: "claude".to_string(),
            cost_type: "byok".to_string(),
            session_start: now,
            session_end: Some(now),
            estimated_cost: 0.05,
            actual_cost: Some(0.048),
            tokens_in: 1000,
            tokens_out: 750,
            model: Some("claude-3-sonnet".to_string()),
            created_at: now,
        };

        db.create_cost_session(&session);

        let (daily, weekly, monthly) = enforcer.get_current_spending("user1");
        assert!(daily > 0.0);
        assert!(weekly > 0.0);
        assert!(monthly > 0.0);
    }
}
