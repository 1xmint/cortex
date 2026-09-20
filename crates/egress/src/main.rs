//! The egress mediator: a `CONNECT` proxy that reaches exactly the hosts it
//! was started with.
//!
//! One of these runs per attempt, on that attempt's internal network, and is
//! torn down with it. It is the only host on that network with a route off the
//! machine, which is what makes the allowlist enforcement rather than advice —
//! see `docs/adr/ADR-0002-egress-mediation.md`.
//!
//! What it deliberately is not:
//!
//! - **Not a TLS terminator.** It reads a `CONNECT host:port` line, opens a
//!   socket, and copies bytes. It never sees plaintext, so it can never leak
//!   plaintext, and no certificate authority has to be trusted by the task.
//! - **Not an HTTP proxy.** A non-`CONNECT` request is answered `405` rather
//!   than forwarded. Absolute-URI forwarding means parsing and re-emitting
//!   headers, keep-alive, and chunked bodies, which is a lot of protocol
//!   surface in the one component that sits between a task and the internet.
//! - **Not a credential holder.** Its entire configuration is an allowlist on
//!   argv. There is nothing here for a task to steal.
//!
//! Usage: `cortex-egress --listen 0.0.0.0:3128 --allow registry.npmjs.org:443`

use std::collections::HashSet;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// How long a client has to send its request line before it is dropped. A
/// connection that opens and says nothing is either broken or probing.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How long to wait for the upstream host to accept a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest request line accepted. A `CONNECT` line is tens of bytes; this
/// is generous and still bounds the memory a client can make us allocate.
const MAX_REQUEST_LINE: u64 = 8 * 1024;

/// Printed once the listener is bound. The runner waits for this line before
/// starting the sandbox, so a task never races the mediator.
const READY_MARKER: &str = "cortex-egress ready";

struct Config {
    listen: SocketAddr,
    /// Exactly the `host:port` pairs that may be reached. Matching is exact:
    /// no wildcards, no suffixes, and the port is part of the key because a
    /// host entry without one is an allowlist for every service on that host.
    allow: HashSet<String>,
}

fn parse_args() -> Result<Config, String> {
    let mut listen = "0.0.0.0:3128".to_string();
    let mut allow = HashSet::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => {
                listen = args.next().ok_or("--listen needs an address")?;
            }
            "--allow" => {
                let value = args.next().ok_or("--allow needs a host:port")?;
                // Accept a comma-separated list so the whole allowlist can be
                // one argv entry, which is easier to read in a process table.
                for entry in value.split(',') {
                    let entry = entry.trim();
                    if entry.is_empty() {
                        continue;
                    }
                    if !entry.contains(':') {
                        return Err(format!(
                            "allowlist entry {entry:?} has no port; a host without a port \
                             permits every service on that host"
                        ));
                    }
                    allow.insert(entry.to_string());
                }
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let listen = listen
        .parse()
        .map_err(|e| format!("could not parse listen address: {e}"))?;
    Ok(Config { listen, allow })
}

#[tokio::main]
async fn main() -> ExitCode {
    let config = match parse_args() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("cortex-egress: {err}");
            return ExitCode::FAILURE;
        }
    };

    // An empty allowlist is a configuration mistake, not a strict policy: the
    // caller only starts a mediator when something was granted. Starting one
    // that can reach nothing would look like a working proxy and fail every
    // request, which is the confusing failure rather than the loud one.
    if config.allow.is_empty() {
        eprintln!("cortex-egress: refusing to start with an empty allowlist");
        return ExitCode::FAILURE;
    }

    let listener = match TcpListener::bind(config.listen).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("cortex-egress: could not bind {}: {err}", config.listen);
            return ExitCode::FAILURE;
        }
    };

    let mut sorted: Vec<&String> = config.allow.iter().collect();
    sorted.sort();
    println!("{READY_MARKER} on {} allowing {sorted:?}", config.listen);

    let allow = std::sync::Arc::new(config.allow);
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(err) => {
                eprintln!("cortex-egress: accept failed: {err}");
                continue;
            }
        };
        let allow = allow.clone();
        tokio::spawn(async move {
            if let Err(err) = serve(stream, &allow).await {
                eprintln!("cortex-egress: {peer}: {err}");
            }
        });
    }
}

async fn serve(client: TcpStream, allow: &HashSet<String>) -> Result<(), String> {
    let mut reader = BufReader::new(client);

    let mut request_line = String::new();
    let read = tokio::time::timeout(
        REQUEST_TIMEOUT,
        (&mut reader)
            .take(MAX_REQUEST_LINE)
            .read_line(&mut request_line),
    )
    .await;
    match read {
        Ok(Ok(0)) => return Ok(()),
        Ok(Ok(_)) => {}
        Ok(Err(err)) => return Err(format!("read failed: {err}")),
        Err(_) => return Err("client sent no request line".to_string()),
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();

    if !method.eq_ignore_ascii_case("CONNECT") {
        // Told no, explicitly. A client that gets no answer hangs until its own
        // timeout and reports something that looks like a network fault; this
        // reports what actually happened.
        respond(
            reader.get_mut(),
            "405 Method Not Allowed",
            "this proxy tunnels CONNECT only\n",
        )
        .await;
        return Err(format!("refused a {method} request"));
    }

    // The target of a CONNECT is authority-form: `host:port`, already exactly
    // the shape of an allowlist key.
    let target = target.to_ascii_lowercase();
    if !allow.contains(&target) {
        respond(
            reader.get_mut(),
            "403 Forbidden",
            "host is not on this task's egress allowlist\n",
        )
        .await;
        return Err(format!("denied {target}"));
    }

    // Drain the rest of the request head. Nothing in it is consulted — in
    // particular the Host header is ignored, because it is client-controlled
    // and matching on it would let a task name one host in CONNECT and reach
    // another.
    loop {
        let mut line = String::new();
        let n = (&mut reader)
            .take(MAX_REQUEST_LINE)
            .read_line(&mut line)
            .await
            .map_err(|e| format!("read failed: {e}"))?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }

    // We resolve the name, not the client. A task never gets to say which
    // address a permitted name points at, which is DNS rebinding closed
    // without a special case for it.
    let upstream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&target))
        .await
        .map_err(|_| format!("upstream {target} timed out"))?
        .map_err(|e| format!("upstream {target} refused: {e}"))?;

    let mut client = reader.into_inner();
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await
        .map_err(|e| format!("could not acknowledge the tunnel: {e}"))?;

    let (mut client_read, mut client_write) = client.into_split();
    let (mut upstream_read, mut upstream_write) = upstream.into_split();
    let up = tokio::io::copy(&mut client_read, &mut upstream_write);
    let down = tokio::io::copy(&mut upstream_read, &mut client_write);
    let _ = tokio::join!(up, down);
    Ok(())
}

async fn respond(client: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = client.write_all(response.as_bytes()).await;
    let _ = client.shutdown().await;
}
