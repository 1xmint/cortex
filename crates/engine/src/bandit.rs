use std::collections::HashMap;

use chrono::{DateTime, Utc};
use cortex_core::provider::ProviderId;
use cortex_core::routing::{Intent, RiskLevel};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub enum TaskFamily {
    CodeEdit,
    CodeReview,
    Search,
    Testing,
    Planning,
    Deployment,
}

impl TaskFamily {
    pub fn from_intent(intent: &Intent) -> Self {
        match intent {
            Intent::Fix | Intent::Add | Intent::Refactor => Self::CodeEdit,
            Intent::Review => Self::CodeReview,
            Intent::Explore => Self::Search,
            Intent::Test => Self::Testing,
            Intent::Think => Self::Planning,
            Intent::Ship => Self::Deployment,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct ArmKey {
    pub task_family: TaskFamily,
    pub risk_level: RiskLevel,
    pub provider: ProviderId,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ArmStats {
    pub successes: u32,
    pub trials: u32,
    pub total_reward: f64,
    pub last_updated: DateTime<Utc>,
}

impl ArmStats {
    pub fn mean_reward(&self) -> f64 {
        if self.trials == 0 {
            0.5
        } else {
            self.total_reward / self.trials as f64
        }
    }

    pub fn is_cold(&self) -> bool {
        self.trials < 5
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UcbScorer {
    pub arms: HashMap<ArmKey, ArmStats>,
    pub exploration_weight: f64,
    pub total_trials: u32,
}

impl UcbScorer {
    pub fn new(exploration_weight: f64) -> Self {
        Self {
            arms: HashMap::new(),
            exploration_weight,
            total_trials: 0,
        }
    }

    pub fn score(&self, key: &ArmKey) -> f64 {
        let arm_trials = self.arms.get(key).map_or(0_u32, |stats| stats.trials);
        let mean_reward = self.arms.get(key).map_or(0.5, ArmStats::mean_reward);
        let numerator = (self.total_trials as f64 + 1.0).ln();
        let denominator = arm_trials as f64 + 1.0;
        let exploration_bonus = self.exploration_weight * (numerator / denominator).sqrt();

        mean_reward + exploration_bonus
    }

    pub fn update(&mut self, key: ArmKey, reward: f64, contamination: f64) {
        let effective_reward = reward * (1.0 - contamination.clamp(0.0, 1.0));
        let now = Utc::now();

        let entry = self.arms.entry(key).or_insert_with(|| ArmStats {
            successes: 0,
            trials: 0,
            total_reward: 0.0,
            last_updated: now,
        });

        entry.trials += 1;
        entry.total_reward += effective_reward;
        entry.last_updated = now;
        if effective_reward >= 0.5 {
            entry.successes += 1;
        }

        self.total_trials += 1;
    }

    pub fn best_arm(
        &self,
        task_family: TaskFamily,
        risk_level: RiskLevel,
        providers: &[ProviderId],
    ) -> Option<ProviderId> {
        providers.iter().copied().max_by(|left, right| {
            let left_score = self.score(&ArmKey {
                task_family,
                risk_level,
                provider: *left,
            });
            let right_score = self.score(&ArmKey {
                task_family,
                risk_level,
                provider: *right,
            });

            left_score.total_cmp(&right_score)
        })
    }

    pub fn exploration_weight_for_dial(dial: u8) -> f64 {
        let dial = dial.clamp(1, 10);
        if dial <= 5 {
            0.1 + (dial as f64 - 1.0) * (0.9 / 4.0)
        } else {
            1.0 + (dial as f64 - 5.0) * (1.5 / 5.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(provider: ProviderId) -> ArmKey {
        ArmKey {
            task_family: TaskFamily::CodeEdit,
            risk_level: RiskLevel::Medium,
            provider,
        }
    }

    #[test]
    fn test_cold_start_explores() {
        let mut scorer = UcbScorer::new(2.5);
        let warm_key = key(ProviderId::Claude);
        let cold_key = key(ProviderId::Openai);

        for _ in 0..25 {
            scorer.update(warm_key, 0.4, 0.0);
        }

        let warm_score = scorer.score(&warm_key);
        let cold_score = scorer.score(&cold_key);

        assert!(cold_score > warm_score);
    }

    #[test]
    fn test_successful_arm_preferred() {
        let mut scorer = UcbScorer::new(0.5);
        let winner = key(ProviderId::Claude);
        let loser = key(ProviderId::Gemini);

        for _ in 0..40 {
            scorer.update(winner, 1.0, 0.0);
            scorer.update(loser, 0.2, 0.0);
        }

        assert!(scorer.score(&winner) > scorer.score(&loser));
    }

    #[test]
    fn test_contaminated_reward_discounted() {
        let mut scorer = UcbScorer::new(1.0);
        let arm = key(ProviderId::Openai);

        scorer.update(arm, 1.0, 0.75);

        let stats = scorer.arms.get(&arm).expect("arm stats should exist");
        assert_eq!(stats.trials, 1);
        assert_eq!(stats.successes, 0);
        assert!((stats.total_reward - 0.25).abs() < 1e-9);
        assert!((stats.mean_reward() - 0.25).abs() < 1e-9);
    }

    #[test]
    fn test_dial_scales_exploration() {
        assert!((UcbScorer::exploration_weight_for_dial(1) - 0.1).abs() < 1e-9);
        assert!((UcbScorer::exploration_weight_for_dial(5) - 1.0).abs() < 1e-9);
        assert!((UcbScorer::exploration_weight_for_dial(10) - 2.5).abs() < 1e-9);
        assert!(
            UcbScorer::exploration_weight_for_dial(8) > UcbScorer::exploration_weight_for_dial(3)
        );
    }

    #[test]
    fn test_best_arm_returns_winner() {
        let mut scorer = UcbScorer::new(0.3);
        let providers = [ProviderId::Claude, ProviderId::Openai, ProviderId::Gemini];

        for _ in 0..20 {
            scorer.update(key(ProviderId::Claude), 0.95, 0.0);
            scorer.update(key(ProviderId::Openai), 0.45, 0.0);
            scorer.update(key(ProviderId::Gemini), 0.35, 0.0);
        }

        let best = scorer.best_arm(TaskFamily::CodeEdit, RiskLevel::Medium, &providers);
        assert_eq!(best, Some(ProviderId::Claude));
    }
}
