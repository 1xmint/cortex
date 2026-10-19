//! The API engine: its own tool loop over the Messages API, tested on
//! recorded responses so no test ever calls the network or spends money.

use cortex::engine::api::{ApiEngine, MAX_TURNS, ReplayTransport};
use cortex::engine::{CancelToken, Engine, ModelCallRecord, SessionInput, SessionOutcome};
use cortex::model::builtin;
use cortex::payload::{Spend, Tokens};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

fn input(worktree: &Path) -> SessionInput {
    SessionInput {
        brief: "write hello to src/out.txt".into(),
        worktree: worktree.to_path_buf(),
        checks_dir: worktree.join("no-checks"),
        session: 1,
        timeout: Duration::from_secs(60),
    }
}

fn run(engine: &mut ApiEngine<ReplayTransport>, dir: &Path) -> (SessionOutcome, Vec<String>) {
    let (outcome, lines, _) = run_recorded(engine, dir);
    (outcome, lines)
}

fn run_recorded(
    engine: &mut ApiEngine<ReplayTransport>,
    dir: &Path,
) -> (SessionOutcome, Vec<String>, Vec<ModelCallRecord>) {
    let mut lines = Vec::new();
    let mut records = Vec::new();
    let outcome = engine
        .run(
            &input(dir),
            &mut |l| lines.push(l.to_string()),
            &mut |r| records.push(r),
            &CancelToken::new(),
        )
        .unwrap();
    (outcome, lines, records)
}

/// A reply that calls one tool.
fn calls(name: &str, input: Value) -> Value {
    json!({
        "type": "message",
        "role": "assistant",
        "stop_reason": "tool_use",
        "content": [{"type": "tool_use", "id": format!("id-{name}"), "name": name, "input": input}]
    })
}

fn done() -> Value {
    json!({
        "type": "message",
        "role": "assistant",
        "stop_reason": "end_turn",
        "content": [{"type": "text", "text": "done"}]
    })
}

/// The tool results the engine sent back, in order.
fn tool_results(engine: &ApiEngine<ReplayTransport>) -> Vec<(String, bool)> {
    engine
        .transport
        .requests
        .iter()
        .filter_map(|r| r["messages"].as_array()?.last().cloned())
        .filter(|m| m["role"] == "user")
        .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
        .filter(|c| c["type"] == "tool_result")
        .map(|c| {
            let text = match &c["content"] {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (text, c["is_error"] == json!(true))
        })
        .collect()
}

#[test]
fn api_engine_records_each_reply_as_a_model_call() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("readme.txt"), "project").unwrap();
    let replay = ReplayTransport::from_json(include_str!("fixtures/api-session.json")).unwrap();
    let mut engine = ApiEngine::new(replay);
    let (_, lines, records) = run_recorded(&mut engine, dir.path());
    assert_eq!(records.len(), 4, "{records:#?}");
    for (i, r) in records.iter().enumerate() {
        let n = i as u64 + 1;
        assert_eq!(r.session, 1);
        assert_eq!(r.role, "worker");
        assert_eq!(r.provider, "anthropic-api");
        assert_eq!(r.profile, "api-sonnet");
        // The replies name another model, so the profile's prices do not apply.
        assert_eq!(r.model, "claude-test-1");
        assert_eq!(r.estimated_usd_micros, None);
        assert_eq!(r.params["max_tokens"], json!(8192));
        assert_eq!(
            r.tokens,
            Tokens {
                input: Some(100 * n),
                output: Some(10 * n),
                ..Tokens::default()
            }
        );
        assert_eq!(r.spent, Spend::Unknown);
        // The body sent and the reply received, as the transcript saw them.
        let response = String::from_utf8(r.response.clone().unwrap()).unwrap();
        assert_eq!(response, lines[i]);
        let request: Value = serde_json::from_slice(r.request.as_ref().unwrap()).unwrap();
        assert_eq!(request, engine.transport.requests[i]);
    }
}

#[test]
fn api_request_uses_profile_model() {
    let dir = tempfile::tempdir().unwrap();
    let mut reply = done();
    reply["model"] = json!("claude-sonnet-5-5");
    reply["usage"] = json!({"input_tokens": 1000, "output_tokens": 100});
    let mut engine = ApiEngine::new(ReplayTransport::new(vec![reply.clone()]));
    assert_eq!(engine.version().unwrap(), "claude-sonnet-5-5");
    let (_, _, records) = run_recorded(&mut engine, dir.path());
    assert_eq!(engine.transport.requests[0]["model"], "claude-sonnet-5-5");
    let request: Value = serde_json::from_slice(records[0].request.as_ref().unwrap()).unwrap();
    assert_eq!(request["model"], "claude-sonnet-5-5");
    // $2 per million in and $10 per million out.
    assert_eq!(records[0].estimated_usd_micros, Some(3000));
    assert_eq!(records[0].spent, Spend::Usd { micros: 3000 });

    let mut opus = builtin("api-sonnet").unwrap();
    opus.id = "api-opus".into();
    opus.model = "claude-opus-5-5".into();
    let mut engine = ApiEngine::with_profile(ReplayTransport::new(vec![reply]), opus);
    let (_, _, records) = run_recorded(&mut engine, dir.path());
    assert_eq!(engine.transport.requests[0]["model"], "claude-opus-5-5");
    assert_eq!(records[0].profile, "api-opus");
    // The reply came from Sonnet, so the profile cannot price it.
    assert_eq!(records[0].estimated_usd_micros, None);
    assert_eq!(records[0].spent, Spend::Unknown);
}

#[test]
fn api_engine_replays_session_and_writes_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("readme.txt"), "project").unwrap();
    let replay = ReplayTransport::from_json(include_str!("fixtures/api-session.json")).unwrap();
    let mut engine = ApiEngine::new(replay);
    let (outcome, lines) = run(&mut engine, dir.path());
    assert_eq!(
        outcome,
        SessionOutcome::Ended {
            note: "Wrote src/out.txt.".into(),
            reported_usd_micros: None,
        }
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("src/out.txt")).unwrap(),
        "hello\n"
    );
    let first = &engine.transport.requests[0];
    assert_eq!(first["messages"][0]["role"], "user");
    assert!(
        first["messages"][0]["content"]
            .to_string()
            .contains("write hello to src/out.txt")
    );
    let mut names: Vec<&str> = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["list_files", "read_file", "run", "write_file"]);
    let results = tool_results(&engine);
    assert_eq!(results.len(), 3, "{results:?}");
    assert!(results[0].0.contains("readme.txt"), "{results:?}");
    assert!(results[2].0.contains("hello"), "{results:?}");
    assert!(results.iter().all(|(_, err)| !err), "{results:?}");
    // Every reply the engine got is in the transcript.
    assert_eq!(lines.len(), 4, "{lines:#?}");
}

#[test]
fn api_engine_confines_paths() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(dir.path().join("outside.txt"), "secret").unwrap();
    let mut responses = vec![
        calls(
            "write_file",
            json!({"path": "../escape.txt", "content": "x"}),
        ),
        calls("read_file", json!({"path": "../outside.txt"})),
        calls(
            "read_file",
            json!({"path": dir.path().join("outside.txt").display().to_string()}),
        ),
        calls("list_files", json!({"path": ".."})),
    ];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path(), work.join("link")).unwrap();
        responses.push(calls("read_file", json!({"path": "link/outside.txt"})));
        responses.push(calls(
            "write_file",
            json!({"path": "link/escape2.txt", "content": "x"}),
        ));
    }
    let count = responses.len();
    responses.push(done());
    let mut engine = ApiEngine::new(ReplayTransport::new(responses));
    let (outcome, _) = run(&mut engine, &work);
    assert!(
        matches!(outcome, SessionOutcome::Ended { .. }),
        "{outcome:?}"
    );
    let results = tool_results(&engine);
    assert_eq!(results.len(), count, "{results:?}");
    for (text, is_error) in &results {
        assert!(is_error, "{text}");
        assert!(!text.contains("secret"), "{text}");
    }
    assert!(!dir.path().join("escape.txt").exists());
    assert!(!dir.path().join("escape2.txt").exists());
}

#[test]
fn api_engine_stops_at_turn_limit() {
    let dir = tempfile::tempdir().unwrap();
    let responses = (0..MAX_TURNS + 5)
        .map(|_| calls("list_files", json!({"path": "."})))
        .collect();
    let mut engine = ApiEngine::new(ReplayTransport::new(responses));
    let (outcome, _) = run(&mut engine, dir.path());
    let SessionOutcome::Failed { reason } = outcome else {
        panic!("{outcome:?}");
    };
    assert!(reason.contains("turn"), "{reason}");
    assert_eq!(engine.transport.requests.len(), MAX_TURNS);
}

#[test]
fn api_engine_maps_errors() {
    let dir = tempfile::tempdir().unwrap();
    let limited =
        json!({"type": "error", "error": {"type": "rate_limit_error", "message": "slow down"}});
    let mut engine = ApiEngine::new(ReplayTransport::new(vec![limited]));
    let (outcome, _) = run(&mut engine, dir.path());
    assert!(
        matches!(outcome, SessionOutcome::RateLimited { .. }),
        "{outcome:?}"
    );
    let broken =
        json!({"type": "error", "error": {"type": "invalid_request_error", "message": "bad"}});
    let mut engine = ApiEngine::new(ReplayTransport::new(vec![broken]));
    let (outcome, _) = run(&mut engine, dir.path());
    let SessionOutcome::Failed { reason } = outcome else {
        panic!("{outcome:?}");
    };
    assert!(reason.contains("bad"), "{reason}");
}

#[cfg(target_os = "linux")]
#[test]
fn api_engine_routes_run_through_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let responses = vec![
        calls(
            "run",
            json!({"command": "cat /proc/version; echo made > made.txt; exit 4"}),
        ),
        done(),
    ];
    let mut engine = ApiEngine::new(ReplayTransport::new(responses));
    let (outcome, _) = run(&mut engine, dir.path());
    assert!(
        matches!(outcome, SessionOutcome::Ended { .. }),
        "{outcome:?}"
    );
    let results = tool_results(&engine);
    let (text, _) = &results[0];
    assert!(text.to_lowercase().contains("gvisor"), "{text}");
    assert!(text.contains("exit code: 4"), "{text}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("made.txt")).unwrap(),
        "made\n"
    );
}
