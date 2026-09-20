//! Context-Flow Pipeline Integration Tests
//!
//! Tests the complete Context-Flow system including artifact creation,
//! context assembly, database persistence, and end-to-end flow.

use chrono::Utc;
use std::collections::HashMap;
use tempfile::TempDir;
use uuid::Uuid;

use cortex_api::context_flow::{
    Artifact, ArtifactKind, ContextBus, ContextBusConfig, ContextTransform,
};
use cortex_api::db::Database;

/// Helper to create a test database
fn create_test_db() -> (TempDir, Database) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = Database::open(&db_path);
    (temp_dir, db)
}

/// Helper to create a test artifact
fn create_test_artifact(
    step_id: &str,
    run_id: &str,
    kind: ArtifactKind,
    content: &str,
    summary: &str,
) -> Artifact {
    Artifact {
        id: Uuid::new_v4().to_string(),
        producer_step_id: step_id.to_string(),
        producer_run_id: run_id.to_string(),
        kind,
        content: content.to_string(),
        summary: summary.to_string(),
        files_changed: vec!["test.rs".to_string()],
        confidence: 0.8,
        // An artifact is a step's own account of its own work, so this is what
        // `ContextBus::create_artifact` sets. Constructed literally here to keep
        // the helper independent of that constructor.
        provenance: cortex_core::provenance::Provenance::AgentOutput {
            producer_step_id: step_id.to_string(),
        },
        tokens: (content.len() / 4) as u32,
        created_at: Utc::now(),
        metadata: HashMap::new(),
    }
}

#[tokio::test]
async fn test_artifact_creation_and_storage() {
    let (_temp_dir, db) = create_test_db();
    let context_bus = ContextBus::new(ContextBusConfig::default());

    let run_id = "test-run-1";
    let step_id = "test-step-1";

    // Create and store an artifact
    let artifact = create_test_artifact(
        step_id,
        run_id,
        ArtifactKind::Code,
        "fn hello() { println!(\"Hello, world!\"); }",
        "Created a simple hello function",
    );

    context_bus.add_artifact(Some(&db), artifact.clone()).await;

    // Verify artifact was stored in database
    let stored_artifacts = db.get_context_artifacts_for_run(run_id);
    assert_eq!(stored_artifacts.len(), 1);

    let (
        stored_id,
        stored_step_id,
        stored_kind,
        stored_content,
        stored_summary,
        stored_files,
        stored_confidence,
        stored_tokens,
        _created_at,
    ) = &stored_artifacts[0];

    assert_eq!(stored_id, &artifact.id);
    assert_eq!(stored_step_id, step_id);
    assert_eq!(stored_kind, "code");
    assert_eq!(stored_content, &artifact.content);
    assert_eq!(stored_summary, &artifact.summary);
    assert_eq!(stored_files, &artifact.files_changed);
    assert_eq!(*stored_confidence, 0.8);
    assert_eq!(*stored_tokens, artifact.tokens);
}

#[tokio::test]
async fn test_context_assembly_from_multiple_artifacts() {
    let (_temp_dir, db) = create_test_db();
    let context_bus = ContextBus::new(ContextBusConfig::default());

    let run_id = "test-run-2";

    // Create multiple artifacts in sequence
    let artifacts = vec![
        create_test_artifact(
            "step-1",
            run_id,
            ArtifactKind::Analysis,
            "The user wants to create a web server",
            "Analyzed requirements: web server needed",
        ),
        create_test_artifact(
            "step-2",
            run_id,
            ArtifactKind::Plan,
            "1. Use axum framework\n2. Add routes\n3. Start server",
            "Created implementation plan",
        ),
        create_test_artifact(
            "step-3",
            run_id,
            ArtifactKind::Code,
            "use axum::Router;\nfn main() { /* server code */ }",
            "Implemented basic server structure",
        ),
    ];

    // Store all artifacts
    for artifact in artifacts {
        context_bus.add_artifact(Some(&db), artifact).await;
        // Add small delay to ensure different timestamps
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // Assemble context for next step
    let context = context_bus
        .assemble_context(Some(&db), run_id, "Create a web server", None)
        .await;

    // Verify context assembly
    assert_eq!(context.user_goal, "Create a web server");
    assert_eq!(context.predecessor_summaries.len(), 3);

    // Check that artifacts are in chronological order
    assert_eq!(context.predecessor_summaries[0].step_id, "step-1");
    assert_eq!(context.predecessor_summaries[0].kind, "analysis");
    assert_eq!(context.predecessor_summaries[1].step_id, "step-2");
    assert_eq!(context.predecessor_summaries[1].kind, "plan");
    assert_eq!(context.predecessor_summaries[2].step_id, "step-3");
    assert_eq!(context.predecessor_summaries[2].kind, "code");

    // Check that files_changed are preserved
    assert_eq!(
        context.predecessor_summaries[0].files_changed,
        vec!["test.rs"]
    );
}

#[tokio::test]
async fn test_token_budget_enforcement() {
    let (_temp_dir, db) = create_test_db();

    // Create config with very small token budget
    let config = ContextBusConfig {
        max_predecessors: 10,
        max_total_tokens: 50, // Very small budget
        default_transform: ContextTransform::default(),
    };
    let context_bus = ContextBus::new(config);

    let run_id = "test-run-budget";

    // Create artifacts that exceed the budget
    let large_content = "x".repeat(1000); // ~250 tokens
    let artifacts = vec![
        create_test_artifact(
            "step-1",
            run_id,
            ArtifactKind::Code,
            &large_content,
            "Large artifact 1",
        ),
        create_test_artifact(
            "step-2",
            run_id,
            ArtifactKind::Code,
            &large_content,
            "Large artifact 2",
        ),
    ];

    for artifact in artifacts {
        context_bus.add_artifact(Some(&db), artifact).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // Assemble context - should respect token budget
    let context = context_bus
        .assemble_context(Some(&db), run_id, "Test goal", None)
        .await;

    // Should only include artifacts that fit within budget
    // With 50 token budget, should only fit first artifact (partially)
    assert!(context.predecessor_summaries.len() <= 2);

    // Verify total content is within reasonable bounds
    let total_content_length: usize = context
        .predecessor_summaries
        .iter()
        .map(|s| s.summary.len())
        .sum();

    // Should be significantly smaller than original content
    assert!(total_content_length < 2000);
}

#[tokio::test]
async fn test_max_predecessors_limit() {
    let (_temp_dir, db) = create_test_db();

    // Create config with small predecessor limit
    let config = ContextBusConfig {
        max_predecessors: 2,
        max_total_tokens: 10000,
        default_transform: ContextTransform::default(),
    };
    let context_bus = ContextBus::new(config);

    let run_id = "test-run-predecessors";

    // Create more artifacts than the limit
    for i in 1..=5 {
        let artifact = create_test_artifact(
            &format!("step-{}", i),
            run_id,
            ArtifactKind::Code,
            "small content",
            &format!("Step {} output", i),
        );
        context_bus.add_artifact(Some(&db), artifact).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // Assemble context - should respect predecessor limit
    let context = context_bus
        .assemble_context(Some(&db), run_id, "Test goal", None)
        .await;

    // Should only include first 2 artifacts (oldest first)
    assert_eq!(context.predecessor_summaries.len(), 2);
    assert_eq!(context.predecessor_summaries[0].step_id, "step-1");
    assert_eq!(context.predecessor_summaries[1].step_id, "step-2");
}

#[tokio::test]
async fn test_artifact_type_classification() {
    let (_temp_dir, db) = create_test_db();
    let context_bus = ContextBus::new(ContextBusConfig::default());

    let run_id = "test-run-types";

    // Create artifacts of different types
    let test_cases = vec![
        (ArtifactKind::Answer, "answer", "This is the final answer"),
        (ArtifactKind::Code, "code", "fn main() {}"),
        (
            ArtifactKind::Analysis,
            "analysis",
            "Analysis of the problem",
        ),
        (ArtifactKind::Plan, "plan", "Step 1: Do something"),
        (ArtifactKind::Review, "review", "Code looks good"),
        (ArtifactKind::Error, "error", "Something went wrong"),
        (ArtifactKind::Working, "working", "Work in progress"),
    ];

    for (kind, expected_kind_str, content) in test_cases {
        let artifact = create_test_artifact("step-1", run_id, kind, content, "Test artifact");

        context_bus.add_artifact(Some(&db), artifact).await;

        // Verify kind is stored correctly
        let stored_artifacts = db.get_context_artifacts_for_run(run_id);
        let last_artifact = stored_artifacts.last().unwrap();
        assert_eq!(last_artifact.2, expected_kind_str); // kind field

        // Clear for next test
        db.cleanup_context_artifacts_for_run(run_id);
    }
}

#[tokio::test]
async fn test_graceful_degradation_without_database() {
    let context_bus = ContextBus::new(ContextBusConfig::default());

    let run_id = "test-run-no-db";

    // Create artifact
    let artifact = create_test_artifact(
        "step-1",
        run_id,
        ArtifactKind::Code,
        "fn test() {}",
        "Test function",
    );

    // Add artifact without database (should not panic)
    context_bus.add_artifact(None, artifact).await;

    // Assemble context without database (should return empty context)
    let context = context_bus
        .assemble_context(None, run_id, "Test goal", None)
        .await;

    assert_eq!(context.user_goal, "Test goal");
    assert_eq!(context.predecessor_summaries.len(), 0);
    assert_eq!(context.conversation_excerpt, None);
}

#[tokio::test]
async fn test_cleanup_removes_artifacts() {
    let (_temp_dir, db) = create_test_db();
    let context_bus = ContextBus::new(ContextBusConfig::default());

    let run_id = "test-run-cleanup";

    // Create artifacts
    for i in 1..=3 {
        let artifact = create_test_artifact(
            &format!("step-{}", i),
            run_id,
            ArtifactKind::Code,
            "test content",
            "test summary",
        );
        context_bus.add_artifact(Some(&db), artifact).await;
    }

    // Verify artifacts exist
    let artifacts_before = db.get_context_artifacts_for_run(run_id);
    assert_eq!(artifacts_before.len(), 3);

    // Cleanup
    context_bus.cleanup_run(Some(&db), run_id).await;

    // Verify artifacts are removed
    let artifacts_after = db.get_context_artifacts_for_run(run_id);
    assert_eq!(artifacts_after.len(), 0);
}

#[tokio::test]
async fn test_empty_run_returns_empty_context() {
    let (_temp_dir, db) = create_test_db();
    let context_bus = ContextBus::new(ContextBusConfig::default());

    let run_id = "empty-run";

    // Assemble context for run with no artifacts
    let context = context_bus
        .assemble_context(
            Some(&db),
            run_id,
            "Test goal",
            Some("Test conversation".to_string()),
        )
        .await;

    assert_eq!(context.user_goal, "Test goal");
    assert_eq!(context.predecessor_summaries.len(), 0);
    assert_eq!(
        context.conversation_excerpt,
        Some("Test conversation".to_string())
    );
}

#[tokio::test]
async fn test_database_statistics() {
    let (_temp_dir, db) = create_test_db();
    let context_bus = ContextBus::new(ContextBusConfig::default());

    let run1 = "stats-run-1";
    let run2 = "stats-run-2";

    // Create artifacts of different types across multiple runs
    let artifacts = vec![
        (run1, ArtifactKind::Code, "code content"),
        (run1, ArtifactKind::Analysis, "analysis content"),
        (run2, ArtifactKind::Code, "more code"),
        (run2, ArtifactKind::Plan, "plan content"),
    ];

    for (run_id, kind, content) in artifacts {
        let artifact = create_test_artifact("step-1", run_id, kind, content, "test summary");
        context_bus.add_artifact(Some(&db), artifact).await;
    }

    // Test statistics
    let (total, kind_counts, avg_tokens) = db.get_context_artifact_stats();
    assert_eq!(total, 4);
    assert_eq!(kind_counts.get("code").unwrap(), &2);
    assert_eq!(kind_counts.get("analysis").unwrap(), &1);
    assert_eq!(kind_counts.get("plan").unwrap(), &1);
    assert!(avg_tokens > 0.0);

    // Test recent runs
    let recent_runs = db.get_recent_runs_with_artifacts(5);
    assert!(recent_runs.contains(&run1.to_string()));
    assert!(recent_runs.contains(&run2.to_string()));
}
