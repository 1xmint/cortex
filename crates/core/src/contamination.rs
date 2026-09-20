use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum EvidenceSource {
    HumanWrittenTest,
    HumanReview,
    HumanApproval,
    AiGeneratedTest,
    AiGeneratedCode,
    AiReview,
    CompilerOutput,
    CiPipeline,
    ExistingTestSuite,
    RuntimeBehavior,
    Linter,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Copy)]
#[repr(u8)]
pub enum SignalTier {
    Policy = 0,
    HardObjective = 1,
    IndependentVerify = 2,
    Stability = 3,
    WeakSubjective = 4,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ContaminationScore {
    pub raw_value: f64,
    pub source: EvidenceSource,
    pub generator_model: Option<String>,
    pub verifier_model: Option<String>,
    pub same_model_penalty: bool,
}

impl ContaminationScore {
    pub fn compute(
        source: EvidenceSource,
        generator_model: Option<String>,
        verifier_model: Option<String>,
    ) -> Self {
        let same_model_penalty = matches!(
            source,
            EvidenceSource::AiGeneratedTest | EvidenceSource::AiReview
        ) && matches!(
            (&generator_model, &verifier_model),
            (Some(generator), Some(verifier)) if generator == verifier
        );

        let raw_value = match source {
            EvidenceSource::CompilerOutput
            | EvidenceSource::CiPipeline
            | EvidenceSource::RuntimeBehavior
            | EvidenceSource::Linter => 0.0,
            EvidenceSource::ExistingTestSuite => 0.1,
            EvidenceSource::HumanWrittenTest => 0.05,
            EvidenceSource::HumanReview | EvidenceSource::HumanApproval => 0.0,
            EvidenceSource::AiGeneratedTest if same_model_penalty => 0.85,
            EvidenceSource::AiGeneratedTest => 0.5,
            EvidenceSource::AiGeneratedCode => 0.3,
            EvidenceSource::AiReview if same_model_penalty => 0.8,
            EvidenceSource::AiReview => 0.35,
        };

        Self {
            raw_value,
            source,
            generator_model,
            verifier_model,
            same_model_penalty,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EvidenceSignal {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub tier: SignalTier,
    pub source: EvidenceSource,
    pub contamination: ContaminationScore,
    pub raw_reward: f64,
    pub effective_reward: f64,
    pub description: String,
}

impl EvidenceSignal {
    pub fn new(
        tier: SignalTier,
        source: EvidenceSource,
        generator_model: Option<String>,
        verifier_model: Option<String>,
        raw_reward: f64,
        description: impl Into<String>,
    ) -> Self {
        let contamination =
            ContaminationScore::compute(source.clone(), generator_model, verifier_model);
        let effective_reward = raw_reward * (1.0 - contamination.raw_value);

        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            tier,
            source,
            contamination,
            raw_reward,
            effective_reward,
            description: description.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(left: f64, right: f64) {
        assert!((left - right).abs() < 1e-9, "{left} != {right}");
    }

    #[test]
    fn test_compiler_zero_contamination() {
        let score = ContaminationScore::compute(EvidenceSource::CompilerOutput, None, None);
        approx_eq(score.raw_value, 0.0);
        assert!(!score.same_model_penalty);
    }

    #[test]
    fn test_same_model_generates_and_tests() {
        let score = ContaminationScore::compute(
            EvidenceSource::AiGeneratedTest,
            Some("gpt-5".to_string()),
            Some("gpt-5".to_string()),
        );

        assert!(score.raw_value >= 0.8);
        assert!(score.same_model_penalty);
    }

    #[test]
    fn test_cross_model_review() {
        let score = ContaminationScore::compute(
            EvidenceSource::AiReview,
            Some("gpt-5".to_string()),
            Some("claude".to_string()),
        );

        approx_eq(score.raw_value, 0.35);
        assert!(!score.same_model_penalty);
    }

    #[test]
    fn test_human_review_clean() {
        let score = ContaminationScore::compute(EvidenceSource::HumanReview, None, None);
        approx_eq(score.raw_value, 0.0);
        assert!(!score.same_model_penalty);
    }

    #[test]
    fn test_effective_reward_scales_down() {
        let signal = EvidenceSignal::new(
            SignalTier::IndependentVerify,
            EvidenceSource::AiGeneratedTest,
            Some("gpt-5".to_string()),
            Some("gpt-5".to_string()),
            1.0,
            "AI-generated test passed against AI-generated code",
        );

        approx_eq(signal.contamination.raw_value, 0.85);
        approx_eq(signal.effective_reward, 0.15);
    }
}
