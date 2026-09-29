//! Run the frozen checks against a delivered tree and bind the verdict to the
//! ledger.
//!
//! This is the money path. Everything here exists to make two guarantees hold
//! at once: a verdict is produced by *us* executing checks rather than by a
//! worker reporting on itself, and the settlement that follows — a charge or
//! an absorption of the attempt's own observed cost — fires exactly once no
//! matter how many times this runs.
//!
//! **No verdict here ever produces a refund.** The owner's settled billing
//! rule (`cortex_core::billing_binding`) is pass-through: a customer pays
//! exactly what the model calls an attempt made, nothing more, and a failed
//! attempt is still charged for calls it made. The only question this module
//! answers per attempt-end is *charge the customer, or have Cortex absorb it*
//! — see [`billing_binding::AttemptEndCause`] and [`billing_binding::settle_attempt`].
//!
//! The sequence, and why it is in this order:
//!
//! 1. Load the specs frozen at dispatch. Deriving them now would let the task
//!    influence its own exam.
//! 2. Claim the attempt. `UNIQUE(run_id, step_id, attempt)` is a CAS — losing
//!    it means another process owns this verdict, and the correct response is
//!    to stop, not to retry.
//! 3. Snapshot the delivered commit into a fresh checkout. Never the worker's
//!    working directory: the moment a worker can influence its own verdict the
//!    product claim is void.
//! 4. Execute, record, compute the verdict, seal it.
//! 5. Record the attempt's end durably (`attempt_endings`, the single source
//!    of truth that it ended and why) and settle it: sum its settled observed
//!    provider cost and either charge it (via `ChargeKey::for_attempt`, so a
//!    replay of the same attempt writes no second row) or absorb it against
//!    Cortex. The record and the settlement are two separate, idempotent
//!    steps — see [`crate::db::Database::settle_pending_attempts`] — so a
//!    crash between them still leaves a trail the scheduler finishes later.
//!
//! See `cortex/plan/VERIFIER.md` and `cortex/plan/V3-LAUNCH-SPEC.md`.

use std::path::{Path, PathBuf};

use cortex_core::billing_binding::{classify_exam_integrity_failure, AttemptEndCause};
use cortex_core::check_derivation::EcosystemFacts;
use cortex_core::diff_surface::{self, ClassOutcome, VerdictClass};
use cortex_core::verification::{
    CheckExecution, CheckOutcome, CheckRunner, CheckSpec, TreeSnapshot, Verdict,
};

use crate::db::Database;

/// How many extra passes a check gets when the *runner* fails, before the
/// check is recorded as `NotExecuted`. Infra problems cost us time, not the
/// customer money — but they cannot retry forever either.
const RUNNER_RETRIES: usize = 2;

/// What the completion handler knows once a worker has delivered.
#[derive(Debug, Clone)]
pub struct DeliveryFacts {
    pub run_id: String,
    pub step_id: String,
    /// The attempt this delivery belongs to. Carried so the verdict can be
    /// written against the same lifecycle row the delivery opened.
    pub attempt_id: String,
    pub attempt: i64,
    /// The repository the worker delivered into. Used only as the source for
    /// a detached checkout — never mounted directly.
    pub workspace_dir: PathBuf,
    /// The commit the worker delivered. This is what gets graded.
    pub head_commit: String,
    /// Credits quoted for this step at dispatch time, kept for display only.
    /// **Not what the customer is charged** — `finish_and_bill` records the
    /// attempt's end and `settle_pending_attempts` settles it from the
    /// attempt's actual settled observed provider cost, never this quote.
    /// `None` means no quote was reachable; that has no effect on billing
    /// either way.
    pub quoted_credits: Option<i64>,
}

/// The image checks execute in, pinned by digest in production.
///
/// "It passed" is only reproducible if *where* it passed is pinned, so this is
/// recorded on every execution. Overridable per deployment because Phase A
/// (containers) and Phase B (microVMs) use different images.
/// Which set of runner rules was in force when a job was enqueued.
///
/// Recorded on the job so a receipt can say what the rules *were* rather than
/// what they are now. Bumped whenever the runner's behaviour changes in a way
/// that would make two verdicts incomparable.
pub const RUNNER_POLICY_VERSION: &str = "runner-policy-1";

pub fn runner_image() -> String {
    std::env::var("CORTEX_RUNNER_IMAGE").unwrap_or_else(|_| "cortex/runner:phase-a".to_string())
}

/// Whether a commit the worker reported resolves in the server's workspace
/// repository.
///
/// Three outcomes, not two, because the difference is who pays. `Missing` is
/// the worker's delivery being broken (charged). `CheckFailed` means Cortex
/// could not find out -- git would not start, or the workspace has no `.git`
/// at all -- which is Cortex's own machinery failing and is absorbed, never
/// charged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommitCheck {
    /// The commit exists as a commit object in the workspace repository.
    Resolves,
    /// The commit is not there as far as git can tell: the workspace's
    /// `.git` exists and the commit is not in it, git refuses that `.git`
    /// (an unsupported `repositoryformatversion`, a corrupt or unreadable
    /// store), or the reported string is not even a well-formed object id.
    /// The workspace repository is worker-writable, so all of these are the
    /// worker's doing and all are charged.
    Missing,
    /// Cortex could not determine either way: git would not spawn, or the
    /// workspace has no `.git`.
    CheckFailed(String),
}

/// Is `commit` shaped like a full object id (SHA-1 or SHA-256 hex)?
///
/// The delivered commit is worker-controlled text that ends up in git's argv.
/// Anything else -- a NUL byte, a string too long to spawn with -- would make
/// the *spawn* fail, and a failed spawn looks exactly like Cortex's own fault.
fn is_full_object_id(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64) && commit.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The directory git must not search upward into when it looks for a
/// repository from `workspace_dir`: the workspace's parent. With it set, a
/// workspace that is not itself a repository cannot be answered by some
/// enclosing repository.
fn discovery_ceiling(workspace_dir: &Path) -> Option<PathBuf> {
    let resolved = std::fs::canonicalize(workspace_dir)
        .ok()
        .or_else(|| std::path::absolute(workspace_dir).ok())?;
    resolved.parent().map(Path::to_path_buf)
}

/// A `git -C {workspace_dir}` command with a stable locale (so logged stderr
/// is stable; nothing classifies on it) and repository discovery pinned to the
/// workspace itself.
///
/// The workspace repository (its `.git`, hooks, config, attribute files and
/// object store) is writable by the worker, so nothing git would run *because
/// of that repository* is allowed to run: hooks are pointed at nowhere, the
/// fsmonitor is off, the attribute file the repository's config names is
/// replaced by an empty one, and neither the system nor the user's global git
/// configuration is read. This does not make the repository trustworthy -- see
/// [`control_checkout`], which is why no verdict about who is at fault is ever
/// taken from it.
fn workspace_git(workspace_dir: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.attributesFile=/dev/null",
        ])
        .arg("-C")
        .arg(workspace_dir)
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    if let Some(ceiling) = discovery_ceiling(workspace_dir) {
        command.env("GIT_CEILING_DIRECTORIES", ceiling);
    }
    command
}

/// Blocking: shells out to git. Async callers must run it through
/// `tokio::task::spawn_blocking`.
///
/// A commit that is not 40 or 64 hex digits is `Missing` without spawning
/// anything (see [`is_full_object_id`]).
///
/// `CheckFailed` is returned ONLY when git cannot be started, or when
/// `{workspace_dir}/.git` does not exist. Everything else is `Missing`, which
/// is charged: `git cat-file -e {commit}^{commit}` exiting non-zero for *any*
/// reason -- the commit being absent, git refusing a `.git` that does exist
/// (an unsupported `repositoryformatversion`, say), an unreadable or corrupt
/// object store, a lock or permission error -- because the exit status does not
/// tell them apart, and because the workspace repository is writable by the
/// worker: any state of it that makes git refuse it is something the worker
/// could have put there, so it must never buy an absorbed attempt.
pub(crate) fn check_commit(workspace_dir: &Path, commit: &str) -> CommitCheck {
    if !is_full_object_id(commit) {
        return CommitCheck::Missing;
    }
    if !workspace_dir.join(".git").exists() {
        return CommitCheck::CheckFailed(format!(
            "workspace {} has no .git",
            workspace_dir.display()
        ));
    }
    match workspace_git(workspace_dir)
        .arg("cat-file")
        .arg("-e")
        .arg(format!("{commit}^{{commit}}"))
        .output()
    {
        Err(e) => CommitCheck::CheckFailed(format!("could not run git cat-file: {e}")),
        Ok(out) if out.status.success() => CommitCheck::Resolves,
        Ok(_) => CommitCheck::Missing,
    }
}

/// Why a delivered tree could not be checked out. The split decides who pays.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TreeCheckoutError {
    /// git ran, refused the delivered commit, and a control checkout through
    /// the very same git in a repository Cortex made itself succeeded -- so
    /// the refusal is about the delivered tree or the workspace repository
    /// (an entry named `.git`, `..`, a name too long or unsafe for the
    /// checkout filesystem, an unreadable object, a broken store). A worker
    /// can build every one of those, so it is charged.
    DeliveredTree(String),
    /// Anything else: git would not spawn, the workspace has no `.git`, the
    /// scratch disk is nearly full, or the control checkout failed too.
    /// Cortex's own machinery, absorbed.
    Cortex(String),
}

impl TreeCheckoutError {
    fn message(&self) -> &str {
        match self {
            Self::DeliveredTree(m) | Self::Cortex(m) => m,
        }
    }
}

/// Free space on the scratch filesystem below which a refused checkout is
/// blamed on Cortex: git may well have been refused by a full disk, and a
/// control checkout on a full disk would only repeat the failure.
const CONTROL_MIN_FREE_BYTES: u64 = 64 * 1024 * 1024;

/// Bytes available to an unprivileged writer on the filesystem holding `dir`,
/// or `None` when that cannot be found out.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;

    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is a valid NUL-terminated string that outlives the call,
    // and `stats` is a valid out-pointer that `statvfs` fully initialises when
    // it returns 0, which is the only case in which it is read.
    let ok = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } == 0;
    if !ok {
        return None;
    }
    // SAFETY: initialised by the successful `statvfs` call above.
    let stats = unsafe { stats.assume_init() };
    Some((stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64))
}

/// Without `statvfs` there is no check; the control checkout still runs.
#[cfg(not(unix))]
fn free_bytes(_dir: &Path) -> Option<u64> {
    None
}

/// `Err` when `dir`'s filesystem is known to have less than
/// [`CONTROL_MIN_FREE_BYTES`] free. An unknown amount is not an error.
fn has_room(dir: &Path) -> Result<(), String> {
    match free_bytes(dir) {
        Some(free) if free < CONTROL_MIN_FREE_BYTES => Err(format!(
            "only {free} bytes are free under {}",
            dir.display()
        )),
        _ => Ok(()),
    }
}

/// A `git` command for the control: no `-C` into the workspace, no
/// repository, hooks, attribute file or fsmonitor, no system or global
/// configuration, and a fixed author and committer, so nothing about it
/// depends on the machine's git setup or on anything the worker can write.
fn control_git() -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "init.defaultBranch=main",
        ])
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "cortex")
        .env("GIT_AUTHOR_EMAIL", "cortex@localhost")
        .env("GIT_COMMITTER_NAME", "cortex")
        .env("GIT_COMMITTER_EMAIL", "cortex@localhost")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(std::process::Stdio::null());
    command
}

/// Run one control git command; its trimmed stdout, or why it failed.
fn run_control(mut command: std::process::Command, what: &str) -> Result<String, String> {
    let out = command
        .output()
        .map_err(|e| format!("could not run {what}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{what} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The control: a checkout that involves nothing the worker can touch. A
/// fresh repository Cortex makes itself under `parent` (never the workspace,
/// whose config, hooks, attributes and object store are all worker-writable)
/// gets a parentless empty-tree commit, which is checked out with
/// `git worktree add --detach` into another fresh directory under `parent` --
/// the same operation, through the same git and onto the same filesystem, as
/// the delivered checkout. Both directories are removed afterwards.
///
/// `Ok` means git and the checkout filesystem work, so a refusal of the
/// delivered commit was about the delivered commit (or the repository it
/// lives in). `Err` means they do not, which is Cortex's own fault.
fn control_checkout(parent: &Path) -> Result<(), String> {
    let scratch = parent.join(format!(
        "cortex-verify-control-repo-{}",
        uuid::Uuid::new_v4()
    ));
    let checkout = parent.join(format!("cortex-verify-control-{}", uuid::Uuid::new_v4()));
    let result = control_checkout_in(&scratch, &checkout);
    // Whatever happened, leave nothing behind.
    let _ = std::fs::remove_dir_all(&checkout);
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

fn control_checkout_in(scratch: &Path, checkout: &Path) -> Result<(), String> {
    let in_scratch = || {
        let mut command = control_git();
        command.arg("-C").arg(scratch);
        command
    };

    let mut init = control_git();
    init.args(["init", "--quiet", "--template="]).arg(scratch);
    run_control(init, "git init of the control repository")?;

    let mut hash = in_scratch();
    hash.args(["hash-object", "-t", "tree", "-w", "--stdin"]);
    let tree = run_control(hash, "git hash-object")?;

    let mut commit = in_scratch();
    commit.args(["commit-tree", &tree, "-m", "cortex control checkout"]);
    let commit = run_control(commit, "git commit-tree")?;

    // Give the scratch repository a real `HEAD`, so `worktree add` has an
    // ordinary repository to work from.
    let mut head = in_scratch();
    head.args(["update-ref", "HEAD", &commit]);
    run_control(head, "git update-ref")?;

    let mut add = in_scratch();
    add.args(["worktree", "add", "--detach"])
        .arg(checkout)
        .arg(&commit);
    run_control(add, "the control git worktree add")?;
    Ok(())
}

/// A detached checkout of the delivered commit, removed on drop.
struct TreeCheckout {
    workspace_dir: PathBuf,
    path: PathBuf,
}

impl TreeCheckout {
    /// `git worktree add --detach` at the delivered commit. Cheap (it shares
    /// the object store) and, unlike a copy, guaranteed to be exactly the
    /// delivered tree with nothing the worker left lying around.
    ///
    /// When git spawns but refuses, that alone does not say whose fault it is:
    /// git's stderr echoes worker-chosen file names and depends on the locale,
    /// so it is never classified on. And nothing in the workspace repository
    /// can be trusted to say, because the worker can write all of it. So the
    /// decision is made from state Cortex owns: once the failed checkout is
    /// cleaned up and the scratch filesystem is known to have room, a control
    /// checkout is run in a repository Cortex creates itself
    /// ([`control_checkout`]). If the control works, what git refused is the
    /// delivered tree ([`TreeCheckoutError::DeliveredTree`], charged); if the
    /// control fails too, Cortex's machinery is what failed
    /// ([`TreeCheckoutError::Cortex`], absorbed). A git that will not spawn,
    /// a workspace with no `.git`, and a nearly full scratch disk are `Cortex`
    /// outright.
    fn create(workspace_dir: &Path, commit: &str) -> Result<Self, TreeCheckoutError> {
        Self::create_in(&std::env::temp_dir(), workspace_dir, commit)
    }

    /// [`Self::create`] with the scratch directory chosen by the caller.
    fn create_in(
        parent: &Path,
        workspace_dir: &Path,
        commit: &str,
    ) -> Result<Self, TreeCheckoutError> {
        if !workspace_dir.join(".git").exists() {
            return Err(TreeCheckoutError::Cortex(format!(
                "workspace {} has no .git",
                workspace_dir.display()
            )));
        }
        let path = parent.join(format!("cortex-verify-{}", uuid::Uuid::new_v4()));
        let out = workspace_git(workspace_dir)
            .arg("worktree")
            .arg("add")
            .arg("--detach")
            .arg(&path)
            .arg(commit)
            .output()
            .map_err(|e| {
                TreeCheckoutError::Cortex(format!("could not run git worktree add: {e}"))
            })?;
        if !out.status.success() {
            // Best effort: a refused checkout can leave a half-made directory
            // and a registration of it in the workspace repository.
            let _ = std::fs::remove_dir_all(&path);
            let _ = workspace_git(workspace_dir)
                .arg("worktree")
                .arg("prune")
                .output();
            // For the log message only; never classified on.
            let message = format!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            if let Err(room) = has_room(parent) {
                return Err(TreeCheckoutError::Cortex(format!(
                    "{message}; not enough scratch space for a control checkout: {room}"
                )));
            }
            return Err(match control_checkout(parent) {
                Ok(()) => TreeCheckoutError::DeliveredTree(message),
                Err(control) => TreeCheckoutError::Cortex(format!(
                    "{message}; the control checkout failed too: {control}"
                )),
            });
        }
        Ok(Self {
            workspace_dir: workspace_dir.to_path_buf(),
            path,
        })
    }
}

impl Drop for TreeCheckout {
    fn drop(&mut self) {
        let _ = workspace_git(&self.workspace_dir)
            .arg("worktree")
            .arg("remove")
            .arg("--force")
            .arg(&self.path)
            .output();
    }
}

/// Verify one delivery. Returns the verdict, or `None` if there was nothing to
/// do — the attempt was already claimed, or the tree could not be snapshotted.
///
/// Generic over the runner rather than taking `dyn CheckRunner`: the trait uses
/// RPITIT and is not object-safe.
pub async fn verify_delivery<R: CheckRunner>(
    db: &Database,
    runner: &R,
    facts: &DeliveryFacts,
) -> Option<Verdict> {
    let specs = db.load_check_specs(&facts.run_id, &facts.step_id);

    // Nothing was frozen for this step, so it is not a step this machinery
    // attaches to — read-only work (Search/Think/Review/Gate) never has specs
    // frozen for it. Claiming a verification here would mint a verdict of
    // `Unverified`, and `Unverified` *is* billable, so a Think step would
    // charge. Verification attaches to steps that change trees and to nothing
    // else (VERIFIER.md, "what not to do").
    if specs.is_empty() {
        tracing::debug!(
            run_id = %facts.run_id,
            step_id = %facts.step_id,
            "no frozen checks for this step; nothing to verify"
        );
        return None;
    }

    let verification_id = db.claim_verification(
        &facts.run_id,
        &facts.step_id,
        facts.attempt,
        &facts.head_commit,
        runner.runner_image(),
    )?;

    // Phase 27.2. Freezing the specs stopped a task rewriting its own *argv*.
    // It never stopped it rewriting what that argv reads, and the checkout
    // below is a clean checkout of the delivered commit -- the very commit
    // whose test files the agent controls.
    //
    // So ask what the delivery touched before grading it. A plan that declared
    // `strong` and then edited the exam has contradicted its own contract.
    // That is not a verdict, and the run does not get to choose the weaker
    // class after the fact.
    match exam_integrity(db, facts).await {
        ExamIntegrity::Intact | ExamIntegrity::PermittedAuthoredWork => {}
        ExamIntegrity::ModifiedExam { paths } => {
            let detail = format!(
                "exam integrity: modified protected exam surface: {}",
                paths.join(", ")
            );
            tracing::error!(
                run_id = %facts.run_id,
                step_id = %facts.step_id,
                exam_paths = ?paths,
                "declared verdict_class=strong and then edited the exam; inconclusive"
            );
            finish_inconclusive_without_grading(
                db,
                &verification_id,
                facts,
                &detail,
                AttemptEndCause::ExamTampered,
            )
            .await;
            return Some(Verdict::Inconclusive);
        }
        ExamIntegrity::Unknown {
            reason,
            delivered_tree_caused,
        } => {
            let detail = format!("exam integrity unknown: {reason}");
            tracing::error!(
                run_id = %facts.run_id,
                step_id = %facts.step_id,
                reason = %reason,
                delivered_tree_caused,
                "could not establish frozen exam integrity; grading refused"
            );
            finish_inconclusive_without_grading(
                db,
                &verification_id,
                facts,
                &detail,
                classify_exam_integrity_failure(delivered_tree_caused),
            )
            .await;
            return Some(Verdict::Inconclusive);
        }
    }

    // Blocking git and filesystem work, so off the async thread.
    let checkout = {
        let (workspace, head) = (facts.workspace_dir.clone(), facts.head_commit.clone());
        tokio::task::spawn_blocking(move || TreeCheckout::create(&workspace, &head))
            .await
            .unwrap_or_else(|e| {
                Err(TreeCheckoutError::Cortex(format!(
                    "checkout task did not complete: {e}"
                )))
            })
    };
    let checkout = match checkout {
        Ok(c) => c,
        Err(e) => {
            // We could not produce a tree to grade, so it is Inconclusive and
            // an operator still hears about it. Leaving the row 'pending'
            // would strand the attempt. Who pays depends on why:
            //
            // - git ran and refused the delivered tree while a control checkout
            //   in a repository Cortex made itself worked (an entry named
            //   `.git`, `..`, a name too long or unsafe -- a worker can build
            //   such a commit with `git mktree`): that is what the worker
            //   delivered, so it is charged, not free work.
            // - anything else (git would not spawn, IO, low scratch space, or
            //   the control failed too: disk, lock) is Cortex's own machinery:
            //   absorbed.
            let delivered_tree_caused = matches!(e, TreeCheckoutError::DeliveredTree(_));
            tracing::error!(
                run_id = %facts.run_id,
                step_id = %facts.step_id,
                error = %e.message(),
                delivered_tree_caused,
                "could not snapshot the delivered tree; verification is inconclusive"
            );
            finish_inconclusive_without_grading(
                db,
                &verification_id,
                facts,
                e.message(),
                classify_exam_integrity_failure(delivered_tree_caused),
            )
            .await;
            return Some(Verdict::Inconclusive);
        }
    };

    let tree = TreeSnapshot {
        tree_hash: facts.head_commit.clone(),
        path: checkout.path.to_string_lossy().to_string(),
    };

    let mut executions: Vec<CheckExecution> = Vec::with_capacity(specs.len());
    for spec in &specs {
        let execution = run_with_retries(runner, &tree, spec).await;
        if let Err(e) = db.record_check_execution(&verification_id, spec, &execution) {
            tracing::error!(
                verification_id = %verification_id,
                spec_id = %spec.id,
                error = %e,
                "failed to record a check execution"
            );
        }
        executions.push(execution);
    }

    let report = cortex_core::verification::compute_verdict(&specs, &executions);
    tracing::info!(
        run_id = %facts.run_id,
        step_id = %facts.step_id,
        verdict = ?report.verdict,
        required_passed = report.required_passed,
        required_total = report.required_total,
        "verification complete"
    );

    // A real verdict maps straight to its matching end cause. `Inconclusive`
    // reaching here can only mean `run_with_retries` exhausted its retries --
    // our runner, not the customer's work, failed to produce a result -- so
    // it absorbs rather than charges.
    let end_cause = match report.verdict {
        Verdict::Verified => AttemptEndCause::Verified,
        Verdict::Unverified => AttemptEndCause::Unverified,
        Verdict::Failed => AttemptEndCause::Failed,
        Verdict::Inconclusive => AttemptEndCause::RunnerDown,
    };
    finish_and_bill(db, &verification_id, report.verdict, end_cause, facts).await;
    project_verdict(db, facts, report.verdict, None);
    Some(report.verdict)
}

/// Seal an integrity refusal without reaching the runner, then settle the
/// attempt for the given cause (see
/// [`cortex_core::billing_binding::settle_attempt`]).
async fn finish_inconclusive_without_grading(
    db: &Database,
    verification_id: &str,
    facts: &DeliveryFacts,
    detail: &str,
    cause: AttemptEndCause,
) {
    finish_and_bill(db, verification_id, Verdict::Inconclusive, cause, facts).await;
    project_verdict(db, facts, Verdict::Inconclusive, Some(detail));
}

/// Move the step itself to the state the verdict implies.
///
/// This is the line that makes verification mean something. Before it existed
/// the verdict was recorded beside the step and the step had already been
/// marked succeeded by the worker's own report; the grade was written on a
/// paper nobody read.
///
/// A verdict for a superseded attempt is a no-op — the transition carries the
/// `lease_gen` CAS, so a verifier that finishes after its step was re-leased
/// cannot move the live attempt.
fn project_verdict(db: &Database, facts: &DeliveryFacts, verdict: Verdict, detail: Option<&str>) {
    let state = match verdict {
        Verdict::Verified => "verified",
        Verdict::Failed => "failed",
        // Unverified means no executable ground truth existed, so nothing was
        // proven. It is not a pass. It stays visible as an unanswered question
        // rather than becoming a badge.
        Verdict::Unverified | Verdict::Inconclusive => "inconclusive",
    };
    let applied = db.record_verification_outcome(
        &facts.step_id,
        &facts.attempt_id,
        facts.attempt,
        state,
        detail,
    );
    if !applied {
        tracing::warn!(
            run_id = %facts.run_id,
            step_id = %facts.step_id,
            attempt = facts.attempt,
            state,
            "verdict did not move the step — the attempt was superseded or was not verifying"
        );
    }
}

/// Run one check, retrying only when the *runner* failed.
///
/// A check that runs and fails is a verdict about the work and is returned
/// immediately. Only `Err` — our infrastructure failing — is retried, and only
/// a bounded number of times, after which the check is `NotExecuted` and the
/// verdict becomes `Inconclusive` rather than a charge.
async fn run_with_retries<R: CheckRunner>(
    runner: &R,
    tree: &TreeSnapshot,
    spec: &CheckSpec,
) -> CheckExecution {
    let mut last_error = String::new();
    for attempt in 0..=RUNNER_RETRIES {
        match runner.run(tree, spec).await {
            Ok(execution) => return execution,
            Err(e) => {
                last_error = e.to_string();
                tracing::warn!(
                    spec_id = %spec.id,
                    attempt,
                    error = %last_error,
                    "check runner failed; retrying"
                );
            }
        }
    }

    tracing::error!(
        spec_id = %spec.id,
        error = %last_error,
        "check could not be executed after retries; verdict will be inconclusive"
    );
    CheckExecution {
        spec_id: spec.id.clone(),
        exit_code: None,
        outcome: CheckOutcome::NotExecuted,
        duration_ms: 0,
        output_digest: String::new(),
        output_tail: format!("runner error: {last_error}"),
        runner_image: runner.runner_image().to_string(),
    }
}

/// Seal the verdict, then record the attempt's end durably and settle it:
/// charge its settled observed cost to the customer, or have Cortex absorb
/// it. Never a refund — see the module doc comment and
/// [`cortex_core::billing_binding::settle_attempt`].
///
/// This no longer computes the settled cost or writes the ledger row itself.
/// It writes one `attempt_endings` row (the durable, single source of truth
/// that this attempt ended and why) and then runs the shared settler,
/// [`Database::settle_pending_attempts`], immediately — so the ledger still
/// reflects the outcome by the time this returns, matching every existing
/// caller's and test's expectations. Splitting the record from the charge
/// this way means a crash between the two still leaves a durable trail: the
/// scheduler's tick and startup passes call the same settler and will finish
/// the job even if this in-request call never returns.
async fn finish_and_bill(
    db: &Database,
    verification_id: &str,
    verdict: Verdict,
    end_cause: AttemptEndCause,
    facts: &DeliveryFacts,
) {
    // `worker_owned_by_cortex` only matters to `classify_lease_expiry` and
    // `classify_worker_failure`, neither of which produces the causes this
    // module ever passes here (Verified/Unverified/Failed/ExamTampered/
    // CortexCrash/RunnerDown) -- `settle_attempt` maps those directly with no
    // dependence on worker ownership. `false` is the schema's own default for
    // a record where the field is not meaningful.
    //
    // Sealing the verdict and recording why the attempt ended happen in one
    // transaction (F6 of the money-review fix pass): a crash between them
    // used to be able to leave a sealed verdict with no durable ending for
    // the settler to find.
    if let Err(e) = db.finish_verification_and_end(
        verification_id,
        verdict,
        &facts.run_id,
        &facts.step_id,
        &facts.attempt_id,
        end_cause,
        false,
    ) {
        tracing::error!(
            run_id = %facts.run_id,
            step_id = %facts.step_id,
            attempt_id = %facts.attempt_id,
            verification_id,
            error = %e,
            "failed to seal verdict and record attempt ending"
        );
        return;
    }

    if let Err(e) = db.settle_pending_attempts() {
        tracing::error!(
            run_id = %facts.run_id,
            verification_id,
            attempt_id = %facts.attempt_id,
            error = %e,
            "settle_pending_attempts failed after recording attempt ending; the scheduler's next tick will retry"
        );
    }
}

/// What is known about the frozen exam before any grading process starts.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ExamIntegrity {
    Intact,
    PermittedAuthoredWork,
    ModifiedExam {
        paths: Vec<String>,
    },
    /// `delivered_tree_caused` distinguishes a failure caused by what the
    /// worker actually delivered (its head commit is missing from a healthy
    /// workspace repository) from a failure in Cortex's own machinery (a
    /// missing or unreadable work contract, a base commit Cortex itself
    /// should have recorded at dispatch, git or the disk failing). Fed to
    /// `classify_exam_integrity_failure` (F2 of the money-review fix pass)
    /// so an unknown result is charged when it stems from delivered content
    /// and absorbed only when it is genuinely ours.
    Unknown {
        reason: String,
        delivered_tree_caused: bool,
    },
}

async fn exam_integrity(db: &Database, facts: &DeliveryFacts) -> ExamIntegrity {
    let contract = match db.read_step_work_contract(&facts.step_id, facts.attempt) {
        Ok(Some(contract)) => contract,
        Ok(None) => {
            return ExamIntegrity::Unknown {
                reason: "missing work contract".to_string(),
                delivered_tree_caused: false,
            };
        }
        Err(err) => {
            return ExamIntegrity::Unknown {
                reason: format!("unreadable work contract: {err}"),
                delivered_tree_caused: false,
            };
        }
    };
    // `None` (undeclared -- either a pre-PR contract or one written by a path
    // that never set the field) is graded exactly like `Authored`: today's
    // behaviour on `main`, before this field existed at all.
    if contract.verdict_class != Some(VerdictClass::Strong) {
        return ExamIntegrity::PermittedAuthoredWork;
    }

    let Some(base) = contract.expected_base_commit.as_deref() else {
        return ExamIntegrity::Unknown {
            reason: "strong contract is missing its required base commit".to_string(),
            delivered_tree_caused: false,
        };
    };
    // Blocking git, so off the async thread.
    let changed = {
        let (workspace, base, head) = (
            facts.workspace_dir.clone(),
            base.to_string(),
            facts.head_commit.clone(),
        );
        tokio::task::spawn_blocking(move || changed_paths(&workspace, &base, &head))
            .await
            .unwrap_or_else(|e| Err(format!("diff inspection task did not complete: {e}")))
    };
    let changed = match changed {
        Ok(changed) => changed,
        Err(err) => {
            let (workspace, head) = (facts.workspace_dir.clone(), facts.head_commit.clone());
            // `StepCompleted` already rejects a head that does not resolve
            // in the workspace (charged there), so a head that reaches this
            // point resolved when it was delivered. See
            // `diff_failure_is_delivered_tree` for who pays for a failure here.
            let delivered_tree_caused = tokio::task::spawn_blocking(move || {
                diff_failure_is_delivered_tree(&workspace, &head)
            })
            .await
            .unwrap_or(false);
            return ExamIntegrity::Unknown {
                reason: format!("diff inspection failed: {err}"),
                delivered_tree_caused,
            };
        }
    };
    let partition = diff_surface::partition(&changed, &ecosystem_facts_for(&facts.workspace_dir));

    match diff_surface::resolve_class(VerdictClass::Strong, &partition) {
        ClassOutcome::StrongContractBroken { exam_paths } => {
            ExamIntegrity::ModifiedExam { paths: exam_paths }
        }
        ClassOutcome::Honoured(_) => ExamIntegrity::Intact,
    }
}

/// `git diff --name-only base..head`, against the workspace.
///
/// Failure is an explicit unknown-integrity result. It is neither evidence of
/// tampering nor permission to grade.
fn changed_paths(workspace_dir: &Path, base: &str, head: &str) -> Result<Vec<String>, String> {
    let out = workspace_git(workspace_dir)
        .arg("diff")
        .arg("--name-only")
        .arg(format!("{base}..{head}"))
        .output()
        .map_err(|err| format!("could not run git diff: {err}"))?;
    if !out.status.success() {
        return Err(format!(
            "git diff {base}..{head} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Is a failed `base..head` diff the delivered tree's doing? True whenever
/// [`check_commit`] does not say `CheckFailed` for the head -- that is, unless
/// git would not spawn or the workspace has no `.git`, in which case Cortex's
/// own machinery failed and the attempt is absorbed. Nothing here looks at
/// git's stderr. Blocking.
///
/// Tradeoff, on purpose: the workspace repository, including its object store,
/// is writable by the worker, so nothing read from it can prove the fault is
/// Cortex's -- a worker can delete or corrupt any object to make a diff fail,
/// and an answer that turned on "can the base still be read" could be steered
/// by exactly that. The cost is that a genuine disk I/O error in the
/// worker-writable store is charged to the customer. The only answer that
/// cannot be steered is a Cortex-owned copy of the objects, which the
/// delivery-transport PR provides.
fn diff_failure_is_delivered_tree(workspace_dir: &Path, head: &str) -> bool {
    !matches!(
        check_commit(workspace_dir, head),
        CommitCheck::CheckFailed(_)
    )
}

/// What ecosystems the workspace root declares, for path classification.
fn ecosystem_facts_for(workspace_dir: &Path) -> EcosystemFacts {
    EcosystemFacts {
        has_cargo_manifest: workspace_dir.join("Cargo.toml").exists(),
        has_package_json: workspace_dir.join("package.json").exists(),
        npm_scripts: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::verification::{CheckSource, RunnerError};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// A runner we control, so the driver's sequencing can be tested without
    /// Docker. Generic bound is `R: CheckRunner`, so this substitutes freely.
    struct ScriptedRunner {
        /// Outcome per call, popped in order. `Err` simulates infra failure.
        script: Mutex<Vec<Result<CheckOutcome, RunnerError>>>,
        calls: AtomicUsize,
        image: String,
    }

    impl ScriptedRunner {
        fn new(script: Vec<Result<CheckOutcome, RunnerError>>) -> Self {
            let mut reversed = script;
            reversed.reverse();
            Self {
                script: Mutex::new(reversed),
                calls: AtomicUsize::new(0),
                image: "test/runner@sha256:0".to_string(),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl CheckRunner for ScriptedRunner {
        async fn run(
            &self,
            _tree: &TreeSnapshot,
            check: &CheckSpec,
        ) -> Result<CheckExecution, RunnerError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let next = self.script.lock().unwrap().pop();
            match next {
                Some(Ok(outcome)) => Ok(CheckExecution {
                    spec_id: check.id.clone(),
                    exit_code: Some(if outcome == CheckOutcome::Passed {
                        0
                    } else {
                        1
                    }),
                    outcome,
                    duration_ms: 5,
                    output_digest: "sha256:test".to_string(),
                    output_tail: String::new(),
                    runner_image: self.image.clone(),
                }),
                Some(Err(e)) => Err(e),
                None => Ok(CheckExecution {
                    spec_id: check.id.clone(),
                    exit_code: Some(0),
                    outcome: CheckOutcome::Passed,
                    duration_ms: 5,
                    output_digest: "sha256:test".to_string(),
                    output_tail: String::new(),
                    runner_image: self.image.clone(),
                }),
            }
        }

        fn runner_image(&self) -> &str {
            &self.image
        }
    }

    fn spec(id: &str) -> CheckSpec {
        CheckSpec {
            id: id.to_string(),
            source: CheckSource::Contract,
            command: vec!["true".to_string()],
            timeout_secs: 5,
            required: true,
        }
    }

    #[tokio::test]
    async fn runner_failure_is_retried_then_recorded_as_not_executed() {
        let runner = ScriptedRunner::new(vec![
            Err(RunnerError::ExecutionFailed("boom".into())),
            Err(RunnerError::ExecutionFailed("boom".into())),
            Err(RunnerError::ExecutionFailed("boom".into())),
        ]);
        let tree = TreeSnapshot {
            tree_hash: "abc".to_string(),
            path: ".".to_string(),
        };

        let execution = run_with_retries(&runner, &tree, &spec("c1")).await;
        assert_eq!(
            execution.outcome,
            CheckOutcome::NotExecuted,
            "exhausted retries must not become a pass or a fail"
        );
        assert!(execution.exit_code.is_none());
    }

    #[tokio::test]
    async fn a_check_that_runs_and_fails_is_not_retried() {
        // One Failed, then entries that would pass. If the driver retried a
        // real failure it would come back Passed, which would be a charge for
        // work that did not meet its contract.
        let runner = ScriptedRunner::new(vec![
            Ok(CheckOutcome::Failed),
            Ok(CheckOutcome::Passed),
            Ok(CheckOutcome::Passed),
        ]);
        let tree = TreeSnapshot {
            tree_hash: "abc".to_string(),
            path: ".".to_string(),
        };

        let execution = run_with_retries(&runner, &tree, &spec("c1")).await;
        assert_eq!(execution.outcome, CheckOutcome::Failed);
    }

    // --- End-to-end: the whole sequence against a real git repository ---

    fn test_db() -> crate::db::Database {
        let dir = tempfile::tempdir().unwrap().keep();
        crate::db::Database::open(&dir.join("cortex.sqlite"))
    }

    /// A real repository with one commit, so `git worktree add --detach` has
    /// something to check out. The tree snapshot is deliberately not mockable —
    /// shelling out to git is what guarantees the checks run against the
    /// delivered commit rather than against a worker's directory.
    fn repo_with_one_commit() -> (std::path::PathBuf, String) {
        let dir = tempfile::tempdir().unwrap().keep();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("git runs");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(&["init", "--initial-branch=main"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("file.txt"), "delivered\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "delivered work"]);
        let head = git(&["rev-parse", "HEAD"]);
        (dir, head)
    }

    fn facts(dir: &std::path::Path, head: &str) -> DeliveryFacts {
        DeliveryFacts {
            run_id: "run-1".to_string(),
            step_id: "step-1".to_string(),
            attempt_id: "attempt-1".to_string(),
            attempt: 1,
            workspace_dir: dir.to_path_buf(),
            head_commit: head.to_string(),
            // No quote is reachable yet, so the ledger is deliberately untouched.
            quoted_credits: None,
        }
    }

    #[tokio::test]
    async fn a_step_with_no_frozen_checks_is_not_verified_at_all() {
        let db = test_db();
        let (dir, head) = repo_with_one_commit();

        let runner = ScriptedRunner::new(vec![]);
        let verdict = verify_delivery(&db, &runner, &facts(&dir, &head)).await;

        assert!(
            verdict.is_none(),
            "read-only work has no frozen specs and must not mint a verdict — \
             Unverified is billable, so a Think step would otherwise charge"
        );
        assert!(db.get_receipt("run-1", "step-1").is_none());
    }

    #[tokio::test]
    async fn passing_checks_produce_a_verified_receipt() {
        let db = test_db();
        let (dir, head) = repo_with_one_commit();
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Strong, &head),
        ));
        let specs = vec![spec("c1"), spec("c2")];
        db.save_check_specs(&run_id, "step-1", &specs).unwrap();

        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed), Ok(CheckOutcome::Passed)]);
        let verdict =
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1")).await;
        assert_eq!(verdict, Some(Verdict::Verified));

        let receipt = db.get_receipt(&run_id, "step-1").expect("receipt exists");
        assert_eq!(
            receipt.tree_hash, head,
            "the receipt pins the graded commit"
        );
        assert_eq!(receipt.executions.len(), 2);
        assert_eq!(receipt.gate.required_total, 2);
        assert_eq!(receipt.gate.required_passed, 2);
        // This test is what guards `get_receipt` against the deadlock its
        // trailing self.read_step_work_contract / self.get_step_quote /
        // self.ledger_net_charge_for_verification calls could reintroduce: a
        // regression there hangs this test rather than failing it, so the
        // fields those calls actually populate need to be asserted here.
        assert_eq!(
            receipt.verdict_class,
            Some(VerdictClass::Strong),
            "read back from the frozen work contract declared above"
        );
        assert_eq!(
            receipt.charged_credits, None,
            "this attempt made no settled provider calls, so finish_and_bill's \
             attempt_settled_cost_micro_usd is 0 and no ledger row exists to read back"
        );
    }

    #[tokio::test]
    async fn one_failing_check_fails_the_verdict() {
        let db = test_db();
        let (dir, head) = repo_with_one_commit();
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Strong, &head),
        ));
        let specs = vec![spec("c1"), spec("c2")];
        db.save_check_specs(&run_id, "step-1", &specs).unwrap();

        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed), Ok(CheckOutcome::Failed)]);
        let verdict =
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1")).await;
        assert_eq!(verdict, Some(Verdict::Failed));

        let receipt = db.get_receipt(&run_id, "step-1").expect("receipt exists");
        assert!(receipt.gate.failed.contains(&"c2".to_string()));
    }

    #[tokio::test]
    async fn a_runner_that_cannot_execute_is_inconclusive_and_never_bills() {
        let db = test_db();
        let (dir, head) = repo_with_one_commit();
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Strong, &head),
        ));
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();

        // Every attempt fails as infrastructure, exhausting the retries.
        let runner = ScriptedRunner::new(vec![
            Err(RunnerError::ExecutionFailed("no docker".into())),
            Err(RunnerError::ExecutionFailed("no docker".into())),
            Err(RunnerError::ExecutionFailed("no docker".into())),
        ]);
        let verdict =
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1")).await;

        assert_eq!(
            verdict,
            Some(Verdict::Inconclusive),
            "our infrastructure failing must cost us time, not the customer money"
        );
        assert!(!Verdict::Inconclusive.has_billing_effect());
    }

    #[tokio::test]
    async fn the_second_verifier_to_reach_an_attempt_does_nothing() {
        let db = test_db();
        let (dir, head) = repo_with_one_commit();
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Strong, &head),
        ));
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();
        let f = facts_for(&dir, &head, &run_id, "step-1");

        let first = verify_delivery(
            &db,
            &ScriptedRunner::new(vec![Ok(CheckOutcome::Passed)]),
            &f,
        )
        .await;
        assert_eq!(first, Some(Verdict::Verified));

        // A duplicate delivery for the same attempt: the CAS must lose, so the
        // ledger key derived from the verification id is minted exactly once.
        let second = verify_delivery(
            &db,
            &ScriptedRunner::new(vec![Ok(CheckOutcome::Failed)]),
            &f,
        )
        .await;
        assert!(
            second.is_none(),
            "the second claim must not produce a verdict"
        );

        let receipt = db.get_receipt(&run_id, "step-1").expect("receipt");
        assert_eq!(
            receipt.gate.verdict,
            Verdict::Verified,
            "the duplicate must not overwrite the first verdict"
        );
        assert_eq!(receipt.executions.len(), 1, "and must not add executions");
    }

    /// A repo with a base commit and a delivered commit editing `paths`.
    fn repo_with_base_and_delivery(paths: &[&str]) -> (std::path::PathBuf, String, String) {
        let dir = tempfile::tempdir().unwrap().keep();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("git runs");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(&["init", "--initial-branch=main"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]
",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/lib.rs"),
            "pub fn a() {}
",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        std::fs::write(
            dir.join("tests/e2e.rs"),
            "#[test] fn t() {}
",
        )
        .unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "base"]);
        let base = git(&["rev-parse", "HEAD"]);

        for p in paths {
            std::fs::write(
                dir.join(p),
                "// delivered edit
",
            )
            .unwrap();
        }
        git(&["add", "."]);
        git(&["commit", "-m", "delivered"]);
        let head = git(&["rev-parse", "HEAD"]);
        (dir, base, head)
    }

    /// Seed a real run and step, because `step_work_contracts` has foreign
    /// keys to both. Returns the generated run id.
    fn seed_run_and_step(db: &Database, step_id: &str) -> String {
        db.create_run_with_steps(
            "user-1",
            "do the thing",
            "balanced",
            &[],
            None,
            None,
            None,
            &[(
                step_id.to_string(),
                "execute".to_string(),
                "modify".to_string(),
                None,
                "execute".to_string(),
                "low".to_string(),
                "do the thing".to_string(),
                0,
            )],
            &[],
        )
    }

    fn facts_for(dir: &std::path::Path, head: &str, run_id: &str, step_id: &str) -> DeliveryFacts {
        DeliveryFacts {
            run_id: run_id.to_string(),
            step_id: step_id.to_string(),
            attempt_id: "attempt-1".to_string(),
            attempt: 1,
            workspace_dir: dir.to_path_buf(),
            head_commit: head.to_string(),
            quoted_credits: None,
        }
    }

    fn seed_verifying_step(db: &Database, step_id: &str, head: &str) -> (String, i64) {
        let run_id = seed_run_and_step(db, step_id);
        db.update_run_status(&run_id, "running", None);
        db.register_worker("worker-1", "user-1", false);
        let lease_gen = db
            .lease_step(step_id, "worker-1", i64::MAX, "attempt-1")
            .expect("step leases");
        assert!(db.start_step(step_id, lease_gen));
        assert!(db.deliver_step(
            step_id,
            "attempt-1",
            lease_gen,
            None,
            None,
            None,
            Some(head),
        ));
        assert!(db.begin_verifying_step(step_id, "attempt-1", lease_gen, None));
        (run_id, lease_gen)
    }

    fn verification_reason(db: &Database, step_id: &str, lease_gen: i64) -> String {
        db.conn()
            .query_row(
                "SELECT terminal_reason FROM step_verification_state
                 WHERE step_id = ?1 AND attempt_id = 'attempt-1' AND lease_gen = ?2",
                rusqlite::params![step_id, lease_gen],
                |row| row.get::<_, String>(0),
            )
            .expect("terminal diagnostic persisted")
    }

    fn contract_declaring(class: VerdictClass, base: &str) -> cortex_core::task::TaskContract {
        let mut c = cortex_core::task::TaskContract::new(
            "do the thing".to_string(),
            cortex_core::provider::Tier::Execute,
            cortex_core::routing::RiskLevel::Low,
        );
        c.verdict_class = Some(class);
        c.expected_base_commit = Some(base.to_string());
        c
    }

    #[tokio::test]
    async fn a_strong_declaration_that_edits_the_exam_is_a_contract_violation() {
        // The case Phase 27.2 exists for. Dispatch promised the battery would
        // be the customer's own; delivery rewrote it. Downgrading to
        // `authored` here would make `strong` mean "strong unless it was
        // inconvenient", so it is refused instead.
        let db = test_db();
        let (dir, base, head) = repo_with_base_and_delivery(&["src/lib.rs", "tests/e2e.rs"]);
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Strong, &base),
        ));

        assert_eq!(
            exam_integrity(&db, &facts_for(&dir, &head, &run_id, "step-1")).await,
            ExamIntegrity::ModifiedExam {
                paths: vec!["tests/e2e.rs".to_string()]
            },
            "the exam path is named, and only the exam path"
        );
    }

    #[tokio::test]
    async fn a_strong_declaration_that_leaves_the_exam_alone_is_graded() {
        let db = test_db();
        let (dir, base, head) = repo_with_base_and_delivery(&["src/lib.rs"]);
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Strong, &base),
        ));

        assert_eq!(
            exam_integrity(&db, &facts_for(&dir, &head, &run_id, "step-1")).await,
            ExamIntegrity::Intact,
            "subject-only work under a strong declaration is what strong means"
        );
    }

    #[tokio::test]
    async fn authored_work_may_edit_the_exam() {
        // Characterization testing and TDD both write the exam. If this ever
        // fires, Phase 24.2's brownfield on-ramp is broken.
        let db = test_db();
        let (dir, base, head) = repo_with_base_and_delivery(&["src/lib.rs", "tests/e2e.rs"]);
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Authored, &base),
        ));

        assert_eq!(
            exam_integrity(&db, &facts_for(&dir, &head, &run_id, "step-1")).await,
            ExamIntegrity::PermittedAuthoredWork,
            "an authored verdict is allowed to have written the exam"
        );
    }

    #[tokio::test]
    async fn an_undeclared_verdict_class_is_graded_as_authored() {
        // `verdict_class: None` is a pre-PR contract, or one written by a path
        // that never set the field. It must be treated exactly like
        // `Authored` -- today's behaviour on `main` -- rather than tripping
        // the `strong` exam-integrity check it never opted into.
        let db = test_db();
        let (dir, base, head) = repo_with_base_and_delivery(&["src/lib.rs", "tests/e2e.rs"]);
        let run_id = seed_run_and_step(&db, "step-1");
        let mut contract = contract_declaring(VerdictClass::Authored, &base);
        contract.verdict_class = None;
        assert!(db.record_step_work_contract("step-1", &run_id, 1, &contract,));

        assert_eq!(
            exam_integrity(&db, &facts_for(&dir, &head, &run_id, "step-1")).await,
            ExamIntegrity::PermittedAuthoredWork,
            "undeclared reads back as authored, not as a broken strong contract"
        );
    }

    #[tokio::test]
    async fn a_missing_contract_is_explicitly_unknown() {
        let db = test_db();
        let (dir, _base, head) = repo_with_base_and_delivery(&["tests/e2e.rs"]);
        assert_eq!(
            exam_integrity(&db, &facts(&dir, &head)).await,
            ExamIntegrity::Unknown {
                reason: "missing work contract".to_string(),
                delivered_tree_caused: false,
            }
        );
    }

    #[tokio::test]
    async fn a_modified_protected_exam_is_inconclusive_without_running_checks() {
        let db = test_db();
        let (dir, base, head) = repo_with_base_and_delivery(&["src/lib.rs", "tests/e2e.rs"]);
        let (run_id, lease_gen) = seed_verifying_step(&db, "step-1", &head);
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            lease_gen,
            &contract_declaring(VerdictClass::Strong, &base),
        ));
        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed)]);

        let verdict =
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1")).await;

        assert_eq!(verdict, Some(Verdict::Inconclusive));
        assert_eq!(runner.calls(), 0, "a modified exam must not be graded");
        assert_eq!(
            db.get_step_status("step-1").as_deref(),
            Some("inconclusive")
        );
        assert!(verification_reason(&db, "step-1", lease_gen)
            .contains("modified protected exam surface: tests/e2e.rs"));
    }

    #[tokio::test]
    async fn a_missing_contract_is_inconclusive_unbilled_and_idempotent() {
        let db = test_db();
        let (dir, _base, head) = repo_with_base_and_delivery(&["src/lib.rs"]);
        let (run_id, lease_gen) = seed_verifying_step(&db, "step-1", &head);
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();
        db.init_credit_balance("user-1", 50).expect("balance");
        let mut f = facts_for(&dir, &head, &run_id, "step-1");
        f.quoted_credits = Some(10);
        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed)]);

        assert_eq!(
            verify_delivery(&db, &runner, &f).await,
            Some(Verdict::Inconclusive)
        );
        assert_eq!(runner.calls(), 0, "unknown integrity must run no checks");
        assert_eq!(db.get_credit_balance("user-1").subscription_remaining, 50);
        assert_eq!(db.credit_ledger_totals("user-1"), (0, 0));
        assert_eq!(
            db.get_step_status("step-1").as_deref(),
            Some("inconclusive")
        );
        assert!(verification_reason(&db, "step-1", lease_gen)
            .contains("exam integrity unknown: missing work contract"));

        assert_eq!(
            verify_delivery(&db, &runner, &f).await,
            None,
            "replay must lose the verification claim"
        );
        assert_eq!(runner.calls(), 0);
        assert_eq!(db.credit_ledger_totals("user-1"), (0, 0));
    }

    #[tokio::test]
    async fn an_unreadable_contract_is_inconclusive_with_its_own_diagnostic() {
        let db = test_db();
        let (dir, base, head) = repo_with_base_and_delivery(&["src/lib.rs"]);
        let (run_id, lease_gen) = seed_verifying_step(&db, "step-1", &head);
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            lease_gen,
            &contract_declaring(VerdictClass::Strong, &base),
        ));
        db.conn()
            .execute(
                "UPDATE step_work_contracts SET contract_json = '{' \
                 WHERE step_id = 'step-1' AND lease_gen = ?1",
                rusqlite::params![lease_gen],
            )
            .unwrap();
        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed)]);

        assert_eq!(
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1"),).await,
            Some(Verdict::Inconclusive)
        );
        assert_eq!(runner.calls(), 0);
        assert!(verification_reason(&db, "step-1", lease_gen).contains("unreadable work contract"));
    }

    #[tokio::test]
    async fn a_missing_required_base_is_inconclusive_with_its_own_diagnostic() {
        let db = test_db();
        let (dir, _base, head) = repo_with_base_and_delivery(&["src/lib.rs"]);
        let (run_id, lease_gen) = seed_verifying_step(&db, "step-1", &head);
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();
        let mut contract = contract_declaring(VerdictClass::Strong, &head);
        contract.expected_base_commit = None;
        assert!(db.record_step_work_contract("step-1", &run_id, lease_gen, &contract,));
        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed)]);

        assert_eq!(
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1"),).await,
            Some(Verdict::Inconclusive)
        );
        assert_eq!(runner.calls(), 0);
        assert!(verification_reason(&db, "step-1", lease_gen)
            .contains("strong contract is missing its required base commit"));
    }

    const ATTEMPT_NOW: i64 = 1_800_000_000_000;

    /// One genuinely settled provider call for `attempt_id` -- the same
    /// reserve-then-settle path a real gateway `forward()` drives -- so a
    /// test can assert on an exact charge or absorption, not just a label.
    fn settle_call_for(db: &Database, user_id: &str, attempt_id: &str, observed: i64) {
        let price_list_id = db.active_price_list().unwrap().id;
        db.set_supplier_capacity("claude", 10_000_000, ATTEMPT_NOW)
            .unwrap();
        let authorization = crate::db::SpendAuthorization {
            id: format!("auth-{attempt_id}"),
            user_id: user_id.into(),
            run_id: format!("run-for-{attempt_id}"),
            attempt_id: attempt_id.into(),
            provider: "claude".into(),
            model: "claude-sonnet-5".into(),
            price_list_id,
            max_micro_usd: 10_000_000,
            expires_at_ms: ATTEMPT_NOW + 60_000,
        };
        db.create_spend_authorization(&authorization, ATTEMPT_NOW)
            .unwrap();
        let claims = crate::provider_gateway::GatewayCapability::new(
            authorization.id,
            authorization.user_id,
            authorization.run_id,
            authorization.attempt_id,
            authorization.provider,
            authorization.model,
            authorization.expires_at_ms,
        );
        let request_key = format!("call-{attempt_id}");
        db.reserve_provider_request(&claims, &request_key, "digest", observed, ATTEMPT_NOW)
            .expect("reserve");
        let settled = db
            .settle_provider_request(&request_key, observed, Some("upstream-1"), ATTEMPT_NOW)
            .expect("settle");
        assert_eq!(settled.status, "settled");
    }

    fn ending_cause(db: &Database, attempt_id: &str) -> Option<String> {
        db.conn()
            .query_row(
                "SELECT cause FROM attempt_endings WHERE attempt_id = ?1",
                rusqlite::params![attempt_id],
                |row| row.get::<_, String>(0),
            )
            .ok()
    }

    #[tokio::test]
    async fn a_failed_diff_inspection_on_a_resolving_head_is_charged_with_its_own_diagnostic() {
        // The base is not a commit but the head is valid and resolves. The
        // workspace repository is worker-writable, so a diff that fails
        // against a head that resolves cannot be shown to be Cortex's fault:
        // it is charged. The attempt made a real settled call, so an exact
        // balance is meaningful.
        let db = test_db();
        let (dir, _base, head) = repo_with_base_and_delivery(&["src/lib.rs"]);
        let (run_id, lease_gen) = seed_verifying_step(&db, "step-1", &head);
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            lease_gen,
            &contract_declaring(VerdictClass::Strong, "not-a-commit"),
        ));
        db.init_credit_balance("user-1", 1_000).expect("balance");
        settle_call_for(&db, "user-1", "attempt-1", 300_000);
        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed)]);

        assert_eq!(
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1"),).await,
            Some(Verdict::Inconclusive)
        );
        assert_eq!(runner.calls(), 0);
        assert!(verification_reason(&db, "step-1", lease_gen).contains("diff inspection failed"));
        assert_eq!(
            ending_cause(&db, "attempt-1").as_deref(),
            Some("exam_tampered"),
            "a diff that fails on a head that resolves is charged, not absorbed"
        );
        assert_eq!(
            db.get_credit_balance("user-1").subscription_remaining,
            997,
            "300_000 micro-USD is exactly 3 whole credits at the seeded price"
        );
    }

    /// A commit that exists but that git refuses to check out: its tree has a
    /// single entry called `entry_name`. Built with `git mktree` and
    /// `git commit-tree`, which is all a worker needs to do. Returns the
    /// repository, a known-good base commit in it, and the crafted commit.
    fn commit_with_tree_entry(entry_name: &str) -> (std::path::PathBuf, String, String) {
        use std::io::Write as _;
        use std::process::Stdio;

        let (dir, base) = repo_with_one_commit();
        let run = |args: &[&str], stdin: Option<&str>| -> String {
            let mut child = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("git runs");
            if let Some(input) = stdin {
                child
                    .stdin
                    .as_mut()
                    .expect("piped stdin")
                    .write_all(input.as_bytes())
                    .expect("write stdin");
            }
            drop(child.stdin.take());
            let out = child.wait_with_output().expect("git finishes");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let blob = run(&["hash-object", "-w", "file.txt"], None);
        let tree = run(
            &["mktree"],
            Some(&format!("100644 blob {blob}\t{entry_name}\n")),
        );
        let commit = run(&["commit-tree", &tree, "-m", "crafted"], None);
        (dir, base, commit)
    }

    #[test]
    fn a_tree_git_refuses_to_check_out_is_a_delivered_tree_fault() {
        let (dir, _base, commit) = commit_with_tree_entry(".git");

        // The commit exists, so the upstream resolve check passes it ...
        assert_eq!(check_commit(&dir, &commit), CommitCheck::Resolves);

        // ... and the refusal at checkout is typed as the worker's own doing:
        // the control checkout, in a repository Cortex made itself, works
        // through the same machinery.
        match TreeCheckout::create(&dir, &commit) {
            Err(TreeCheckoutError::DeliveredTree(message)) => {
                assert!(message.contains("git worktree add failed"), "{message}");
            }
            Err(other) => panic!("expected a delivered-tree fault, got {other:?}"),
            Ok(_) => panic!("git must refuse a tree with a .git entry"),
        }
    }

    #[test]
    fn a_tree_entry_name_over_the_filesystem_limit_is_a_delivered_tree_fault() {
        // 300 bytes is over NAME_MAX (255), so the checkout fails with
        // "File name too long" -- which says nothing about Cortex. Classifying
        // on that text is what once let this be absorbed as free work.
        let long_name = "a".repeat(300);
        let (dir, _base, commit) = commit_with_tree_entry(&long_name);
        assert_eq!(check_commit(&dir, &commit), CommitCheck::Resolves);

        match TreeCheckout::create(&dir, &commit) {
            Err(TreeCheckoutError::DeliveredTree(_)) => {}
            Err(other) => panic!("expected a delivered-tree fault, got {other:?}"),
            Ok(_) => panic!("a 300-byte entry name cannot be checked out"),
        }
    }

    #[test]
    fn a_failing_control_checkout_is_a_cortex_fault() {
        // The delivered tree is refused, but so is the control (the scratch
        // directory cannot be created: its parent is a regular file): git or
        // the disk is what is failing, so the refusal proves nothing about the
        // worker and is absorbed.
        let (dir, _base, commit) = commit_with_tree_entry(".git");
        let file = tempfile::tempdir().unwrap().keep().join("not-a-directory");
        std::fs::write(&file, "x").unwrap();
        match TreeCheckout::create_in(&file.join("scratch"), &dir, &commit) {
            Err(TreeCheckoutError::Cortex(message)) => {
                assert!(message.contains("control checkout failed too"), "{message}");
            }
            Err(other) => panic!("expected a Cortex-side fault, got {other:?}"),
            Ok(_) => panic!("git must refuse a tree with a .git entry"),
        }
    }

    #[test]
    fn a_checkout_in_a_non_repository_is_a_cortex_fault() {
        // No `.git` at all: nothing to check out of, and that says nothing
        // about the tree's contents.
        let dir = tempfile::tempdir().unwrap().keep();
        let missing = "0000000000000000000000000000000000000000";
        match TreeCheckout::create(&dir, missing) {
            Err(TreeCheckoutError::Cortex(_)) => {}
            Err(other) => panic!("expected a Cortex-side fault, got {other:?}"),
            Ok(_) => panic!("there is nothing to check out"),
        }
    }

    /// Everything a worker can plant in the workspace repository to make git
    /// misbehave: an executable `post-checkout` hook that fails, in both the
    /// default hooks directory and one named by `core.hooksPath`, and an
    /// attributes file applying a required smudge filter that always fails.
    #[cfg(unix)]
    fn plant_hostile_repo_state(dir: &Path) {
        use std::os::unix::fs::PermissionsExt as _;

        let install_hook = |hooks: &Path| {
            std::fs::create_dir_all(hooks).unwrap();
            let hook = hooks.join("post-checkout");
            std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        install_hook(&dir.join(".git/hooks"));
        let hooks_path = dir.join("hostile-hooks");
        install_hook(&hooks_path);
        let attributes = dir.join("hostile-attributes");
        std::fs::write(&attributes, "* filter=x\n").unwrap();

        let config = |key: &str, value: &str| {
            let out = std::process::Command::new("git")
                .args(["config", key, value])
                .current_dir(dir)
                .output()
                .expect("git runs");
            assert!(out.status.success(), "git config {key} failed");
        };
        config("core.hooksPath", hooks_path.to_str().unwrap());
        config("core.attributesFile", attributes.to_str().unwrap());
        config("filter.x.smudge", "false");
        config("filter.x.required", "true");
    }

    #[cfg(unix)]
    #[test]
    fn worker_planted_hooks_and_filters_do_not_stop_a_good_checkout() {
        let (dir, base, _commit) = commit_with_tree_entry(".git");
        plant_hostile_repo_state(&dir);

        // The planted state really is hostile: a bare git cannot check out
        // the good commit through it.
        let bare = tempfile::tempdir().unwrap().keep().join("bare");
        let naive = std::process::Command::new("git")
            .args(["worktree", "add", "--detach"])
            .arg(&bare)
            .arg(&base)
            .current_dir(&dir)
            .output()
            .expect("git runs");
        assert!(
            !naive.status.success(),
            "the planted hooks and filters must break an unhardened checkout"
        );
        let _ = std::fs::remove_dir_all(&bare);

        let checkout = TreeCheckout::create(&dir, &base)
            .expect("hooks and filters in the worker-writable repository must not apply");
        assert!(checkout.path.join("file.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn worker_planted_state_does_not_change_who_pays_for_a_refused_tree() {
        // With the same hostile repository state, a tree git refuses is
        // still the worker's: the control never touches the workspace.
        let (dir, _base, commit) = commit_with_tree_entry(".git");
        plant_hostile_repo_state(&dir);
        match TreeCheckout::create(&dir, &commit) {
            Err(TreeCheckoutError::DeliveredTree(_)) => {}
            Err(other) => panic!("expected a delivered-tree fault, got {other:?}"),
            Ok(_) => panic!("git must refuse a tree with a .git entry"),
        }
    }

    /// Every file under `dir`, recursively.
    fn count_files(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| {
                        if entry.path().is_dir() {
                            count_files(&entry.path())
                        } else {
                            1
                        }
                    })
                    .sum()
            })
            .unwrap_or(0)
    }

    #[test]
    fn the_control_leaves_nothing_behind_and_mints_nothing_in_the_workspace() {
        let (dir, _base, commit) = commit_with_tree_entry(".git");
        let parent = tempfile::tempdir().unwrap();
        let objects_before = count_files(&dir.join(".git/objects"));

        match TreeCheckout::create_in(parent.path(), &dir, &commit) {
            Err(TreeCheckoutError::DeliveredTree(_)) => {}
            Err(other) => panic!("expected a delivered-tree fault, got {other:?}"),
            Ok(_) => panic!("git must refuse a tree with a .git entry"),
        }

        assert_eq!(
            std::fs::read_dir(parent.path()).unwrap().count(),
            0,
            "the failed checkout and the control must both be removed"
        );
        assert_eq!(
            count_files(&dir.join(".git/objects")),
            objects_before,
            "the control must not write objects into the workspace repository"
        );
    }

    #[test]
    fn the_control_checkout_works_and_cleans_up_after_itself() {
        let parent = tempfile::tempdir().unwrap();
        control_checkout(parent.path()).expect("a control checkout must work");
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
    }

    #[test]
    fn the_free_space_floor_only_applies_when_the_space_is_known() {
        assert_eq!(
            has_room(&std::env::temp_dir().join("no-such-dir-xyz")),
            Ok(())
        );
        #[cfg(unix)]
        assert!(free_bytes(&std::env::temp_dir()).is_some());
    }

    #[test]
    fn a_git_dir_git_refuses_is_missing_and_charged_not_could_not_check() {
        // `.git` exists but git will not open it (a repository format from
        // the future). The workspace is worker-writable, so that is the
        // worker's doing: `Missing`, never `CheckFailed`.
        let (dir, head) = repo_with_one_commit();
        assert_eq!(check_commit(&dir, &head), CommitCheck::Resolves);
        let config = dir.join(".git/config");
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.contains("repositoryformatversion = 0"), "{text}");
        std::fs::write(
            &config,
            text.replace(
                "repositoryformatversion = 0",
                "repositoryformatversion = 99",
            ),
        )
        .unwrap();
        assert_eq!(check_commit(&dir, &head), CommitCheck::Missing);
    }

    #[test]
    fn check_commit_separates_missing_from_could_not_check() {
        let (dir, head) = repo_with_one_commit();
        assert_eq!(check_commit(&dir, &head), CommitCheck::Resolves);
        assert_eq!(
            check_commit(&dir, "0000000000000000000000000000000000000000"),
            CommitCheck::Missing
        );

        // Worker-controlled text that is not an object id never reaches git.
        for malformed in [
            "0\u{0}".to_string(),
            "not-hex".to_string(),
            String::new(),
            "g".repeat(40),
            head[..39].to_string(),
            "a".repeat(200 * 1024),
        ] {
            assert_eq!(
                check_commit(&dir, &malformed),
                CommitCheck::Missing,
                "a malformed commit must be Missing, not a spawn failure"
            );
        }

        let not_a_repo = tempfile::tempdir().unwrap().keep();
        assert!(matches!(
            check_commit(&not_a_repo, &head),
            CommitCheck::CheckFailed(_)
        ));
        assert!(matches!(
            check_commit(&not_a_repo.join("does-not-exist"), &head),
            CommitCheck::CheckFailed(_)
        ));
    }

    #[test]
    fn a_workspace_nested_in_another_repository_is_not_answered_by_it() {
        let (outer, head) = repo_with_one_commit();
        let nested = outer.join("workspace");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(
            matches!(check_commit(&nested, &head), CommitCheck::CheckFailed(_)),
            "the enclosing repository must not answer for a workspace that is not one"
        );
    }

    #[tokio::test]
    async fn a_head_whose_tree_cannot_be_read_is_a_delivered_tree_fault() {
        // The head commit object resolves, but its root tree is gone, so the
        // diff fails. The head resolves, so this is charged: nothing read from
        // the worker-writable repository can prove the fault is Cortex's.
        let db = test_db();
        let (dir, base, head) = repo_with_base_and_delivery(&["src/lib.rs"]);
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Strong, &base),
        ));
        let tree = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", &format!("{head}^{{tree}}")])
                .current_dir(&dir)
                .output()
                .expect("git runs")
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        let object = dir.join(".git/objects").join(&tree[..2]).join(&tree[2..]);
        assert!(object.exists(), "the head's root tree is a loose object");
        std::fs::remove_file(&object).unwrap();

        match exam_integrity(&db, &facts_for(&dir, &head, &run_id, "step-1")).await {
            ExamIntegrity::Unknown {
                delivered_tree_caused,
                ..
            } => assert!(
                delivered_tree_caused,
                "a head that resolves but whose diff fails is charged"
            ),
            other => panic!("expected unknown integrity, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_base_whose_objects_are_gone_is_a_delivered_tree_fault() {
        // The worker can delete objects from the store it writes to. The head
        // still resolves, so the failed diff is charged: the answer must not
        // turn on whether the base can still be read, because that is exactly
        // what the worker can steer.
        let db = test_db();
        let (dir, base, head) = repo_with_base_and_delivery(&["src/lib.rs"]);
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Strong, &base),
        ));
        let object = dir.join(".git/objects").join(&base[..2]).join(&base[2..]);
        assert!(object.exists(), "the base commit is a loose object");
        std::fs::remove_file(&object).unwrap();
        assert_eq!(check_commit(&dir, &head), CommitCheck::Resolves);

        match exam_integrity(&db, &facts_for(&dir, &head, &run_id, "step-1")).await {
            ExamIntegrity::Unknown {
                delivered_tree_caused,
                ..
            } => assert!(
                delivered_tree_caused,
                "deleting the base's objects from the workspace store is charged"
            ),
            other => panic!("expected unknown integrity, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_failed_diff_in_a_workspace_with_no_git_is_a_cortex_fault() {
        // No `.git` at all: Cortex cannot find out anything, which is its own
        // failure and the only diff failure that is absorbed.
        let db = test_db();
        let (dir, base, head) = repo_with_base_and_delivery(&["src/lib.rs"]);
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Strong, &base),
        ));
        std::fs::remove_dir_all(dir.join(".git")).unwrap();

        match exam_integrity(&db, &facts_for(&dir, &head, &run_id, "step-1")).await {
            ExamIntegrity::Unknown {
                delivered_tree_caused,
                ..
            } => assert!(
                !delivered_tree_caused,
                "a workspace with no .git is Cortex's fault"
            ),
            other => panic!("expected unknown integrity, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_delivered_tree_git_refuses_is_charged_not_absorbed() {
        let db = test_db();
        let (dir, base, head) = commit_with_tree_entry(".git");
        let (run_id, lease_gen) = seed_verifying_step(&db, "step-1", &head);
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();
        // `Authored` so exam integrity passes and the checkout is reached.
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            lease_gen,
            &contract_declaring(VerdictClass::Authored, &base),
        ));
        db.init_credit_balance("user-1", 1_000).expect("balance");
        settle_call_for(&db, "user-1", "attempt-1", 300_000);
        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed)]);

        assert_eq!(
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1")).await,
            Some(Verdict::Inconclusive)
        );
        assert_eq!(runner.calls(), 0);
        assert_eq!(
            ending_cause(&db, "attempt-1").as_deref(),
            Some("exam_tampered"),
            "a commit the worker crafted so that git refuses it is not free work"
        );
        assert_eq!(
            db.get_credit_balance("user-1").subscription_remaining,
            997,
            "300_000 micro-USD is exactly 3 whole credits at the seeded price"
        );
    }

    #[tokio::test]
    async fn an_overlong_tree_entry_is_charged_not_absorbed() {
        // "File name too long" is the checkout filesystem refusing what the
        // worker delivered. It used to be absorbed as free work.
        let db = test_db();
        let (dir, base, head) = commit_with_tree_entry(&"a".repeat(300));
        let (run_id, lease_gen) = seed_verifying_step(&db, "step-1", &head);
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            lease_gen,
            &contract_declaring(VerdictClass::Authored, &base),
        ));
        db.init_credit_balance("user-1", 1_000).expect("balance");
        settle_call_for(&db, "user-1", "attempt-1", 300_000);
        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed)]);

        assert_eq!(
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1")).await,
            Some(Verdict::Inconclusive)
        );
        assert_eq!(runner.calls(), 0);
        assert_eq!(
            ending_cause(&db, "attempt-1").as_deref(),
            Some("exam_tampered"),
            "an entry name the worker made too long for the filesystem is not free work"
        );
        assert_eq!(
            db.get_credit_balance("user-1").subscription_remaining,
            997,
            "300_000 micro-USD is exactly 3 whole credits at the seeded price"
        );
    }
}
