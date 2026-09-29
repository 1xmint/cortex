//! The per-run repository: the one place a run's code lives on the server.
//!
//! A run's `repo_key` names the repository the run works on. At run start the
//! server fetches that repository into a bare repository of its own,
//! `{runs dir}/{run_id}.git`, using the run owner's GitHub OAuth token (the
//! token never leaves this process: workers hold none, they exchange git
//! bundles with the server over HTTP, see `worker_transport`). Everything
//! downstream reads or writes this repository and nothing else: dispatch
//! (the base commit and the ecosystem probes), verification (the delivered
//! commit and its object view) and the pull request push.
//!
//! `repo_key` is `owner/repo` (GitHub) or, outside production only,
//! `local:<absolute path>` (fetched from that path; tests and local
//! development).
//!
//! All git here runs on the host with the hardened environment of
//! `verification_driver::control_git` (no hooks, no system or global
//! configuration, no prompts) under `run_git` deadlines. What is whose fault
//! is decided the same way as in verification: only a git that cannot be
//! spawned, or our own filesystem failing, is Cortex's (never charged); the
//! worker's bytes being bad is the worker's (charged like any failed
//! attempt); GitHub being down is neither, and no run exists yet to charge.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::verification_driver::{control_git, run_git, GitRun};

/// Manifest files whose presence at the base commit steers dispatch (check
/// specs, egress) and verification (path classification). They are copied out
/// of the base tree once, at run start, into `{run_id}.manifests`, so nothing
/// after that has to check a tree out.
pub const MANIFEST_FILES: [&str; 5] = [
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "requirements.txt",
    "go.mod",
];
const MANIFEST_MAX_BYTES: usize = 1 << 20;

/// A fetch of a whole repository.
const FETCH_DEADLINE: Duration = Duration::from_secs(600);
/// Everything else that only touches the run repository.
const QUICK_DEADLINE: Duration = Duration::from_secs(60);
/// Bundling and ingesting can move a lot of bytes.
const BUNDLE_DEADLINE: Duration = Duration::from_secs(300);

/// Run repositories this long after their run is terminal are deleted.
pub const RUN_REPO_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// A half-made staging directory this old belongs to a process that died.
const STALE_PENDING_AGE: Duration = Duration::from_secs(2 * 60 * 60);

/// The ref the base commit is kept under, so the objects are never pruned.
const BASE_REF: &str = "refs/cortex/base";
/// Plain file holding the base commit id, so dispatch reads it without git.
const BASE_FILE: &str = "cortex-base";

// --- Locations -------------------------------------------------------------

/// Directory holding every run repository: `runs/` next to the Cortex
/// database, which is the server's persistent data directory (it honours
/// `CORTEX_DB_PATH`, and in production is outside the deploy-reset checkout).
pub fn runs_root(workspace_dir: &Path) -> PathBuf {
    match crate::state::cortex_db_path(workspace_dir).parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join("runs"),
        _ => workspace_dir.join(".cortex").join("runs"),
    }
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// `{runs dir}/{run_id}.git`. `None` for a run id that is not a plain id, so a
/// hostile value can never name a path outside the runs directory.
pub fn run_repo_path(workspace_dir: &Path, run_id: &str) -> Option<PathBuf> {
    safe_id(run_id).then(|| runs_root(workspace_dir).join(format!("{run_id}.git")))
}

/// The manifests snapshot that belongs to a run repository (`X.git` ->
/// `X.manifests`).
pub fn manifests_dir_for(repo: &Path) -> PathBuf {
    repo.with_extension("manifests")
}

/// The directory ecosystem probes should read for a repository path: the
/// manifests snapshot when `dir` is a run repository, else `dir` itself (a
/// plain checkout, as tests use).
pub fn probe_dir_for(dir: &Path) -> PathBuf {
    if dir.extension().is_some_and(|e| e == "git") {
        let manifests = manifests_dir_for(dir);
        if manifests.is_dir() {
            return manifests;
        }
    }
    dir.to_path_buf()
}

// --- What a repo_key names ---------------------------------------------------

/// Where a run's code comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoSource {
    GitHub {
        owner: String,
        repo: String,
    },
    /// Outside production only.
    Local(PathBuf),
}

impl RepoSource {
    /// `owner/repo`, or `local:<absolute path>` when not `production`.
    pub fn parse(repo_key: &str, production: bool) -> Result<Self, PrepareError> {
        let key = repo_key.trim();
        if let Some(path) = key.strip_prefix("local:") {
            if production {
                return Err(PrepareError::BadRepo(
                    "this run needs a GitHub repository (owner/repo); local repositories are \
                     not available in production"
                        .into(),
                ));
            }
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err(PrepareError::BadRepo(
                    "a local: repo_key needs an absolute path".into(),
                ));
            }
            return Ok(Self::Local(path));
        }
        if let Some((owner, repo)) = key
            .split_once('/')
            .filter(|_| crate::github::is_valid_repo_full_name(key))
        {
            return Ok(Self::GitHub {
                owner: owner.to_string(),
                repo: repo.to_string(),
            });
        }
        Err(PrepareError::BadRepo(
            "repo_key must be a GitHub repository written owner/repo".into(),
        ))
    }

    /// The HTTPS remote of a GitHub source.
    pub fn github_url(owner: &str, repo: &str) -> String {
        format!("https://github.com/{owner}/{repo}.git")
    }
}

/// `owner/repo` out of a run's `repo_key`; `None` for anything else.
pub fn github_owner_repo(repo_key: &str) -> Option<(String, String)> {
    match RepoSource::parse(repo_key, true) {
        Ok(RepoSource::GitHub { owner, repo }) => Some((owner, repo)),
        _ => None,
    }
}

// --- Failure to prepare a run's repository --------------------------------

/// Why a run's repository could not be made. None of these is ever charged:
/// the run has not started, so there is nothing to charge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareError {
    /// The repo_key cannot be used at all (malformed, `local:` in
    /// production, an empty or missing repository).
    BadRepo(String),
    /// No GitHub token, or GitHub refused it: the owner must reconnect.
    Reconnect(String),
    /// GitHub (or Clerk, which brokers the token) is unreachable or timed out.
    TryLater(String),
    /// Cortex's own failure: git would not run, or our disk failed.
    Ours(String),
}

impl PrepareError {
    pub fn message(&self) -> &str {
        match self {
            Self::BadRepo(m) | Self::Reconnect(m) | Self::TryLater(m) | Self::Ours(m) => m,
        }
    }

    fn reconnect(what: &str) -> Self {
        Self::Reconnect(format!(
            "GitHub did not accept Cortex's access to {what}. Reconnect GitHub in your \
             Cortex settings (the connection may have expired or may not include private \
             repositories) and start the run again. You have not been charged."
        ))
    }

    fn try_later() -> Self {
        Self::TryLater(
            "GitHub could not be reached just now. Try again later. You have not been charged."
                .into(),
        )
    }
}

/// Whether git's stderr says GitHub refused our credentials or the
/// repository is not visible to them (GitHub answers 404 for a private
/// repository the token cannot see).
fn stderr_means_reconnect(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    [
        "authentication failed",
        "could not read username",
        "could not read password",
        "invalid username or password",
        "terminal prompts disabled",
        "returned error: 401",
        "returned error: 403",
        "returned error: 404",
        "repository not found",
        "permission denied",
    ]
    .iter()
    .any(|needle| s.contains(needle))
}

/// Strip auth material from git output before it reaches a user or a log.
fn scrub(stderr: &str) -> String {
    let cleaned: Vec<&str> = stderr
        .lines()
        .filter(|l| !l.to_ascii_lowercase().contains("authorization"))
        .collect();
    let joined = cleaned.join("\n");
    let trimmed = joined.trim();
    let mut out: String = trimmed.chars().take(400).collect();
    if out.is_empty() {
        out = "git failed".into();
    }
    out
}

// --- Git helpers -----------------------------------------------------------

/// A hardened git command on the bare repository `repo`.
fn git_in(repo: &Path) -> std::process::Command {
    let mut git_dir = std::ffi::OsString::from("--git-dir=");
    git_dir.push(repo);
    let mut command = control_git();
    command.arg(git_dir);
    command
}

enum Ran {
    Ok(Vec<u8>),
    /// Ran, exited non-zero: its scrubbed stderr.
    Failed(String),
    TimedOut,
    /// Could not be spawned.
    Spawn(String),
}

fn run(command: std::process::Command, deadline: Duration) -> Ran {
    match run_git(command, deadline) {
        Ok(GitRun::Finished(out)) if out.status.success() => Ran::Ok(out.stdout),
        Ok(GitRun::Finished(out)) => Ran::Failed(scrub(&String::from_utf8_lossy(&out.stderr))),
        Ok(GitRun::TimedOut) => Ran::TimedOut,
        Err(e) => Ran::Spawn(e.to_string()),
    }
}

pub(crate) fn is_hex_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.chars().all(|c| c.is_ascii_hexdigit())
}

// --- Staging: fetch the repository at run start ------------------------------

/// A run repository fetched but not yet named for its run. `create_run` makes
/// the run id inside the database transaction, so the fetch (which must
/// finish, or refuse the run, before a run exists) lands in a `pending-*`
/// directory and is renamed to `{run_id}.git` once the run exists. Dropped
/// unpromoted, it deletes itself.
#[derive(Debug)]
pub struct Staged {
    root: PathBuf,
    id: String,
    base: String,
    promoted: bool,
}

impl Staged {
    fn repo(&self) -> PathBuf {
        self.root.join(format!("pending-{}.git", self.id))
    }

    /// The commit the run starts from: the remote's default branch head.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Give the repository its run's name. Both the repository and its
    /// manifests snapshot move; on error nothing is left under the run's name.
    pub fn promote(mut self, run_id: &str) -> Result<(), String> {
        if !safe_id(run_id) {
            return Err("run id is not a plain id".into());
        }
        let final_repo = self.root.join(format!("{run_id}.git"));
        let pending = self.repo();
        std::fs::rename(&pending, &final_repo)
            .map_err(|e| format!("could not place the run repository: {e}"))?;
        if let Err(e) = std::fs::rename(manifests_dir_for(&pending), manifests_dir_for(&final_repo))
        {
            let _ = std::fs::remove_dir_all(&final_repo);
            return Err(format!("could not place the run manifests: {e}"));
        }
        self.promoted = true;
        Ok(())
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.promoted {
            let repo = self.repo();
            let _ = std::fs::remove_dir_all(manifests_dir_for(&repo));
            let _ = std::fs::remove_dir_all(&repo);
        }
    }
}

/// Fetch `repo_key` for `user_id` into a staging repository under the runs
/// directory. `production` decides whether `local:` keys are allowed.
pub async fn stage(
    workspace_dir: &Path,
    clerk_secret_key: Option<&str>,
    user_id: &str,
    repo_key: &str,
    production: bool,
) -> Result<Staged, PrepareError> {
    let source = RepoSource::parse(repo_key, production)?;
    let token = match &source {
        RepoSource::GitHub { owner, repo } => {
            match crate::github::github_oauth_token(clerk_secret_key, user_id).await {
                Ok(Some(token)) => Some((token, format!("{owner}/{repo}"))),
                Ok(None) => return Err(PrepareError::reconnect(&format!("{owner}/{repo}"))),
                Err(e) => {
                    tracing::warn!(user_id, "run start: could not get the GitHub token: {e}");
                    return Err(PrepareError::try_later());
                }
            }
        }
        RepoSource::Local(_) => None,
    };
    let root = runs_root(workspace_dir);
    tokio::task::spawn_blocking(move || stage_blocking(&root, &source, token))
        .await
        .map_err(|e| PrepareError::Ours(format!("run repository task failed: {e}")))?
}

fn stage_blocking(
    root: &Path,
    source: &RepoSource,
    token: Option<(String, String)>,
) -> Result<Staged, PrepareError> {
    std::fs::create_dir_all(root)
        .map_err(|e| PrepareError::Ours(format!("could not create the runs directory: {e}")))?;
    let mut staged = Staged {
        root: root.to_path_buf(),
        id: uuid::Uuid::new_v4().to_string(),
        base: String::new(),
        promoted: false,
    };
    let repo = staged.repo();

    let mut init = control_git();
    init.args(["init", "--quiet", "--bare", "--template="])
        .arg(&repo);
    match run(init, QUICK_DEADLINE) {
        Ran::Ok(_) => {}
        Ran::Spawn(e) => return Err(PrepareError::Ours(format!("could not run git: {e}"))),
        Ran::Failed(e) => return Err(PrepareError::Ours(format!("git init failed: {e}"))),
        Ran::TimedOut => return Err(PrepareError::Ours("git init did not finish".into())),
    }

    let mut fetch = git_in(&repo);
    fetch.args(["-c", "protocol.allow=never"]);
    let refspec = format!("+HEAD:{BASE_REF}");
    let what;
    match (source, token) {
        (RepoSource::GitHub { owner, repo: name }, Some((token, full_name))) => {
            what = full_name;
            fetch
                .args([
                    "-c",
                    "protocol.https.allow=always",
                    "-c",
                    "http.followRedirects=false",
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    "--no-write-fetch-head",
                ])
                .arg(RepoSource::github_url(owner, name))
                .arg(&refspec);
            // The token goes in the environment, never in argv (which any
            // local user can read) and never to disk.
            fetch
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraheader")
                .env(
                    "GIT_CONFIG_VALUE_0",
                    crate::github_repos::extraheader_value(&token),
                );
        }
        (RepoSource::Local(path), _) => {
            what = path.display().to_string();
            fetch
                .args([
                    "-c",
                    "protocol.file.allow=always",
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    "--no-write-fetch-head",
                ])
                .arg(path)
                .arg(&refspec);
        }
        (RepoSource::GitHub { .. }, None) => {
            return Err(PrepareError::reconnect("this repository"));
        }
    }

    match run(fetch, FETCH_DEADLINE) {
        Ran::Ok(_) => {}
        Ran::Spawn(e) => return Err(PrepareError::Ours(format!("could not run git: {e}"))),
        Ran::TimedOut => return Err(PrepareError::try_later()),
        Ran::Failed(stderr) => {
            tracing::warn!("run start: fetch of {what} failed: {stderr}");
            let lower = stderr.to_ascii_lowercase();
            return Err(match source {
                RepoSource::Local(_) => {
                    PrepareError::BadRepo(format!("could not read the local repository: {stderr}"))
                }
                _ if lower.contains("couldn't find remote ref") => {
                    PrepareError::BadRepo(format!("{what} has no commits to start from"))
                }
                _ if stderr_means_reconnect(&stderr) => PrepareError::reconnect(&what),
                _ => PrepareError::try_later(),
            });
        }
    }

    let mut rev = git_in(&repo);
    rev.args(["rev-parse", "--verify", &format!("{BASE_REF}^{{commit}}")]);
    let base = match run(rev, QUICK_DEADLINE) {
        Ran::Ok(out) => String::from_utf8_lossy(&out).trim().to_string(),
        Ran::Spawn(e) => return Err(PrepareError::Ours(format!("could not run git: {e}"))),
        Ran::Failed(e) => return Err(PrepareError::BadRepo(format!("no base commit: {e}"))),
        Ran::TimedOut => return Err(PrepareError::Ours("git rev-parse did not finish".into())),
    };
    if !is_hex_sha(&base) {
        return Err(PrepareError::BadRepo(
            "the base commit is not a commit id".into(),
        ));
    }
    std::fs::write(repo.join(BASE_FILE), &base)
        .map_err(|e| PrepareError::Ours(format!("could not record the base commit: {e}")))?;
    snapshot_manifests(&repo, &manifests_dir_for(&repo))?;
    staged.base = base;
    Ok(staged)
}

/// Copy the manifest files present at the base commit out of the repository.
fn snapshot_manifests(repo: &Path, dest: &Path) -> Result<(), PrepareError> {
    std::fs::create_dir_all(dest)
        .map_err(|e| PrepareError::Ours(format!("could not create the manifests dir: {e}")))?;
    for name in MANIFEST_FILES {
        // Ask for the size first: a blob is read only once it is known to be
        // small, so a huge file at a manifest's path is never pulled into memory.
        let spec = format!("{BASE_REF}:{name}");
        let mut size = git_in(repo);
        size.args(["cat-file", "-s", &spec]);
        match run(size, QUICK_DEADLINE) {
            Ran::Ok(bytes) => {
                let len = String::from_utf8_lossy(&bytes).trim().parse::<usize>();
                if !len.is_ok_and(|len| len <= MANIFEST_MAX_BYTES) {
                    continue;
                }
            }
            // Missing at the base: the ecosystem simply is not declared.
            Ran::Failed(_) => continue,
            Ran::Spawn(e) => return Err(PrepareError::Ours(format!("could not run git: {e}"))),
            Ran::TimedOut => {
                return Err(PrepareError::Ours("git cat-file did not finish".into()));
            }
        }
        let mut show = git_in(repo);
        show.args(["cat-file", "blob", &spec]);
        match run(show, QUICK_DEADLINE) {
            Ran::Ok(bytes) if bytes.len() <= MANIFEST_MAX_BYTES => {
                std::fs::write(dest.join(name), bytes).map_err(|e| {
                    PrepareError::Ours(format!("could not write the manifests snapshot: {e}"))
                })?;
            }
            Ran::Ok(_) | Ran::Failed(_) => {}
            Ran::Spawn(e) => return Err(PrepareError::Ours(format!("could not run git: {e}"))),
            Ran::TimedOut => {
                return Err(PrepareError::Ours("git cat-file did not finish".into()));
            }
        }
    }
    Ok(())
}

/// The directory a repo map may read for a run: only a `local:` source (or the
/// keyless development fallback to the server workspace) has one; a GitHub
/// source has no checkout on the server.
pub fn local_source_dir(
    repo_key: Option<&str>,
    workspace_dir: &Path,
    production: bool,
) -> Option<PathBuf> {
    let key = repo_key
        .map(str::trim)
        .filter(|k| !k.is_empty() && *k != "default");
    match key {
        Some(key) => match RepoSource::parse(key, production) {
            Ok(RepoSource::Local(path)) => Some(path),
            _ => None,
        },
        None if !production => Some(workspace_dir.to_path_buf()),
        None => None,
    }
}

/// The commit a run repository started from, or `None` if it has none.
pub fn read_base(repo: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(repo.join(BASE_FILE)).ok()?;
    let base = raw.trim();
    is_hex_sha(base).then(|| base.to_string())
}

/// The repository a run works in, making it now if the run has none yet:
/// runs created before this existed, runs created by tests straight in the
/// database, or a repository that was swept. `repo_key` is the run's own key;
/// without one, outside production, the server's workspace stands in.
pub async fn ensure_run_repo(
    workspace_dir: &Path,
    clerk_secret_key: Option<&str>,
    user_id: &str,
    run_id: &str,
    repo_key: Option<&str>,
    production: bool,
) -> Result<PathBuf, PrepareError> {
    let path = run_repo_path(workspace_dir, run_id)
        .ok_or_else(|| PrepareError::BadRepo("invalid run id".into()))?;
    if read_base(&path).is_some() {
        return Ok(path);
    }
    let key = match repo_key
        .map(str::trim)
        .filter(|k| !k.is_empty() && *k != "default")
    {
        Some(key) => key.to_string(),
        None if !production => format!("local:{}", workspace_dir.display()),
        None => {
            return Err(PrepareError::BadRepo(
                "this run has no GitHub repository (repo_key)".into(),
            ))
        }
    };
    let staged = stage(workspace_dir, clerk_secret_key, user_id, &key, production).await?;
    // Another dispatch may have won the race; either repository is equivalent.
    if read_base(&path).is_none() {
        let _ = std::fs::remove_dir_all(&path);
        staged.promote(run_id).map_err(PrepareError::Ours)?;
    }
    Ok(path)
}

// --- Worker transport: base bundle out, head bundle in ---------------------

/// Why a bundle operation failed, in the words of who is to blame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    /// The request named something that is not there (unknown base).
    NotFound(String),
    /// What the worker sent is unusable: charged like any failed attempt.
    Worker(String),
    /// Cortex's own failure (git will not run, our disk failed): absorbed.
    Ours(String),
}

/// `git bundle create` of `base`'s history into `out`, for a worker to fetch.
/// A raw commit id cannot be bundled, so it is put under a temporary ref.
pub fn create_base_bundle(repo: &Path, base: &str, out: &Path) -> Result<(), BundleError> {
    if !is_hex_sha(base) {
        return Err(BundleError::NotFound("base is not a commit id".into()));
    }
    let mut exists = git_in(repo);
    exists.args(["cat-file", "-e", &format!("{base}^{{commit}}")]);
    match run(exists, QUICK_DEADLINE) {
        Ran::Ok(_) => {}
        Ran::Failed(_) => return Err(BundleError::NotFound("unknown base commit".into())),
        Ran::Spawn(e) => return Err(BundleError::Ours(format!("could not run git: {e}"))),
        Ran::TimedOut => return Err(BundleError::Ours("git cat-file did not finish".into())),
    }
    let temp_ref = format!("refs/cortex/dispatch/{}", uuid::Uuid::new_v4());
    let mut set = git_in(repo);
    set.args(["update-ref", &temp_ref, base]);
    match run(set, QUICK_DEADLINE) {
        Ran::Ok(_) => {}
        Ran::Failed(e) => return Err(BundleError::Ours(format!("update-ref failed: {e}"))),
        Ran::Spawn(e) => return Err(BundleError::Ours(format!("could not run git: {e}"))),
        Ran::TimedOut => return Err(BundleError::Ours("git update-ref did not finish".into())),
    }
    let mut bundle = git_in(repo);
    bundle.args(["bundle", "create"]).arg(out).arg(&temp_ref);
    let result = run(bundle, BUNDLE_DEADLINE);
    let mut del = git_in(repo);
    del.args(["update-ref", "-d", &temp_ref]);
    let _ = run(del, QUICK_DEADLINE);
    match result {
        Ran::Ok(_) => Ok(()),
        Ran::Failed(e) => Err(BundleError::Ours(format!("git bundle create failed: {e}"))),
        Ran::Spawn(e) => Err(BundleError::Ours(format!("could not run git: {e}"))),
        Ran::TimedOut => Err(BundleError::Ours("git bundle create did not finish".into())),
    }
}

/// The ref a worker's head bundle must carry for `step_id`.
pub fn upload_ref(step_id: &str) -> String {
    format!("refs/cortex/upload/{step_id}")
}

/// Where a step's uploaded head lands in the run repository.
pub fn step_ref(step_id: &str) -> String {
    format!("refs/cortex/step/{step_id}")
}

/// Verify a worker's head bundle and fetch it (with fsck) into the run
/// repository as `refs/cortex/step/{step_id}`; the head commit id.
///
/// Classification follows verification_driver: git that cannot be spawned, or
/// a disk that refuses the write, is Cortex's; a bundle that does not verify,
/// carries the wrong refs, fails fsck, or stalls git is the worker's.
pub fn ingest_head_bundle(
    repo: &Path,
    step_id: &str,
    bundle: &Path,
) -> Result<String, BundleError> {
    let wanted = upload_ref(step_id);

    let mut verify = git_in(repo);
    verify.args(["bundle", "verify"]).arg(bundle);
    match run(verify, BUNDLE_DEADLINE) {
        Ran::Ok(_) => {}
        Ran::Failed(e) => return Err(BundleError::Worker(format!("bundle rejected: {e}"))),
        Ran::TimedOut => return Err(BundleError::Worker("bundle verify stalled".into())),
        Ran::Spawn(e) => return Err(BundleError::Ours(format!("could not run git: {e}"))),
    }

    let mut heads = git_in(repo);
    heads.args(["bundle", "list-heads"]).arg(bundle);
    let listing = match run(heads, QUICK_DEADLINE) {
        Ran::Ok(out) => String::from_utf8_lossy(&out).to_string(),
        Ran::Failed(e) => return Err(BundleError::Worker(format!("bundle unreadable: {e}"))),
        Ran::TimedOut => return Err(BundleError::Worker("bundle list-heads stalled".into())),
        Ran::Spawn(e) => return Err(BundleError::Ours(format!("could not run git: {e}"))),
    };
    let refs: Vec<&str> = listing
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .collect();
    if refs != [wanted.as_str()] {
        return Err(BundleError::Worker(format!(
            "the bundle must carry exactly the ref {wanted}"
        )));
    }

    let mut fetch = git_in(repo);
    fetch
        .args([
            "-c",
            "transfer.fsckObjects=true",
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
        ])
        .arg(bundle)
        .arg(format!("+{wanted}:{}", step_ref(step_id)));
    match run(fetch, BUNDLE_DEADLINE) {
        Ran::Ok(_) => {}
        Ran::Failed(e) => {
            let lower = e.to_ascii_lowercase();
            let ours = [
                "no space left",
                "unable to create",
                "unable to write",
                "read-only",
            ]
            .iter()
            .any(|n| lower.contains(n));
            return Err(if ours {
                BundleError::Ours(format!("could not store the bundle: {e}"))
            } else {
                BundleError::Worker(format!("bundle failed the object check: {e}"))
            });
        }
        Ran::TimedOut => return Err(BundleError::Worker("bundle fetch stalled".into())),
        Ran::Spawn(e) => return Err(BundleError::Ours(format!("could not run git: {e}"))),
    }

    let mut rev = git_in(repo);
    rev.args([
        "rev-parse",
        "--verify",
        &format!("{}^{{commit}}", step_ref(step_id)),
    ]);
    match run(rev, QUICK_DEADLINE) {
        Ran::Ok(out) => {
            let head = String::from_utf8_lossy(&out).trim().to_string();
            if is_hex_sha(&head) {
                Ok(head)
            } else {
                Err(BundleError::Worker(
                    "the bundle's head is not a commit".into(),
                ))
            }
        }
        Ran::Failed(e) => Err(BundleError::Worker(format!(
            "the bundle's head is missing: {e}"
        ))),
        Ran::TimedOut => Err(BundleError::Ours("git rev-parse did not finish".into())),
        Ran::Spawn(e) => Err(BundleError::Ours(format!("could not run git: {e}"))),
    }
}

// --- Push -------------------------------------------------------------------

/// Why a push to GitHub failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushError {
    Reconnect(String),
    Failed(String),
}

/// Push `commit` from the run repository to `branch` of `owner/repo` on
/// GitHub with the owner's OAuth token.
pub fn push_commit(
    repo: &Path,
    owner: &str,
    name: &str,
    token: &str,
    commit: &str,
    branch: &str,
) -> Result<(), PushError> {
    if !is_hex_sha(commit) {
        return Err(PushError::Failed("head is not a commit id".into()));
    }
    let mut push = git_in(repo);
    push.args([
        "-c",
        "protocol.allow=never",
        "-c",
        "protocol.https.allow=always",
        "-c",
        "http.followRedirects=false",
        "push",
        "--quiet",
    ])
    .arg(RepoSource::github_url(owner, name))
    .arg(format!("{commit}:refs/heads/{branch}"))
    .env("GIT_TERMINAL_PROMPT", "0")
    .env("GIT_CONFIG_COUNT", "1")
    .env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraheader")
    .env(
        "GIT_CONFIG_VALUE_0",
        crate::github_repos::extraheader_value(token),
    );
    match run(push, FETCH_DEADLINE) {
        Ran::Ok(_) => Ok(()),
        Ran::Failed(e) if stderr_means_reconnect(&e) => Err(PushError::Reconnect(e)),
        Ran::Failed(e) => Err(PushError::Failed(e)),
        Ran::TimedOut => Err(PushError::Failed("git push did not finish".into())),
        Ran::Spawn(e) => Err(PushError::Failed(format!("could not run git: {e}"))),
    }
}

// --- Sweep -------------------------------------------------------------------

/// What the sweep needs to know about a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunLife {
    /// Not in the database.
    Unknown,
    Active,
    /// Terminal since this time (unix milliseconds).
    FinishedAt(i64),
}

/// Best-effort sweep of the runs directory: repositories (and manifests
/// snapshots) of runs terminal for at least `retention`, repositories of runs
/// the database does not know that are that old, and stale `pending-*`
/// leftovers. Symlinks are never followed. Returns how many entries were
/// removed. Never fails: what it cannot remove it leaves.
pub fn sweep_run_repos<F>(root: &Path, retention: Duration, now_ms: i64, life: F) -> usize
where
    F: Fn(&str) -> RunLife,
{
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let retention_ms = i64::try_from(retention.as_millis()).unwrap_or(i64::MAX);
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(stem) = name
            .strip_suffix(".git")
            .or_else(|| name.strip_suffix(".manifests"))
        else {
            continue;
        };
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let aged = |age: Duration| {
            meta.modified()
                .ok()
                .and_then(|m| m.elapsed().ok())
                .is_some_and(|e| e >= age)
        };
        let expired = if stem.starts_with("pending-") {
            aged(STALE_PENDING_AGE)
        } else {
            match life(stem) {
                RunLife::Active => false,
                RunLife::Unknown => aged(retention),
                RunLife::FinishedAt(at) => now_ms.saturating_sub(at) >= retention_ms,
            }
        };
        if expired && std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Sweep once now, then hourly, for the life of the process.
pub fn spawn_sweeper(state: std::sync::Arc<crate::state::AppState>) {
    tokio::spawn(async move {
        loop {
            let s = state.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let Some(db) = s.db.as_ref() else {
                    return;
                };
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
                let removed = sweep_run_repos(
                    &runs_root(&s.workspace_dir),
                    RUN_REPO_RETENTION,
                    now_ms,
                    |id| db.run_life(id),
                );
                if removed > 0 {
                    tracing::info!("run repositories: removed {removed} expired");
                }
            })
            .await;
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A source repository with one commit holding a Cargo.toml.
    fn source_repo() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "--quiet", "-b", "main"]);
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "--quiet", "-m", "one"]);
        let head = git(dir.path(), &["rev-parse", "HEAD"]);
        (dir, head)
    }

    #[test]
    fn repo_keys_parse() {
        assert_eq!(
            RepoSource::parse("acme/widgets", true),
            Ok(RepoSource::GitHub {
                owner: "acme".into(),
                repo: "widgets".into()
            })
        );
        assert_eq!(
            RepoSource::parse("local:/tmp/x", false),
            Ok(RepoSource::Local(PathBuf::from("/tmp/x")))
        );
        assert!(matches!(
            RepoSource::parse("local:/tmp/x", true),
            Err(PrepareError::BadRepo(_))
        ));
        assert!(matches!(
            RepoSource::parse("local:relative", false),
            Err(PrepareError::BadRepo(_))
        ));
        assert!(matches!(
            RepoSource::parse("default", false),
            Err(PrepareError::BadRepo(_))
        ));
        assert_eq!(
            github_owner_repo("acme/widgets"),
            Some(("acme".into(), "widgets".into()))
        );
        assert_eq!(github_owner_repo("local:/tmp/x"), None);
    }

    #[test]
    fn run_ids_cannot_escape_the_runs_directory() {
        let ws = Path::new("/nonexistent-cortex-ws");
        assert!(run_repo_path(ws, "../evil").is_none());
        assert!(run_repo_path(ws, "a/b").is_none());
        assert!(run_repo_path(ws, "").is_none());
    }

    #[test]
    fn auth_failures_ask_for_a_reconnect_and_outages_do_not() {
        assert!(stderr_means_reconnect(
            "fatal: Authentication failed for 'https://github.com/a/b.git/'"
        ));
        assert!(stderr_means_reconnect(
            "fatal: repository 'x' not found\nRepository not found."
        ));
        assert!(!stderr_means_reconnect(
            "fatal: unable to access 'https://github.com/a/b.git/': Could not resolve host"
        ));
        assert!(!stderr_means_reconnect(
            "error: RPC failed; HTTP 502 curl 22"
        ));
    }

    #[tokio::test]
    async fn a_local_repository_is_fetched_and_promoted() {
        let (src, head) = source_repo();
        let ws = tempfile::tempdir().unwrap();
        let key = format!("local:{}", src.path().display());
        let staged = stage(ws.path(), None, "u", &key, false)
            .await
            .expect("stage");
        assert_eq!(staged.base(), head);
        staged.promote("run-1").expect("promote");
        let repo = run_repo_path(ws.path(), "run-1").unwrap();
        assert_eq!(read_base(&repo).as_deref(), Some(head.as_str()));
        assert!(manifests_dir_for(&repo).join("Cargo.toml").is_file());
        assert!(!manifests_dir_for(&repo).join("package.json").exists());
        assert!(probe_dir_for(&repo).join("Cargo.toml").is_file());
    }

    #[tokio::test]
    async fn an_unpromoted_stage_cleans_up_after_itself() {
        let (src, _) = source_repo();
        let ws = tempfile::tempdir().unwrap();
        let key = format!("local:{}", src.path().display());
        drop(
            stage(ws.path(), None, "u", &key, false)
                .await
                .expect("stage"),
        );
        let left: Vec<_> = std::fs::read_dir(runs_root(ws.path()))
            .unwrap()
            .flatten()
            .collect();
        assert!(left.is_empty(), "nothing may remain: {left:?}");
    }

    #[tokio::test]
    async fn production_refuses_local_and_missing_repositories() {
        let ws = tempfile::tempdir().unwrap();
        let refused = stage(ws.path(), None, "u", "local:/tmp/x", true).await;
        assert!(matches!(refused, Err(PrepareError::BadRepo(_))));
        let none = ensure_run_repo(ws.path(), None, "u", "run-2", None, true).await;
        assert!(matches!(none, Err(PrepareError::BadRepo(_))));
    }

    #[tokio::test]
    async fn a_github_repository_without_a_token_asks_to_reconnect() {
        let ws = tempfile::tempdir().unwrap();
        let refused = stage(ws.path(), None, "u", "acme/widgets", true).await;
        match refused {
            Err(PrepareError::Reconnect(m)) => assert!(m.contains("Reconnect GitHub")),
            other => panic!("expected Reconnect, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn bundles_round_trip_between_run_repo_and_a_worker() {
        let (src, base) = source_repo();
        let ws = tempfile::tempdir().unwrap();
        let key = format!("local:{}", src.path().display());
        stage(ws.path(), None, "u", &key, false)
            .await
            .unwrap()
            .promote("run-3")
            .unwrap();
        let repo = run_repo_path(ws.path(), "run-3").unwrap();
        let tmp = tempfile::tempdir().unwrap();

        // Out: the base bundle contains the base commit.
        let out = tmp.path().join("base.bundle");
        create_base_bundle(&repo, &base, &out).expect("base bundle");
        let worker = tmp.path().join("worker.git");
        git(tmp.path(), &["init", "--quiet", "--bare", "worker.git"]);
        git(
            &worker,
            &[
                "fetch",
                "--quiet",
                out.to_str().unwrap(),
                "refs/cortex/dispatch/*:refs/cortex/dispatch/*",
            ],
        );
        git(&worker, &["cat-file", "-e", &format!("{base}^{{commit}}")]);
        assert_eq!(
            create_base_bundle(&repo, &"0".repeat(40), &tmp.path().join("x")),
            Err(BundleError::NotFound("unknown base commit".into()))
        );

        // Back: a worker commit on top of base, bundled under the upload ref.
        let tree = git(&worker, &["rev-parse", &format!("{base}^{{tree}}")]);
        let head = git(&worker, &["commit-tree", &tree, "-p", &base, "-m", "work"]);
        git(&worker, &["update-ref", &upload_ref("s1"), &head]);
        let up = tmp.path().join("head.bundle");
        git(
            &worker,
            &[
                "bundle",
                "create",
                up.to_str().unwrap(),
                &upload_ref("s1"),
                &format!("^{base}"),
            ],
        );
        assert_eq!(ingest_head_bundle(&repo, "s1", &up), Ok(head.clone()));
        git(&repo, &["cat-file", "-e", &format!("{head}^{{commit}}")]);

        // Wrong step, and garbage, are the worker's fault.
        assert!(matches!(
            ingest_head_bundle(&repo, "s2", &up),
            Err(BundleError::Worker(_))
        ));
        let junk = tmp.path().join("junk.bundle");
        std::fs::write(&junk, b"not a bundle").unwrap();
        assert!(matches!(
            ingest_head_bundle(&repo, "s1", &junk),
            Err(BundleError::Worker(_))
        ));
    }

    #[test]
    fn the_sweep_removes_old_terminal_runs_only() {
        let root = tempfile::tempdir().unwrap();
        for name in [
            "done.git",
            "done.manifests",
            "live.git",
            "new.git",
            "pending-x.git",
            "other",
        ] {
            std::fs::create_dir_all(root.path().join(name)).unwrap();
        }
        let day = 24 * 60 * 60 * 1000;
        let now = 100 * day;
        let removed = sweep_run_repos(root.path(), RUN_REPO_RETENTION, now, |id| match id {
            "done" => RunLife::FinishedAt(now - 8 * day),
            "new" => RunLife::FinishedAt(now - day),
            _ => RunLife::Active,
        });
        assert_eq!(removed, 2);
        assert!(!root.path().join("done.git").exists());
        assert!(!root.path().join("done.manifests").exists());
        assert!(root.path().join("live.git").exists());
        assert!(root.path().join("new.git").exists());
        assert!(
            root.path().join("pending-x.git").exists(),
            "fresh staging is live"
        );
        assert!(root.path().join("other").exists());
    }

    #[test]
    fn a_pr_targets_the_repo_key_and_nothing_else() {
        // `create_pr_core` derives the owner/repo and the push URL from the
        // run's `repo_key` alone.
        let (owner, repo) = github_owner_repo("acme/widgets").expect("valid key");
        assert_eq!((owner.as_str(), repo.as_str()), ("acme", "widgets"));
        assert_eq!(
            RepoSource::github_url(&owner, &repo),
            "https://github.com/acme/widgets.git"
        );
        // Surrounding whitespace is tolerated, as `parse` trims.
        assert_eq!(
            github_owner_repo("  acme/widgets "),
            Some(("acme".to_string(), "widgets".to_string()))
        );
        // Nothing that is not a plain GitHub owner/repo yields a target.
        for key in [
            "",
            "widgets",
            "local:/srv/repo",
            "acme/widgets/extra",
            "acme/wid gets",
            "../etc/passwd",
            "https://evil.example/acme/widgets",
        ] {
            assert_eq!(github_owner_repo(key), None, "{key:?}");
        }
    }
}
