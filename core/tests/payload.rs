//! Model calls are recorded as small audit events that point at their
//! full request and response bodies, which live in a separate table that
//! can be deleted without breaking the log.

use cortex::engine::ModelCallRecord;
use cortex::event::Event;
use cortex::payload::{Spend, Tokens};
use cortex::store::{MissionId, ProjectId, Store};
use serde_json::json;
use sha2::{Digest, Sha256};

fn temp_store() -> (tempfile::TempDir, Store, MissionId) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("cortex.db")).unwrap();
    let m = store
        .create_mission(&ProjectId("p".into()), "josh")
        .unwrap();
    (dir, store, m)
}

fn record(response: Vec<u8>) -> ModelCallRecord {
    ModelCallRecord {
        session: 1,
        role: "worker".into(),
        provider: "anthropic-api".into(),
        profile: "default".into(),
        model: "claude-sonnet-5-5".into(),
        params: json!({"max_tokens": 8192}),
        tokens: Tokens {
            input: Some(1200),
            output: Some(300),
            cache_read: None,
            cache_write: None,
        },
        estimated_usd_micros: None,
        spent: Spend::Unknown,
        millis: 850,
        request: Some(b"{\"messages\":[]}".to_vec()),
        response: Some(response),
    }
}

fn model_call(store: &Store, m: &MissionId) -> Event {
    store
        .events(m)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .find(|e| matches!(e, Event::ModelCall { .. }))
        .expect("a ModelCall event")
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn model_call_points_at_payloads() {
    let (dir, store, m) = temp_store();
    let response: Vec<u8> = (0..10_240).map(|i| b'a' + (i % 26) as u8).collect();
    store
        .record_model_call(&m, record(response.clone()))
        .unwrap();

    let conn = rusqlite::Connection::open(dir.path().join("cortex.db")).unwrap();
    let body: String = conn
        .query_row(
            "SELECT body_json FROM events WHERE kind = 'model_call'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(body.len() < 2048, "event body is {} bytes", body.len());

    let Event::ModelCall {
        response: Some(r),
        request: Some(_),
        ..
    } = model_call(&store, &m)
    else {
        panic!("ModelCall without payload refs");
    };
    assert_eq!(r.bytes, response.len() as u64);
    assert_eq!(r.sha256, hex(&response));
    let back = store.payload(&r).unwrap().expect("payload present");
    assert_eq!(back, response);
    assert_eq!(hex(&back), r.sha256);
}

#[test]
fn deleted_payload_leaves_audit_event() {
    let (_dir, store, m) = temp_store();
    store
        .record_model_call(&m, record(b"reply".to_vec()))
        .unwrap();
    let Event::ModelCall {
        response: Some(r), ..
    } = model_call(&store, &m)
    else {
        panic!("no response ref");
    };
    store.delete_payload(&r).unwrap();
    assert_eq!(store.payload(&r).unwrap(), None);
    let Event::ModelCall { tokens, .. } = model_call(&store, &m) else {
        unreachable!()
    };
    assert_eq!(tokens.input, Some(1200));
    assert_eq!(tokens.output, Some(300));
}

#[test]
fn old_model_call_without_fields_reads() {
    let body = json!({
        "kind": "model_call",
        "session": 2,
        "role": "worker",
        "provider": "claude-cli",
        "profile": "default",
        "model": "claude-opus-5-5",
        "tokens": {"input": 10, "output": 5, "cache_write": 0},
        "estimated_usd_micros": null,
        "spent": {"kind": "subscription"},
        "millis": 12,
        "request": null,
        "response": null
    });
    let e: Event = serde_json::from_value(body).unwrap();
    let Event::ModelCall { tokens, params, .. } = e else {
        panic!("{e:?}");
    };
    assert_eq!(tokens.cache_read, None);
    assert_eq!(tokens.input, Some(10));
    assert!(params.is_null());
}
