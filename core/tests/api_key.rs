//! The API key goes out in a request header and nowhere else: not in the
//! transcript, not in a failure note, not in debug output. A local server
//! stands in for the API and echoes the whole request, key included, back
//! in its error, which is the worst case the engine must clean up.
//!
//! This file holds one test so setting the environment variable is safe.

use cortex::engine::api::{ApiEngine, HttpTransport};
use cortex::engine::{CancelToken, Engine, SessionInput, SessionOutcome};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

const KEY: &str = "sk-ant-test-0123456789-never-show";

/// Reads one HTTP request: the headers, then `content-length` bytes.
fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).into_owned();
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .lines()
                .find_map(|l| {
                    let (name, value) = l.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if buf.len() >= end + 4 + length {
                return text;
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[test]
fn api_key_never_in_transcript() {
    // SAFETY: the only test in this binary, so no other thread reads the
    // environment.
    unsafe { std::env::set_var("ANTHROPIC_API_KEY", KEY) };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream);
        let body = serde_json::json!({"type": "error", "error": {
            "type": "invalid_request_error", "message": request}})
        .to_string();
        write!(
            stream,
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        request
    });

    let transport = HttpTransport::from_env().unwrap().at(&url);
    assert!(!format!("{transport:?}").contains(KEY));
    let mut engine = ApiEngine::new(transport);
    let dir = tempfile::tempdir().unwrap();
    let input = SessionInput {
        brief: "say hi".into(),
        worktree: dir.path().to_path_buf(),
        checks_dir: dir.path().join("no-checks"),
        session: 1,
        timeout: Duration::from_secs(60),
    };
    let mut lines = Vec::new();
    let outcome = engine
        .run(
            &input,
            &mut |l| lines.push(l.to_string()),
            &CancelToken::new(),
        )
        .unwrap();

    let request = server.join().unwrap();
    // The key really was sent, so the checks below can fail.
    assert!(request.contains(KEY), "{request}");
    let SessionOutcome::Failed { reason } = &outcome else {
        panic!("{outcome:?}");
    };
    assert!(reason.contains("invalid_request_error"), "{reason}");
    assert!(!reason.contains(KEY), "{reason}");
    assert!(!lines.is_empty());
    for line in &lines {
        assert!(!line.contains(KEY), "{line}");
    }
}
