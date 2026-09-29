//! How a worker gets a run's code, and gives its work back, without ever
//! holding a GitHub token.
//!
//! The brain keeps one repository per run and does all the GitHub I/O with the
//! run owner's token. The worker exchanges git bundles with it over HTTP,
//! authenticated by the same bearer key it connects with:
//!
//! * `GET  /api/worker/steps/{step_id}/base.bundle?base=<sha>` -- the history
//!   the step starts from. It is fetched into a cache repository the worker
//!   keeps per `repo_key` (`~/.cortex-worker/repos/{hash}`), and skipped when
//!   the cache already has that commit.
//! * `PUT  /api/worker/steps/{step_id}/head.bundle` -- the commits the step
//!   made (`base..head`), sent BEFORE the worker reports the step complete, so
//!   the brain can verify a commit that is already in its repository.
//!
//! Both routes authorise only the worker the step is leased to.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use cortex_core::protocol::WORKER_ID_HEADER;
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Read/write chunk for streaming a bundle to or from disk.
const CHUNK: usize = 64 * 1024;
/// A whole transfer, however large the bundle.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// The most of an error body worth quoting back.
const ERROR_EXCERPT: usize = 500;

/// Where and as whom to talk to the brain, and where the cache lives.
#[derive(Clone)]
pub struct RepoTransport {
    /// `https://host` -- see [`http_base_from_ws`].
    pub base_url: String,
    /// The worker's bearer key. Never logged.
    pub token: String,
    pub worker_id: String,
    /// The directory that holds `repos/` (the worker's own data dir).
    pub cache_root: PathBuf,
}

/// The repository a step works in, as the brain described it.
#[derive(Clone)]
pub struct StepRepo {
    pub repo_key: String,
    pub base_commit: String,
    pub transport: RepoTransport,
}

/// Why a head upload failed, which decides who is charged.
#[derive(Debug)]
pub enum UploadError {
    /// The brain refused the bundle (over the cap, invalid, unknown lease):
    /// the step's delivery is bad. Reported as a failed step.
    Rejected(String),
    /// The brain or the network could not be reached, or the worker's own git
    /// failed: nothing says the work is bad. Reported as a blocked step.
    Unavailable(String),
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(m) | Self::Unavailable(m) => f.write_str(m),
        }
    }
}

/// FNV-1a over the key's bytes, as 16 hex digits. Hand-written so the cache
/// directory name is stable across builds and needs no dependency.
pub fn hash_repo_key(key: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// `{cache_root}/repos/{hash(repo_key)}`.
pub fn cache_repo_path(cache_root: &Path, repo_key: &str) -> PathBuf {
    cache_root.join("repos").join(hash_repo_key(repo_key))
}

/// The worker's data directory: `CORTEX_WORKER_HOME`, else `~/.cortex-worker`.
pub fn default_cache_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("CORTEX_WORKER_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join(".cortex-worker")
}

/// The brain's HTTP origin from the WebSocket URL the worker connects to:
/// `wss://host/api/ws` -> `https://host`, `ws://brain:3001/ws` ->
/// `http://brain:3001`. `None` for anything that is not a `ws(s)` URL.
pub fn http_base_from_ws(ws_url: &str) -> Option<String> {
    let (scheme, rest) = if let Some(rest) = ws_url.strip_prefix("wss://") {
        ("https", rest)
    } else if let Some(rest) = ws_url.strip_prefix("ws://") {
        ("http", rest)
    } else {
        return None;
    };
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix("/ws").unwrap_or(rest);
    let rest = rest.strip_suffix("/api").unwrap_or(rest);
    if rest.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{rest}"))
}

fn is_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Step ids become ref names and URL segments, so only plain ids pass.
fn is_plain_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn git(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(dir).env("GIT_TERMINAL_PROMPT", "0");
    c
}

fn git_ok(mut cmd: Command, what: &str) -> Result<String, String> {
    let out = cmd
        .output()
        .map_err(|e| format!("could not run git ({what}): {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "git {what} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// True when `repo` holds the commit `sha`.
pub fn has_commit(repo: &Path, sha: &str) -> bool {
    if !is_sha(sha) {
        return false;
    }
    let mut c = git(repo);
    c.args(["cat-file", "-e", &format!("{sha}^{{commit}}")]);
    c.output().is_ok_and(|o| o.status.success())
}

/// Create the bare cache repository if it is not there yet.
pub fn init_bare(repo: &Path) -> Result<(), String> {
    if repo.join("HEAD").is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(repo)
        .map_err(|e| format!("could not create {}: {e}", repo.display()))?;
    let mut c = Command::new("git");
    c.args(["init", "--quiet", "--bare", "--template="])
        .arg(repo)
        .env("GIT_TERMINAL_PROMPT", "0");
    git_ok(c, "init").map(|_| ())
}

/// Fetch the brain's base bundle into the cache and pin `sha` so it is never
/// unreachable. The bundle carries `refs/cortex/dispatch/*`; those are dropped
/// again once the commit is pinned.
pub fn fetch_base_bundle(repo: &Path, bundle: &Path, sha: &str) -> Result<(), String> {
    if !is_sha(sha) {
        return Err("base is not a commit id".into());
    }
    let mut fetch = git(repo);
    fetch
        .args([
            "-c",
            "transfer.fsckObjects=true",
            "fetch",
            "--quiet",
        ])
        .arg(bundle)
        .arg("+refs/cortex/dispatch/*:refs/cortex/dispatch/*");
    git_ok(fetch, "fetch of the base bundle")?;
    if !has_commit(repo, sha) {
        return Err("the base bundle did not contain the base commit".into());
    }
    let mut pin = git(repo);
    pin.args(["update-ref", &format!("refs/cortex/pin/{sha}"), sha]);
    git_ok(pin, "update-ref")?;
    let mut list = git(repo);
    list.args([
        "for-each-ref",
        "--format=%(refname)",
        "refs/cortex/dispatch/",
    ]);
    for name in git_ok(list, "for-each-ref")?.lines() {
        let mut del = git(repo);
        del.args(["update-ref", "-d", name]);
        let _ = del.output();
    }
    Ok(())
}

/// Bundle `base..head` for upload: `head` is put under
/// `refs/cortex/upload/{step_id}` (a raw commit id cannot be bundled, and the
/// brain looks for exactly that ref). The ref stays in the cache, which keeps
/// the step's commits reachable for a later step that starts from them.
pub fn create_head_bundle(
    repo: &Path,
    step_id: &str,
    base: &str,
    head: &str,
    out: &Path,
) -> Result<(), String> {
    if !is_plain_id(step_id) || !is_sha(base) || !is_sha(head) {
        return Err("step id, base or head is not usable".into());
    }
    let upload_ref = format!("refs/cortex/upload/{step_id}");
    let mut set = git(repo);
    set.args(["update-ref", &upload_ref, head]);
    git_ok(set, "update-ref")?;
    let mut bundle = git(repo);
    bundle
        .args(["bundle", "create", "--quiet"])
        .arg(out)
        .arg(&upload_ref)
        .arg(format!("^{base}"));
    git_ok(bundle, "bundle create").map(|_| ())
}

/// Removes a temporary file when dropped.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn temp_bundle(cache_root: &Path) -> Result<TempFile, String> {
    let dir = cache_root.join("tmp");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    Ok(TempFile(dir.join(format!("{}.bundle", uuid::Uuid::new_v4()))))
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TRANSFER_TIMEOUT)
        .build()
        .map_err(|e| format!("could not build the HTTP client: {e}"))
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| format!("git task failed: {e}"))?
}

/// Make sure the cache repository for this step's `repo_key` holds
/// `base_commit`, downloading the base bundle only when it does not. Returns
/// the cache repository, ready for a worktree at `base_commit`.
pub async fn ensure_base(repo: &StepRepo, step_id: &str) -> Result<PathBuf, String> {
    if !is_plain_id(step_id) {
        return Err("step id is not usable".into());
    }
    if !is_sha(&repo.base_commit) {
        return Err("the base commit is not a commit id".into());
    }
    let cache = cache_repo_path(&repo.transport.cache_root, &repo.repo_key);
    {
        let cache = cache.clone();
        blocking(move || init_bare(&cache)).await?;
    }
    {
        let (cache, base) = (cache.clone(), repo.base_commit.clone());
        if blocking(move || Ok(has_commit(&cache, &base))).await? {
            tracing::info!(step_id, "base commit already cached; skipping the download");
            return Ok(cache);
        }
    }

    let t = &repo.transport;
    let tmp = temp_bundle(&t.cache_root)?;
    let url = format!("{}/api/worker/steps/{step_id}/base.bundle", t.base_url);
    let resp = client()?
        .get(&url)
        .query(&[("base", repo.base_commit.as_str())])
        .bearer_auth(&t.token)
        .header(WORKER_ID_HEADER, &t.worker_id)
        .send()
        .await
        .map_err(|e| format!("could not reach the brain for the base bundle: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "the brain refused the base bundle (HTTP {status}): {}",
            excerpt(&body)
        ));
    }
    let mut file = tokio::fs::File::create(&tmp.0)
        .await
        .map_err(|e| format!("could not write the base bundle: {e}"))?;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("the base bundle download broke off: {e}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("could not write the base bundle: {e}"))?;
    }
    file.flush()
        .await
        .map_err(|e| format!("could not write the base bundle: {e}"))?;
    drop(file);

    let (cache2, path, base) = (cache.clone(), tmp.0.clone(), repo.base_commit.clone());
    blocking(move || fetch_base_bundle(&cache2, &path, &base)).await?;
    Ok(cache)
}

/// Send the step's commits (`base..head`) to the brain. Must succeed before
/// the step is reported complete.
pub async fn upload_head(
    repo: &StepRepo,
    step_id: &str,
    cache: &Path,
    base: &str,
    head: &str,
) -> Result<(), UploadError> {
    let t = &repo.transport;
    let tmp = temp_bundle(&t.cache_root).map_err(UploadError::Unavailable)?;
    {
        let (cache, step_id, base, head, out) = (
            cache.to_path_buf(),
            step_id.to_string(),
            base.to_string(),
            head.to_string(),
            tmp.0.clone(),
        );
        blocking(move || create_head_bundle(&cache, &step_id, &base, &head, &out))
            .await
            .map_err(UploadError::Unavailable)?;
    }
    let len = std::fs::metadata(&tmp.0)
        .map_err(|e| UploadError::Unavailable(format!("could not read the head bundle: {e}")))?
        .len();
    let file = tokio::fs::File::open(&tmp.0)
        .await
        .map_err(|e| UploadError::Unavailable(format!("could not read the head bundle: {e}")))?;
    let body = reqwest::Body::wrap_stream(futures_util::stream::unfold(file, |mut f| async move {
        let mut buf = vec![0u8; CHUNK];
        match f.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<_, std::io::Error>(buf), f))
            }
            Err(e) => Some((Err(e), f)),
        }
    }));
    let url = format!("{}/api/worker/steps/{step_id}/head.bundle", t.base_url);
    let resp = client()
        .map_err(UploadError::Unavailable)?
        .put(&url)
        .bearer_auth(&t.token)
        .header(WORKER_ID_HEADER, &t.worker_id)
        .header(reqwest::header::CONTENT_TYPE, "application/x-git-bundle")
        .header(reqwest::header::CONTENT_LENGTH, len)
        .body(body)
        .send()
        .await
        .map_err(|e| UploadError::Unavailable(format!("could not reach the brain: {e}")))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    let msg = format!(
        "the brain did not accept the head bundle (HTTP {status}): {}",
        excerpt(&body)
    );
    if status.is_client_error() {
        Err(UploadError::Rejected(msg))
    } else {
        Err(UploadError::Unavailable(msg))
    }
}

fn excerpt(body: &str) -> String {
    let mut s: String = body.chars().take(ERROR_EXCERPT).collect();
    if s.len() < body.len() {
        s.push_str("...");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cortex-rt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run(dir: &Path, args: &[&str]) -> String {
        let mut c = Command::new("git");
        c.arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args);
        git_ok(c, &args.join(" ")).unwrap()
    }

    /// A working repository with two commits; returns (dir, first, second).
    fn source() -> (PathBuf, String, String) {
        let dir = scratch();
        run(&dir, &["init", "--quiet", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "--quiet", "-m", "first"]);
        let first = run(&dir, &["rev-parse", "HEAD"]);
        std::fs::write(dir.join("a.txt"), "two").unwrap();
        run(&dir, &["commit", "--quiet", "-am", "second"]);
        let second = run(&dir, &["rev-parse", "HEAD"]);
        (dir, first, second)
    }

    /// The bundle the brain would serve for `sha`: a temporary dispatch ref.
    fn base_bundle(src: &Path, sha: &str) -> PathBuf {
        run(src, &["update-ref", "refs/cortex/dispatch/x", sha]);
        let out = scratch().join("base.bundle");
        run(
            src,
            &["bundle", "create", "--quiet", out.to_str().unwrap(), "refs/cortex/dispatch/x"],
        );
        run(src, &["update-ref", "-d", "refs/cortex/dispatch/x"]);
        out
    }

    #[test]
    fn repo_key_hash_is_stable_and_distinct() {
        assert_eq!(hash_repo_key("acme/widgets"), hash_repo_key("acme/widgets"));
        assert_ne!(hash_repo_key("acme/widgets"), hash_repo_key("acme/gadgets"));
        assert_eq!(hash_repo_key("").len(), 16);
        // FNV-1a's published offset basis for the empty input.
        assert_eq!(hash_repo_key(""), "cbf29ce484222325");
        let root = Path::new("/w");
        assert_eq!(
            cache_repo_path(root, "acme/widgets"),
            root.join("repos").join(hash_repo_key("acme/widgets"))
        );
    }

    #[test]
    fn the_brain_origin_comes_from_the_websocket_url() {
        assert_eq!(
            http_base_from_ws("wss://api.heyvera.org/api/ws").as_deref(),
            Some("https://api.heyvera.org")
        );
        assert_eq!(
            http_base_from_ws("ws://brain:3001/api/ws").as_deref(),
            Some("http://brain:3001")
        );
        assert_eq!(
            http_base_from_ws("ws://brain:3001/ws").as_deref(),
            Some("http://brain:3001")
        );
        assert_eq!(
            http_base_from_ws("wss://h.example/api/ws?x=1").as_deref(),
            Some("https://h.example")
        );
        assert_eq!(http_base_from_ws("https://h.example/api/ws"), None);
        assert_eq!(http_base_from_ws("wss:///ws"), None);
    }

    #[test]
    fn the_base_bundle_is_cached_and_pinned() {
        let (src, first, second) = source();
        let bundle = base_bundle(&src, &first);
        let cache = scratch().join("cache.git");
        init_bare(&cache).unwrap();
        init_bare(&cache).unwrap();
        assert!(!has_commit(&cache, &first));
        fetch_base_bundle(&cache, &bundle, &first).unwrap();
        assert!(has_commit(&cache, &first));
        assert!(!has_commit(&cache, &second), "only the base was bundled");
        let refs = run(&cache, &["for-each-ref", "--format=%(refname)"]);
        assert_eq!(refs, format!("refs/cortex/pin/{first}"));
        // A bundle that lacks the wanted commit is refused.
        let other = scratch().join("other.git");
        init_bare(&other).unwrap();
        assert!(fetch_base_bundle(&other, &bundle, &second).is_err());
    }

    #[tokio::test]
    async fn a_cached_base_skips_the_download() {
        let (src, first, _) = source();
        let bundle = base_bundle(&src, &first);
        let root = scratch();
        let repo = StepRepo {
            repo_key: "acme/widgets".into(),
            base_commit: first.clone(),
            // Nothing listens here: any download attempt fails the test.
            transport: RepoTransport {
                base_url: "http://127.0.0.1:1".into(),
                token: "t".into(),
                worker_id: "w".into(),
                cache_root: root.clone(),
            },
        };
        let cache = cache_repo_path(&root, "acme/widgets");
        init_bare(&cache).unwrap();
        fetch_base_bundle(&cache, &bundle, &first).unwrap();
        let got = ensure_base(&repo, "step-1").await.expect("cache hit");
        assert_eq!(got, cache);
        // A commit the cache lacks does go to the brain (and fails here).
        let miss = StepRepo {
            base_commit: "0".repeat(40),
            ..repo
        };
        let err = ensure_base(&miss, "step-1").await.unwrap_err();
        assert!(err.contains("could not reach the brain"), "{err}");
    }

    #[test]
    fn the_head_bundle_carries_only_the_new_commits_under_the_upload_ref() {
        let (src, first, second) = source();
        let out = scratch().join("head.bundle");
        create_head_bundle(&src, "step-1", &first, &second, &out).unwrap();
        // A repository that has the base can verify and fetch it.
        let run_repo = scratch().join("run.git");
        init_bare(&run_repo).unwrap();
        let bundle = base_bundle(&src, &first);
        fetch_base_bundle(&run_repo, &bundle, &first).unwrap();
        run(&run_repo, &["bundle", "verify", out.to_str().unwrap()]);
        run(
            &run_repo,
            &[
                "fetch",
                "--quiet",
                out.to_str().unwrap(),
                "+refs/cortex/upload/step-1:refs/cortex/step/step-1",
            ],
        );
        assert_eq!(run(&run_repo, &["rev-parse", "refs/cortex/step/step-1"]), second);
        // The cache keeps the upload ref so the commits stay reachable.
        assert_eq!(run(&src, &["rev-parse", "refs/cortex/upload/step-1"]), second);
        // Bad input is refused before git runs.
        assert!(create_head_bundle(&src, "bad id", &first, &second, &out).is_err());
        assert!(create_head_bundle(&src, "step-1", "nope", &second, &out).is_err());
    }
}
