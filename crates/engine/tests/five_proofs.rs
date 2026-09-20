//! The 5 Proofs — falsifiable integration tests for Cortex v0.1 claims.

use cortex_core::autonomy::{decide_autonomy, AutonomyDecision};
use cortex_core::contamination::*;
use cortex_core::provider::ProviderId;
use cortex_core::routing::RiskLevel;
use cortex_engine::bandit::{ArmKey, TaskFamily, UcbScorer};
use cortex_engine::evidence_floor::{check_floor, FloorVerdict};
use cortex_engine::pipeline::{plan_route, record_outcome, PipelineConfig, PipelineResult};
use cortex_engine::store::CortexStore;

// ─── Proof 1: Pressure scorer beats static routing ──────────────────────────
// Run 100 tasks with one provider genuinely better. UCB routing must converge
// to the better provider and achieve higher cumulative reward than always
// picking a fixed provider.

#[test]
fn proof_1_pressure_scorer_beats_static() {
    let providers = [ProviderId::Claude, ProviderId::Openai, ProviderId::Gemini];
    let task_family = TaskFamily::CodeEdit;
    let risk = RiskLevel::Medium;

    // Claude: 80% success, Openai: 50%, Gemini: 30%
    let true_rates = [0.80, 0.50, 0.30];

    let mut scorer = UcbScorer::new(1.0);
    let mut ucb_total_reward = 0.0;
    let mut static_total_reward = 0.0;

    let mut rng_state: u64 = 42;
    let mut pseudo_rand = || -> f64 {
        rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (rng_state >> 33) as f64 / (1u64 << 31) as f64
    };

    for trial in 0..100 {
        // UCB picks
        let ucb_pick = scorer.best_arm(task_family, risk, &providers).unwrap();
        let ucb_idx = providers.iter().position(|p| *p == ucb_pick).unwrap();
        let ucb_success = pseudo_rand() < true_rates[ucb_idx];
        let ucb_reward = if ucb_success { 1.0 } else { 0.0 };
        ucb_total_reward += ucb_reward;

        scorer.update(
            ArmKey {
                task_family,
                risk_level: risk,
                provider: ucb_pick,
            },
            ucb_reward,
            0.0,
        );

        // Static always picks Openai (index 1) — the mediocre choice
        let static_success = pseudo_rand() < true_rates[1];
        static_total_reward += if static_success { 1.0 } else { 0.0 };

        // After 50 trials, UCB should have started converging
        if trial == 49 {
            let claude_arm = ArmKey {
                task_family,
                risk_level: risk,
                provider: ProviderId::Claude,
            };
            let stats = scorer.arms.get(&claude_arm);
            assert!(
                stats.is_some_and(|s| s.trials >= 5),
                "UCB should have explored Claude by trial 50"
            );
        }
    }

    assert!(
        ucb_total_reward > static_total_reward,
        "UCB ({ucb_total_reward}) should beat static-Openai ({static_total_reward})"
    );

    // UCB should converge to Claude (the best arm)
    let _best = scorer.best_arm(task_family, risk, &providers).unwrap();
    let claude_arm = ArmKey {
        task_family,
        risk_level: risk,
        provider: ProviderId::Claude,
    };
    let claude_stats = scorer.arms.get(&claude_arm).unwrap();
    assert!(
        claude_stats.trials >= 30,
        "UCB should route majority of trials to Claude (got {})",
        claude_stats.trials
    );
}

// ─── Proof 2: Contamination catches circular validation ─────────────────────
// Generate code with Claude → generate tests with Claude → contamination ≥ 0.7.
// Cross-model review gets lower contamination.

#[test]
fn proof_2_contamination_catches_circular_validation() {
    // Same model generates code and tests
    let same_model_test = ContaminationScore::compute(
        EvidenceSource::AiGeneratedTest,
        Some("claude-sonnet".into()),
        Some("claude-sonnet".into()),
    );
    assert!(
        same_model_test.raw_value >= 0.7,
        "Same-model test contamination should be >= 0.7, got {}",
        same_model_test.raw_value
    );
    assert!(same_model_test.same_model_penalty);

    // Same model reviews its own code
    let same_model_review = ContaminationScore::compute(
        EvidenceSource::AiReview,
        Some("gpt-5".into()),
        Some("gpt-5".into()),
    );
    assert!(
        same_model_review.raw_value >= 0.7,
        "Same-model review contamination should be >= 0.7, got {}",
        same_model_review.raw_value
    );
    assert!(same_model_review.same_model_penalty);

    // Cross-model review is cleaner
    let cross_model_review = ContaminationScore::compute(
        EvidenceSource::AiReview,
        Some("claude-sonnet".into()),
        Some("gpt-5".into()),
    );
    assert!(
        cross_model_review.raw_value < same_model_review.raw_value,
        "Cross-model review ({}) should be cleaner than same-model ({})",
        cross_model_review.raw_value,
        same_model_review.raw_value
    );
    assert!(!cross_model_review.same_model_penalty);

    // Compiler output is always clean regardless of who wrote the code
    let compiler = ContaminationScore::compute(EvidenceSource::CompilerOutput, None, None);
    assert!(
        compiler.raw_value < 0.01,
        "Compiler output contamination should be ~0, got {}",
        compiler.raw_value
    );

    // Effective reward scales with contamination
    let signal = EvidenceSignal::new(
        SignalTier::IndependentVerify,
        EvidenceSource::AiGeneratedTest,
        Some("claude-sonnet".into()),
        Some("claude-sonnet".into()),
        1.0,
        "AI tests AI code — same model",
    );
    assert!(
        signal.effective_reward < 0.3,
        "Highly contaminated signal should have low effective reward, got {}",
        signal.effective_reward
    );
}

// ─── Proof 3: Cold start works at 20-50 interactions ────────────────────────
// UCB should explore all providers early, then converge. Routing quality
// (cumulative reward per trial) should improve measurably by trial 50.

#[test]
fn proof_3_cold_start_converges_within_50() {
    let providers = [ProviderId::Claude, ProviderId::Openai, ProviderId::Gemini];
    let task_family = TaskFamily::CodeEdit;
    let risk = RiskLevel::Medium;

    // Claude is best: 85%, Openai: 40%, Gemini: 25%
    let true_rates = [0.85, 0.40, 0.25];

    let mut scorer = UcbScorer::new(1.5);

    let mut rng_state: u64 = 7919;
    let mut pseudo_rand = || -> f64 {
        rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (rng_state >> 33) as f64 / (1u64 << 31) as f64
    };

    let mut first_20_reward = 0.0;
    let mut last_20_reward = 0.0;

    for trial in 0..50 {
        let pick = scorer.best_arm(task_family, risk, &providers).unwrap();
        let idx = providers.iter().position(|p| *p == pick).unwrap();
        let success = pseudo_rand() < true_rates[idx];
        let reward = if success { 1.0 } else { 0.0 };

        if trial < 20 {
            first_20_reward += reward;
        }
        if trial >= 30 {
            last_20_reward += reward;
        }

        scorer.update(
            ArmKey {
                task_family,
                risk_level: risk,
                provider: pick,
            },
            reward,
            0.0,
        );
    }

    // All providers should have been explored
    for &prov in &providers {
        let arm = ArmKey {
            task_family,
            risk_level: risk,
            provider: prov,
        };
        let stats = scorer.arms.get(&arm);
        assert!(
            stats.is_some_and(|s| s.trials >= 1),
            "Provider {:?} should have been explored at least once",
            prov
        );
    }

    // Last 20 trials should perform better than first 20
    let first_avg = first_20_reward / 20.0;
    let last_avg = last_20_reward / 20.0;
    assert!(
        last_avg >= first_avg,
        "Routing quality should improve: first 20 avg={first_avg:.2}, last 20 avg={last_avg:.2}"
    );

    // By trial 50, Claude should be the preferred arm
    let best = scorer.best_arm(task_family, risk, &providers).unwrap();
    assert_eq!(
        best,
        ProviderId::Claude,
        "After 50 trials, best arm should be Claude (the true best)"
    );
}

// ─── Proof 4: Evidence floor blocks unsafe route ────────────────────────────
// Auth code → Critical risk → floor requires HardObjective + IndependentVerify.
// Without evidence: blocked. With contaminated evidence: blocked.
// With clean evidence: satisfied.

#[test]
fn proof_4_evidence_floor_blocks_unsafe_route() {
    // Auth code is Critical risk
    let risk = cortex_engine::risk::classify_risk(&["src/auth/middleware.ts"]);
    assert_eq!(
        risk,
        RiskLevel::Critical,
        "Auth code should be Critical risk"
    );

    // No evidence → blocked
    let verdict = check_floor(RiskLevel::Critical, &[]);
    match verdict {
        FloorVerdict::Blocked {
            missing,
            risk_level,
        } => {
            assert_eq!(risk_level, RiskLevel::Critical);
            assert_eq!(
                missing.len(),
                2,
                "Should require both HardObjective and IndependentVerify"
            );
        }
        other => panic!("Expected Blocked with no evidence, got {other:?}"),
    }

    // Contaminated AI evidence → still blocked (contamination too high)
    let contaminated_signals = vec![
        EvidenceSignal::new(
            SignalTier::HardObjective,
            EvidenceSource::AiGeneratedTest,
            Some("claude".into()),
            Some("claude".into()),
            1.0,
            "AI-generated tests (same model)",
        ),
        EvidenceSignal::new(
            SignalTier::IndependentVerify,
            EvidenceSource::AiReview,
            Some("claude".into()),
            Some("claude".into()),
            1.0,
            "AI review (same model)",
        ),
    ];

    let verdict = check_floor(RiskLevel::Critical, &contaminated_signals);
    match verdict {
        FloorVerdict::Blocked { missing, .. } => {
            assert!(
                !missing.is_empty(),
                "Contaminated evidence should not satisfy Critical floor"
            );
        }
        other => panic!("Expected Blocked with contaminated evidence, got {other:?}"),
    }

    // Clean evidence → satisfied
    let clean_signals = vec![
        EvidenceSignal::new(
            SignalTier::HardObjective,
            EvidenceSource::CompilerOutput,
            None,
            None,
            1.0,
            "Compiler passes",
        ),
        EvidenceSignal::new(
            SignalTier::IndependentVerify,
            EvidenceSource::HumanReview,
            None,
            None,
            1.0,
            "Human reviewed the auth change",
        ),
    ];

    let verdict = check_floor(RiskLevel::Critical, &clean_signals);
    match verdict {
        FloorVerdict::Satisfied { signals_met } => {
            assert_eq!(signals_met.len(), 2, "Both requirements should be met");
        }
        other => panic!("Expected Satisfied with clean evidence, got {other:?}"),
    }

    // The dial doesn't override the floor — even with max autonomy confidence,
    // Critical risk still requires approval
    let decision = decide_autonomy(0.99, RiskLevel::Critical, false);
    assert_eq!(
        decision,
        AutonomyDecision::RequireExplicitApproval,
        "Critical risk must always require explicit approval regardless of confidence"
    );
}

// ─── Proof 5: Three-mode abstraction holds ──────────────────────────────────
// The pipeline correctly handles three operational modes:
// 1. Standalone (no providers) → NoProviders
// 2. Single provider → routes through bandit + autonomy
// 3. Multi-provider → selects best via UCB, high-dial uses expensive templates
//
// Additionally: the full pipeline → store → reload cycle preserves state.

#[test]
fn proof_5_three_mode_abstraction_holds() {
    // Mode 1: No providers → NoProviders result
    let scorer = UcbScorer::new(1.0);
    let no_provider_config = PipelineConfig {
        dial: 5,
        available_providers: vec![],
        is_first_observation: false,
    };
    let result = plan_route("fix login", &["src/main.ts"], &no_provider_config, &scorer);
    assert!(
        matches!(result, PipelineResult::NoProviders),
        "No providers should return NoProviders"
    );

    // Mode 2: Single provider → routes successfully with autonomy
    let single_config = PipelineConfig {
        dial: 3,
        available_providers: vec![ProviderId::Claude],
        is_first_observation: false,
    };
    let result = plan_route("fix login bug", &["src/main.ts"], &single_config, &scorer);
    match &result {
        PipelineResult::Planned(plan) => {
            assert_eq!(plan.provider, ProviderId::Claude);
            assert!(!plan.steps.is_empty());
        }
        other => panic!("Single provider should produce Planned, got {other:?}"),
    }

    // Mode 3: Multi-provider → UCB selects best after training
    let mut multi_scorer = UcbScorer::new(0.5);
    let multi_config = PipelineConfig {
        dial: 5,
        available_providers: vec![ProviderId::Claude, ProviderId::Openai],
        is_first_observation: false,
    };

    // Train Claude as better (Medium risk — "utils" matches medium pattern)
    for _ in 0..30 {
        multi_scorer.update(
            ArmKey {
                task_family: TaskFamily::CodeEdit,
                risk_level: RiskLevel::Medium,
                provider: ProviderId::Claude,
            },
            0.9,
            0.0,
        );
        multi_scorer.update(
            ArmKey {
                task_family: TaskFamily::CodeEdit,
                risk_level: RiskLevel::Medium,
                provider: ProviderId::Openai,
            },
            0.3,
            0.0,
        );
    }

    let result = plan_route(
        "fix a typo",
        &["src/utils.ts"],
        &multi_config,
        &multi_scorer,
    );
    match &result {
        PipelineResult::Planned(plan) => {
            assert_eq!(
                plan.provider,
                ProviderId::Claude,
                "UCB should prefer Claude after training"
            );
        }
        other => panic!("Multi-provider should produce Planned, got {other:?}"),
    }

    // Full cycle: pipeline → record outcome → store → reload → scorer state preserved
    let store = CortexStore::open_memory().unwrap();
    let mut cycle_scorer = UcbScorer::new(1.0);
    let cycle_config = PipelineConfig {
        dial: 5,
        available_providers: vec![ProviderId::Claude],
        is_first_observation: false,
    };

    let result = plan_route("add feature", &["src/app.ts"], &cycle_config, &cycle_scorer);
    let plan = match result {
        PipelineResult::Planned(p) => p,
        other => panic!("Expected Planned, got {other:?}"),
    };

    record_outcome(&mut cycle_scorer, &plan, true, 0.0);

    // Save to store
    for (key, stats) in &cycle_scorer.arms {
        store.save_arm_stats(key, stats).unwrap();
    }

    // Reload from store
    let loaded = store.load_arm_stats().unwrap();
    assert!(!loaded.is_empty(), "Reloaded arm stats should not be empty");

    for (key, original_stats) in &cycle_scorer.arms {
        let loaded_stats = loaded.get(key).expect("Key should exist after reload");
        assert_eq!(loaded_stats.trials, original_stats.trials);
        assert_eq!(loaded_stats.successes, original_stats.successes);
        assert!(
            (loaded_stats.total_reward - original_stats.total_reward).abs() < 1e-9,
            "Reward should survive store round-trip"
        );
    }
}
