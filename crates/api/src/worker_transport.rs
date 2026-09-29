//! The two HTTP routes a worker uses to exchange code with the server.
//!
//! Workers never hold a GitHub token. The server fetches a run's repository
//! (`run_repo`), and a worker only ever talks to the server:
//!
//! - `GET /api/worker/steps/{step_id}/base.bundle?base=<sha>` streams a git
//!   bundle holding `base`'s history, for the worker to create the step's
//!   worktree from.
//! - `PUT /api/worker/steps/{step_id}/head.bundle` takes the bundle of what
//!   the step committed, verifies it, and fetches it into the run repository
//!   under `refs/cortex/step/{step_id}` so verification and the pull request
//!   read it from there.
//!
//! Both routes take the worker's bearer credential (the same one its
//! WebSocket registration uses) plus `x-cortex-worker-id`, and only answer the
//! worker the step is currently leased to.
//!
//! Who pays when the upload fails follows the rest of settlement. A bundle
//! that is unusable (over the cap, corrupt, failing fsck, missing the head)
//! is the worker's doing: `4xx`, and the worker reports the step failed, which
//! is charged like any failed attempt. Cortex's own failure to run git or to
//! write its own disk is `5xx`, and the route itself ends the attempt as
//! `CortexCrash`, which is absorbed.

use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use cortex_core::billing_binding::AttemptEndCause;
use cortex_core::protocol::WORKER_ID_HEADER;
use cortex_engine::captain::SchedulerEvent;
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::db::StepTransportInfo;
use crate::run_repo::{self, BundleError};
use crate::state::AppState;

/// Default cap on an uploaded head bundle: 256 MiB.
pub const DEFAULT_MAX_BUNDLE_BYTES: u64 = 256 * 1024 * 1024;

/// The configured cap on an uploaded head bundle (`CORTEX_MAX_BUNDLE_BYTES`).
/// Enforced on the head route only; the rest of the API keeps its own limit.
pub fn max_bundle_bytes() -> u64 {
    parse_cap(std::env::var("CORTEX_MAX_BUNDLE_BYTES").ok().as_deref())
}

fn parse_cap(raw: Option<&str>) -> u64 {
    raw.and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_MAX_BUNDLE_BYTES)
}

/// A file that is removed when dropped, so a failed or abandoned transfer
/// leaves nothing behind.
struct TempFile(PathBuf);

impl TempFile {
    fn new(dir: &FsPath) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self(
            dir.join(format!("transfer-{}.tmp", uuid::Uuid::new_v4())),
        ))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A worker that has been authenticated and shown to hold the step's lease.
struct Leased {
    info: StepTransportInfo,
    repo: PathBuf,
}

fn reply(status: StatusCode, message: impl Into<String>) -> Response {
    (status, message.into()).into_response()
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))?;
    Some(token.trim().to_string())
}

/// Authenticate the bearer credential, then check the lease.
async fn authorize(
    state: &AppState,
    step_id: &str,
    headers: &HeaderMap,
) -> Result<Leased, Response> {
    let token = bearer(headers).unwrap_or_default();
    let user_id = crate::ws::authenticate_worker(state, &token)
        .await
        .map_err(|_| reply(StatusCode::UNAUTHORIZED, "invalid worker credential"))?;
    authorize_user(state, &user_id, step_id, headers)
}

/// The lease check, for a user that is already authenticated: the named worker
/// must belong to that user and hold a live lease on the step.
fn authorize_user(
    state: &AppState,
    user_id: &str,
    step_id: &str,
    headers: &HeaderMap,
) -> Result<Leased, Response> {
    let Some(db) = state.db.as_ref() else {
        return Err(reply(
            StatusCode::SERVICE_UNAVAILABLE,
            "database not available",
        ));
    };
    let worker_id = headers
        .get(WORKER_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| reply(StatusCode::FORBIDDEN, "missing worker id"))?;
    if db.worker_user_id(worker_id).as_deref() != Some(user_id) {
        return Err(reply(
            StatusCode::FORBIDDEN,
            "worker does not belong to you",
        ));
    }
    let info = db
        .step_transport_info(step_id)
        .ok_or_else(|| reply(StatusCode::NOT_FOUND, "unknown step"))?;
    if info.worker_id != worker_id || !db.verify_step_worker(step_id, worker_id) {
        return Err(reply(
            StatusCode::FORBIDDEN,
            "step is not leased to this worker",
        ));
    }
    let repo = run_repo::run_repo_path(&state.workspace_dir, &info.run_id)
        .ok_or_else(|| reply(StatusCode::NOT_FOUND, "unknown run"))?;
    Ok(Leased { info, repo })
}

/// Stream a file's bytes, keeping `guard` (and so the file) alive until the
/// last byte is sent.
fn stream_file(file: tokio::fs::File, guard: TempFile) -> Body {
    let stream = futures_util::stream::unfold((file, guard), |(mut file, guard)| async move {
        let mut buf = vec![0u8; 64 * 1024];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<Vec<u8>, std::io::Error>(buf), (file, guard)))
            }
            Err(e) => Some((Err(e), (file, guard))),
        }
    });
    Body::from_stream(stream)
}

/// `GET /api/worker/steps/{step_id}/base.bundle?base=<sha>`
pub async fn get_base_bundle(
    State(state): State<Arc<AppState>>,
    Path(step_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let leased = match authorize(&state, &step_id, &headers).await {
        Ok(leased) => leased,
        Err(response) => return response,
    };
    serve_base(&state, &leased, query.get("base").map(String::as_str)).await
}

async fn serve_base(state: &AppState, leased: &Leased, base: Option<&str>) -> Response {
    let Some(base) = base.map(str::trim).filter(|b| !b.is_empty()) else {
        return reply(StatusCode::BAD_REQUEST, "missing base commit");
    };
    let temp = match TempFile::new(&run_repo::runs_root(&state.workspace_dir)) {
        Ok(temp) => temp,
        Err(e) => {
            tracing::error!(error = %e, "could not prepare a base bundle file");
            return reply(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not prepare bundle",
            );
        }
    };
    let repo = leased.repo.clone();
    let base = base.to_string();
    let out = temp.0.clone();
    let made =
        tokio::task::spawn_blocking(move || run_repo::create_base_bundle(&repo, &base, &out))
            .await
            .unwrap_or_else(|e| {
                Err(BundleError::Ours(format!(
                    "bundle task did not complete: {e}"
                )))
            });
    match made {
        Ok(()) => {}
        Err(BundleError::NotFound(why)) | Err(BundleError::Worker(why)) => {
            return reply(StatusCode::NOT_FOUND, why);
        }
        Err(BundleError::Ours(why)) => {
            tracing::error!(run_id = %leased.info.run_id, error = %why, "base bundle failed");
            return reply(StatusCode::INTERNAL_SERVER_ERROR, "could not build bundle");
        }
    }
    let file = match tokio::fs::File::open(&temp.0).await {
        Ok(file) => file,
        Err(e) => {
            tracing::error!(error = %e, "could not open the base bundle");
            return reply(StatusCode::INTERNAL_SERVER_ERROR, "could not read bundle");
        }
    };
    let len = file.metadata().await.map(|m| m.len()).ok();
    let mut response = Response::new(stream_file(file, temp));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/octet-stream"),
    );
    if let Some(len) = len {
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, len.into());
    }
    response
}

/// `PUT /api/worker/steps/{step_id}/head.bundle`
pub async fn put_head_bundle(
    State(state): State<Arc<AppState>>,
    Path(step_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let leased = match authorize(&state, &step_id, &headers).await {
        Ok(leased) => leased,
        Err(response) => return response,
    };
    ingest_head(
        &state,
        &leased,
        &step_id,
        &headers,
        body,
        max_bundle_bytes(),
    )
    .await
}

async fn ingest_head(
    state: &AppState,
    leased: &Leased,
    step_id: &str,
    headers: &HeaderMap,
    body: Body,
    cap: u64,
) -> Response {
    // Refuse early when the worker announces a size over the cap; the count
    // below is what actually enforces it.
    let announced = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if announced.is_some_and(|len| len > cap) {
        return reply(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("bundle is larger than the {cap} byte limit"),
        );
    }

    let temp = match TempFile::new(&run_repo::runs_root(&state.workspace_dir)) {
        Ok(temp) => temp,
        Err(e) => {
            return our_failure(
                state,
                leased,
                step_id,
                &format!("could not prepare a file: {e}"),
            )
            .await;
        }
    };
    let mut file = match tokio::fs::File::create(&temp.0).await {
        Ok(file) => file,
        Err(e) => {
            return our_failure(
                state,
                leased,
                step_id,
                &format!("could not create a file: {e}"),
            )
            .await;
        }
    };
    let mut stream = body.into_data_stream();
    let mut received: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(e) => {
                return reply(
                    StatusCode::BAD_REQUEST,
                    format!("upload was interrupted: {e}"),
                );
            }
        };
        received = received.saturating_add(chunk.len() as u64);
        if received > cap {
            return reply(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("bundle is larger than the {cap} byte limit"),
            );
        }
        if let Err(e) = file.write_all(&chunk).await {
            return our_failure(
                state,
                leased,
                step_id,
                &format!("could not write a file: {e}"),
            )
            .await;
        }
    }
    if let Err(e) = file.flush().await {
        return our_failure(
            state,
            leased,
            step_id,
            &format!("could not write a file: {e}"),
        )
        .await;
    }
    drop(file);
    if received == 0 {
        return reply(StatusCode::UNPROCESSABLE_ENTITY, "the bundle is empty");
    }

    let repo = leased.repo.clone();
    let id = step_id.to_string();
    let path = temp.0.clone();
    let ingested =
        tokio::task::spawn_blocking(move || run_repo::ingest_head_bundle(&repo, &id, &path))
            .await
            .unwrap_or_else(|e| {
                Err(BundleError::Ours(format!(
                    "ingest task did not complete: {e}"
                )))
            });
    match ingested {
        Ok(head) => (
            StatusCode::OK,
            axum::Json(serde_json::json!({ "head": head })),
        )
            .into_response(),
        Err(BundleError::Worker(why)) | Err(BundleError::NotFound(why)) => {
            tracing::warn!(step_id, error = %why, "worker head bundle rejected");
            reply(StatusCode::UNPROCESSABLE_ENTITY, why)
        }
        Err(BundleError::Ours(why)) => our_failure(state, leased, step_id, &why).await,
    }
}

/// Cortex's own failure while taking a worker's code: end the attempt as
/// `CortexCrash` (absorbed, never charged) and tell the scheduler, then answer
/// `500`. The worker's own report that follows finds the step already ended.
async fn our_failure(state: &AppState, leased: &Leased, step_id: &str, reason: &str) -> Response {
    tracing::error!(
        step_id,
        error = %reason,
        "could not take a worker's head bundle; Cortex's own fault, absorbed"
    );
    if let Some(db) = state.db.as_ref() {
        let owned = db.worker_owned_by_cortex(&leased.info.worker_id);
        if db.record_execution_failure_and_end(
            step_id,
            &leased.info.attempt_id,
            leased.info.lease_gen,
            &format!("could not take the worker's code: {reason}"),
            AttemptEndCause::CortexCrash,
            owned,
        ) {
            if let Err(e) = db.settle_pending_attempts() {
                tracing::error!(
                    step_id,
                    error = %e,
                    "settle_pending_attempts failed after a head bundle failure; the \
                     scheduler's next tick will retry"
                );
            }
            state
                .emit_scheduler_event(SchedulerEvent::StepFailed {
                    run_id: leased.info.run_id.clone(),
                    step_id: step_id.to_string(),
                })
                .await;
        } else {
            tracing::warn!(
                step_id,
                "record_execution_failure_and_end returned false for a head bundle \
                 failure; likely a stale lease"
            );
        }
    }
    reply(
        StatusCode::INTERNAL_SERVER_ERROR,
        "could not store the bundle",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "user-fixture";

    async fn state() -> Arc<AppState> {
        let temporary = tempfile::tempdir().expect("temporary workspace");
        let workspace = temporary.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".cortex")).expect("workspace metadata");
        let state = AppState::new(workspace.join(".cortex/ledger.jsonl"), workspace, None).await;
        std::mem::forget(temporary);
        state
    }

    fn git(dir: &FsPath, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    struct Fixture {
        state: Arc<AppState>,
        worker_id: String,
        step_id: String,
        repo: PathBuf,
        base: String,
    }

    /// A leased step whose run has a real bare repository holding one commit.
    async fn fixture() -> Fixture {
        let state = state().await;
        let db = state.db.as_ref().expect("database");
        let worker_id = format!("w-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        db.register_worker(&worker_id, OWNER, false);
        let run_id = db.create_run(OWNER, "goal", "default", &[]);
        let step_id = db.create_step(&run_id, "execute", "strong", "low", "do the thing");
        db.lease_step(
            &step_id,
            &worker_id,
            chrono::Utc::now().timestamp_millis() + 60_000,
            "attempt-transport",
        )
        .expect("lease");

        let repo = run_repo::run_repo_path(&state.workspace_dir, &run_id).expect("run id");
        std::fs::create_dir_all(&repo).expect("repo dir");
        git(&repo, &["init", "--bare"]);
        // Make a commit through a scratch clone.
        let scratch = state.workspace_dir.join(format!("scratch-{run_id}"));
        std::fs::create_dir_all(&scratch).expect("scratch");
        git(&scratch, &["init"]);
        std::fs::write(scratch.join("a.txt"), "one\n").expect("write");
        git(&scratch, &["add", "."]);
        git(&scratch, &["commit", "-m", "base"]);
        let base = git(&scratch, &["rev-parse", "HEAD"]);
        git(
            &scratch,
            &[
                "push",
                repo.to_str().expect("utf8"),
                "HEAD:refs/cortex/base",
            ],
        );
        Fixture {
            state,
            worker_id,
            step_id,
            repo,
            base,
        }
    }

    fn headers(worker_id: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(WORKER_ID_HEADER, worker_id.parse().expect("header"));
        h
    }

    async fn body_bytes(response: Response) -> Vec<u8> {
        use http_body_util::BodyExt;
        response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes()
            .to_vec()
    }

    #[test]
    fn cap_defaults_and_parses() {
        assert_eq!(parse_cap(None), DEFAULT_MAX_BUNDLE_BYTES);
        assert_eq!(parse_cap(Some("nonsense")), DEFAULT_MAX_BUNDLE_BYTES);
        assert_eq!(parse_cap(Some("0")), DEFAULT_MAX_BUNDLE_BYTES);
        assert_eq!(parse_cap(Some(" 1024 ")), 1024);
    }

    #[tokio::test]
    async fn only_the_leased_worker_of_the_owner_passes() {
        let f = fixture().await;
        let ok = authorize_user(&f.state, OWNER, &f.step_id, &headers(&f.worker_id));
        assert!(ok.is_ok());

        // Another user's credential, same worker id.
        let wrong_user =
            authorize_user(&f.state, "someone-else", &f.step_id, &headers(&f.worker_id));
        assert_eq!(
            wrong_user.err().expect("refused").status(),
            StatusCode::FORBIDDEN
        );

        // Another worker of the same user that does not hold the lease.
        let db = f.state.db.as_ref().expect("database");
        db.register_worker("w-other", OWNER, false);
        let other = authorize_user(&f.state, OWNER, &f.step_id, &headers("w-other"));
        assert_eq!(
            other.err().expect("refused").status(),
            StatusCode::FORBIDDEN
        );

        // No worker id at all.
        let none = authorize_user(&f.state, OWNER, &f.step_id, &HeaderMap::new());
        assert_eq!(none.err().expect("refused").status(), StatusCode::FORBIDDEN);

        // A step that does not exist.
        let unknown = authorize_user(&f.state, OWNER, "no-such-step", &headers(&f.worker_id));
        assert_eq!(
            unknown.err().expect("refused").status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn base_bundle_carries_the_base_commit() {
        let f = fixture().await;
        let leased = authorize_user(&f.state, OWNER, &f.step_id, &headers(&f.worker_id))
            .ok()
            .expect("leased");
        let response = serve_base(&f.state, &leased, Some(&f.base)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = body_bytes(response).await;
        assert!(bytes.starts_with(b"# v2 git bundle") || bytes.starts_with(b"# v3 git bundle"));

        // The bundle really contains the base: clone it.
        let out = f.state.workspace_dir.join("cloned");
        let bundle = f.state.workspace_dir.join("base.bundle");
        std::fs::write(&bundle, &bytes).expect("write");
        std::fs::create_dir_all(&out).expect("dir");
        git(&out, &["init"]);
        git(
            &out,
            &[
                "fetch",
                bundle.to_str().expect("utf8"),
                "refs/cortex/dispatch/*:refs/cortex/dispatch/*",
            ],
        );
        git(&out, &["cat-file", "-e", &format!("{}^{{commit}}", f.base)]);

        // An unknown base is a 404, and a missing one a 400.
        let unknown = serve_base(&f.state, &leased, Some(&"0".repeat(40))).await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        let missing = serve_base(&f.state, &leased, None).await;
        assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    }

    /// A worker-side bundle of one new commit on top of `f.base`, under the
    /// upload ref the server insists on.
    fn worker_bundle(f: &Fixture) -> (Vec<u8>, String) {
        let work = f
            .state
            .workspace_dir
            .join(format!("work-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&work).expect("dir");
        git(&work, &["init"]);
        git(
            &work,
            &[
                "fetch",
                f.repo.to_str().expect("utf8"),
                "refs/cortex/base:refs/cortex/base",
            ],
        );
        git(&work, &["checkout", "-q", "-b", "step", &f.base]);
        std::fs::write(work.join("b.txt"), "two\n").expect("write");
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "step"]);
        let head = git(&work, &["rev-parse", "HEAD"]);
        let upload = run_repo::upload_ref(&f.step_id);
        git(&work, &["update-ref", &upload, &head]);
        let bundle = work.join("head.bundle");
        git(
            &work,
            &[
                "bundle",
                "create",
                bundle.to_str().expect("utf8"),
                &upload,
                &format!("^{}", f.base),
            ],
        );
        (std::fs::read(&bundle).expect("read"), head)
    }

    #[tokio::test]
    async fn head_bundle_lands_under_the_step_ref() {
        let f = fixture().await;
        let leased = authorize_user(&f.state, OWNER, &f.step_id, &headers(&f.worker_id))
            .ok()
            .expect("leased");
        let (bytes, head) = worker_bundle(&f);
        let response = ingest_head(
            &f.state,
            &leased,
            &f.step_id,
            &HeaderMap::new(),
            Body::from(bytes),
            DEFAULT_MAX_BUNDLE_BYTES,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let resolved = git(
            &f.repo,
            &[
                "rev-parse",
                &format!("{}^{{commit}}", run_repo::step_ref(&f.step_id)),
            ],
        );
        assert_eq!(resolved, head);
    }

    #[tokio::test]
    async fn oversized_and_corrupt_bundles_are_the_workers_fault() {
        let f = fixture().await;
        let leased = authorize_user(&f.state, OWNER, &f.step_id, &headers(&f.worker_id))
            .ok()
            .expect("leased");
        let (bytes, _) = worker_bundle(&f);

        let over = ingest_head(
            &f.state,
            &leased,
            &f.step_id,
            &HeaderMap::new(),
            Body::from(bytes.clone()),
            16,
        )
        .await;
        assert_eq!(over.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let corrupt = ingest_head(
            &f.state,
            &leased,
            &f.step_id,
            &HeaderMap::new(),
            Body::from(b"this is not a bundle".to_vec()),
            DEFAULT_MAX_BUNDLE_BYTES,
        )
        .await;
        assert_eq!(corrupt.status(), StatusCode::UNPROCESSABLE_ENTITY);

        // Neither was Cortex's fault, so the attempt was not ended by the route.
        let db = f.state.db.as_ref().expect("database");
        assert!(db.verify_step_worker(&f.step_id, &f.worker_id));
        let ended: Option<String> = db
            .conn()
            .query_row(
                "SELECT cause FROM attempt_endings WHERE attempt_id = 'attempt-transport'",
                [],
                |row| row.get(0),
            )
            .ok();
        assert!(
            ended.is_none(),
            "a worker's bad bundle must not be absorbed"
        );
    }
}
