use cortex_core::contamination::{EvidenceSignal, SignalTier};
use cortex_core::routing::RiskLevel;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequiredSignal {
    pub tier: SignalTier,
    pub min_count: u32,
    pub max_contamination: f64,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvidenceFloor {
    pub risk_level: RiskLevel,
    pub required_signals: Vec<RequiredSignal>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FloorVerdict {
    Satisfied {
        signals_met: Vec<String>,
    },
    Blocked {
        missing: Vec<String>,
        risk_level: RiskLevel,
    },
}

pub fn invariant_floors() -> Vec<EvidenceFloor> {
    vec![
        EvidenceFloor {
            risk_level: RiskLevel::Critical,
            required_signals: vec![
                RequiredSignal {
                    tier: SignalTier::HardObjective,
                    min_count: 1,
                    max_contamination: 0.3,
                    description: "Compiler or existing test suite must pass".into(),
                },
                RequiredSignal {
                    tier: SignalTier::IndependentVerify,
                    min_count: 1,
                    max_contamination: 0.5,
                    description: "Independent review required for critical code".into(),
                },
            ],
        },
        EvidenceFloor {
            risk_level: RiskLevel::High,
            required_signals: vec![RequiredSignal {
                tier: SignalTier::HardObjective,
                min_count: 1,
                max_contamination: 0.5,
                description: "Objective verification required".into(),
            }],
        },
        EvidenceFloor {
            risk_level: RiskLevel::Medium,
            required_signals: Vec::new(),
        },
        EvidenceFloor {
            risk_level: RiskLevel::Low,
            required_signals: Vec::new(),
        },
    ]
}

pub fn floor_for_risk(risk_level: RiskLevel) -> EvidenceFloor {
    invariant_floors()
        .into_iter()
        .find(|f| f.risk_level == risk_level)
        .unwrap_or(EvidenceFloor {
            risk_level,
            required_signals: Vec::new(),
        })
}

pub fn check_floor(risk_level: RiskLevel, signals: &[EvidenceSignal]) -> FloorVerdict {
    let floor = floor_for_risk(risk_level);

    if floor.required_signals.is_empty() {
        return FloorVerdict::Satisfied {
            signals_met: Vec::new(),
        };
    }

    let mut signals_met = Vec::new();
    let mut missing = Vec::new();

    for req in &floor.required_signals {
        let matched = signals
            .iter()
            .filter(|s| s.tier == req.tier && s.contamination.raw_value <= req.max_contamination)
            .count() as u32;

        if matched >= req.min_count {
            signals_met.push(req.description.clone());
        } else {
            missing.push(req.description.clone());
        }
    }

    if missing.is_empty() {
        FloorVerdict::Satisfied { signals_met }
    } else {
        FloorVerdict::Blocked {
            missing,
            risk_level,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::contamination::EvidenceSource;

    fn make_signal(
        tier: SignalTier,
        source: EvidenceSource,
        contamination_override: Option<f64>,
    ) -> EvidenceSignal {
        let mut signal = EvidenceSignal::new(tier, source.clone(), None, None, 1.0, "test signal");
        if let Some(c) = contamination_override {
            signal.contamination.raw_value = c;
        }
        signal
    }

    #[test]
    fn test_critical_blocked_without_evidence() {
        match check_floor(RiskLevel::Critical, &[]) {
            FloorVerdict::Blocked {
                missing,
                risk_level,
            } => {
                assert_eq!(risk_level, RiskLevel::Critical);
                assert_eq!(missing.len(), 2);
            }
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    #[test]
    fn test_critical_satisfied_with_clean_signals() {
        let signals = vec![
            make_signal(
                SignalTier::HardObjective,
                EvidenceSource::CompilerOutput,
                None,
            ),
            make_signal(
                SignalTier::IndependentVerify,
                EvidenceSource::HumanReview,
                None,
            ),
        ];
        match check_floor(RiskLevel::Critical, &signals) {
            FloorVerdict::Satisfied { signals_met } => assert_eq!(signals_met.len(), 2),
            other => panic!("expected Satisfied, got {other:?}"),
        }
    }

    #[test]
    fn test_critical_blocked_contaminated_signals() {
        let signals = vec![
            make_signal(
                SignalTier::HardObjective,
                EvidenceSource::AiGeneratedTest,
                Some(0.85),
            ),
            make_signal(
                SignalTier::IndependentVerify,
                EvidenceSource::AiReview,
                Some(0.8),
            ),
        ];
        match check_floor(RiskLevel::Critical, &signals) {
            FloorVerdict::Blocked {
                missing,
                risk_level,
            } => {
                assert_eq!(risk_level, RiskLevel::Critical);
                assert_eq!(missing.len(), 2);
            }
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    #[test]
    fn test_high_satisfied_with_compiler() {
        let signals = vec![make_signal(
            SignalTier::HardObjective,
            EvidenceSource::CompilerOutput,
            None,
        )];
        match check_floor(RiskLevel::High, &signals) {
            FloorVerdict::Satisfied { signals_met } => assert_eq!(signals_met.len(), 1),
            other => panic!("expected Satisfied, got {other:?}"),
        }
    }

    #[test]
    fn test_low_always_satisfied() {
        match check_floor(RiskLevel::Low, &[]) {
            FloorVerdict::Satisfied { .. } => {}
            other => panic!("expected Satisfied, got {other:?}"),
        }
    }

    #[test]
    fn test_medium_always_satisfied() {
        match check_floor(RiskLevel::Medium, &[]) {
            FloorVerdict::Satisfied { .. } => {}
            other => panic!("expected Satisfied, got {other:?}"),
        }
    }
}
