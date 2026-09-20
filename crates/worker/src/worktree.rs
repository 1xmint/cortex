use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use cortex_core::error::CortexError;
use cortex_core::protocol::GitEvidence;

const DIFF_EXCERPT_BYTES: usize = 32 * 1024;

/// A guard that owns an isolated git worktree for a single step execution.
///
/// When dropped, it removes the worktree and deletes the ephemeral branch
/// (unless `keep_branch` is set, in which case only the worktree directory
/// is removed but the branch is preserved for PR creation).
pub struct WorktreeGuard {
    /// Path to the worktree on disk.
    worktree_path: PathBuf,
    /// The branch name created for this worktree.
    branch_name: String,
    /// The root of the main repository (needed for cleanup commands).
    repo_root: PathBuf,
    /// Whether cleanup has already been performed.
    cleaned: bool,
    /// When true, cleanup removes the worktree but keeps the branch alive
    /// so it can be pushed and used for a PR.
    keep_branch: bool,
}

impl WorktreeGuard {
    /// Returns the path to the isolated worktree directory.
    pub fn path(&self) -> &Path {
        &self.worktree_path
    }

    /// Returns the branch name created for this worktree (e.g. `cortex/step/{step_id}`).
    pub fn branch_name(&self) -> &str {
        &self.branch_name
    }

    /// Set whether the branch should be preserved after worktree cleanup.
    ///
    /// When `true`, `cleanup()` will remove the worktree directory but leave
    /// the branch intact so it can be pushed for PR creation.
    pub fn set_keep_branch(&mut self, keep: bool) {
        self.keep_branch = keep;
    }

    /// Check if the worktree has any uncommitted changes or untracked files.
    pub fn has_uncommitted_changes(&self) -> bool {
        let output = Command::new("git")
            .current_dir(&self.worktree_path)
            .args(["status", "--porcelain"])
            .output();

        match output {
            Ok(o) => !o.stdout.is_empty(),
            Err(_) => false,
        }
    }

    /// Check if the worktree branch has commits ahead of the given base commit.
    pub fn has_commits_ahead(&self, base_commit: &str) -> bool {
        let output = Command::new("git")
            .current_dir(&self.worktree_path)
            .args(["log", "--oneline", &format!("{base_commit}..HEAD")])
            .output();

        match output {
            Ok(o) => !String::from_utf8_lossy(&o.stdout).trim().is_empty(),
            Err(_) => false,
        }
    }

    /// Stage all changes and create a commit in the worktree.
    ///
    /// Returns the new commit hash, or `None` if there was nothing to commit.
    pub fn commit_changes(&self, message: &str) -> Result<Option<String>, CortexError> {
        // Stage all changes
        let add_output = Command::new("git")
            .current_dir(&self.worktree_path)
            .args(["add", "-A"])
            .output()
            .map_err(|e| CortexError::WorkerExecution(format!("git add failed: {e}")))?;

        if !add_output.status.success() {
            let stderr = String::from_utf8_lossy(&add_output.stderr);
            return Err(CortexError::WorkerExecution(format!(
                "git add failed: {stderr}"
            )));
        }

        // Check if there's anything staged
        let diff_output = Command::new("git")
            .current_dir(&self.worktree_path)
            .args(["diff", "--cached", "--quiet"])
            .output()
            .map_err(|e| CortexError::WorkerExecution(format!("git diff --cached failed: {e}")))?;

        if diff_output.status.success() {
            // Exit code 0 means no differences — nothing to commit
            return Ok(None);
        }

        // Commit
        let commit_output = Command::new("git")
            .current_dir(&self.worktree_path)
            .args(["commit", "-m", message])
            .output()
            .map_err(|e| CortexError::WorkerExecution(format!("git commit failed: {e}")))?;

        if !commit_output.status.success() {
            let stderr = String::from_utf8_lossy(&commit_output.stderr);
            return Err(CortexError::WorkerExecution(format!(
                "git commit failed: {stderr}"
            )));
        }

        // Get the new HEAD
        let head_output = Command::new("git")
            .current_dir(&self.worktree_path)
            .args(["rev-parse", "HEAD"])
            .output()
            .map_err(|e| CortexError::WorkerExecution(format!("git rev-parse HEAD failed: {e}")))?;

        if head_output.status.success() {
            let hash = String::from_utf8_lossy(&head_output.stdout)
                .trim()
                .to_string();
            tracing::info!(
                branch = %self.branch_name,
                commit = %hash,
                "auto-committed changes in worktree"
            );
            Ok(Some(hash))
        } else {
            Ok(None)
        }
    }

    /// Push the worktree branch to a remote.
    pub fn push_branch(&self, remote: &str) -> Result<(), CortexError> {
        let output = Command::new("git")
            .current_dir(&self.worktree_path)
            .args(["push", "-u", remote, &self.branch_name])
            .output()
            .map_err(|e| CortexError::WorkerExecution(format!("git push failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(CortexError::WorkerExecution(format!(
                "git push failed: {stderr}"
            )));
        }

        tracing::info!(
            branch = %self.branch_name,
            remote = %remote,
            "pushed worktree branch to remote"
        );
        Ok(())
    }

    /// Explicitly clean up the worktree and (optionally) the branch.
    ///
    /// This is also called automatically on drop, but calling it explicitly
    /// lets you handle errors. If `keep_branch` is set, only the worktree
    /// directory is removed; the branch is left intact for PR creation.
    pub fn cleanup(&mut self) -> Result<(), CortexError> {
        if self.cleaned {
            return Ok(());
        }
        self.cleaned = true;

        // Remove the worktree (--force in case there are untracked files)
        let remove_result = Command::new("git")
            .current_dir(&self.repo_root)
            .args(["worktree", "remove", "--force"])
            .arg(&self.worktree_path)
            .output();

        if let Err(e) = &remove_result {
            tracing::warn!(
                worktree = %self.worktree_path.display(),
                error = %e,
                "failed to run git worktree remove"
            );
        }

        if self.keep_branch {
            tracing::info!(
                branch = %self.branch_name,
                "keeping branch alive for PR creation"
            );
        } else {
            // Delete the ephemeral branch
            let branch_result = Command::new("git")
                .current_dir(&self.repo_root)
                .args(["branch", "-D", &self.branch_name])
                .output();

            if let Err(e) = &branch_result {
                tracing::warn!(
                    branch = %self.branch_name,
                    error = %e,
                    "failed to delete worktree branch"
                );
            }
        }

        Ok(())
    }
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        if !self.cleaned {
            if let Err(e) = self.cleanup() {
                tracing::error!(error = %e, "worktree cleanup failed in drop");
            }
        }
    }
}

/// Create an isolated git worktree for a step execution.
///
/// The worktree is placed at `{workspace_dir}/.cortex/worktrees/{step_id}` and
/// based on a new branch `cortex/step/{step_id}` created from the current HEAD.
///
/// Returns `None` if `workspace_dir` is not inside a git repository (graceful
/// fallback — the caller should use the original working directory instead).
pub fn create_worktree(workspace_dir: &Path, step_id: &str) -> Result<WorktreeGuard, CortexError> {
    // Verify this is a git repo by getting the repo root.
    let repo_root_output = Command::new("git")
        .current_dir(workspace_dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| CortexError::WorkerExecution(format!("git rev-parse failed: {e}")))?;

    if !repo_root_output.status.success() {
        return Err(CortexError::WorkerExecution(
            "not a git repository".to_string(),
        ));
    }

    let repo_root = PathBuf::from(
        String::from_utf8_lossy(&repo_root_output.stdout)
            .trim()
            .to_string(),
    );

    let worktree_path = workspace_dir
        .join(".cortex")
        .join("worktrees")
        .join(step_id);
    let branch_name = format!("cortex/step/{step_id}");

    // Ensure the parent directory exists.
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            CortexError::WorkerExecution(format!(
                "failed to create worktree parent dir {}: {e}",
                parent.display()
            ))
        })?;
    }

    // Create the worktree with a new branch from HEAD.
    let add_output = Command::new("git")
        .current_dir(&repo_root)
        .args(["worktree", "add", "-b", &branch_name])
        .arg(&worktree_path)
        .args(["HEAD"])
        .output()
        .map_err(|e| CortexError::WorkerExecution(format!("git worktree add failed: {e}")))?;

    if !add_output.status.success() {
        let stderr = String::from_utf8_lossy(&add_output.stderr);
        return Err(CortexError::WorkerExecution(format!(
            "git worktree add failed: {stderr}"
        )));
    }

    tracing::info!(
        worktree = %worktree_path.display(),
        branch = %branch_name,
        "created isolated worktree for step"
    );

    Ok(WorktreeGuard {
        worktree_path,
        branch_name,
        repo_root,
        cleaned: false,
        keep_branch: false,
    })
}

/// Collect git-derived evidence for a completed worker step.
///
/// Returns `None` when `working_dir` is not inside a git worktree. Changed
/// files are derived from git diff/status, never from provider stdout.
pub fn collect_git_evidence(
    working_dir: &Path,
    base_commit: Option<&str>,
    branch_override: Option<&str>,
) -> Option<GitEvidence> {
    if git_stdout(working_dir, &["rev-parse", "--is-inside-work-tree"])?.trim() != "true" {
        return None;
    }

    let head_commit = git_stdout(working_dir, &["rev-parse", "HEAD"]);
    let branch = branch_override
        .filter(|b| !b.trim().is_empty())
        .map(|b| b.to_string())
        .or_else(|| {
            git_stdout(working_dir, &["branch", "--show-current"]).filter(|b| !b.trim().is_empty())
        });
    let status_porcelain =
        git_stdout(working_dir, &["status", "--porcelain"]).filter(|s| !s.trim().is_empty());

    let mut changed_files = BTreeSet::new();
    if let Some(base) = base_commit.filter(|b| !b.trim().is_empty()) {
        collect_name_only(
            working_dir,
            &["diff", "--name-only", base],
            &mut changed_files,
        );
    } else {
        collect_name_only(
            working_dir,
            &["diff", "--name-only", "HEAD"],
            &mut changed_files,
        );
    }
    if let Some(status) = &status_porcelain {
        for path in parse_status_porcelain_paths(status) {
            changed_files.insert(path);
        }
    }

    let (diff_excerpt, diff_truncated) =
        collect_diff_excerpt(working_dir, base_commit.filter(|b| !b.trim().is_empty()));

    Some(GitEvidence {
        changed_files: changed_files.into_iter().collect(),
        diff_excerpt,
        diff_truncated,
        status_porcelain,
        base_commit: base_commit.map(|b| b.to_string()),
        head_commit,
        branch,
    })
}

fn collect_name_only(working_dir: &Path, args: &[&str], changed_files: &mut BTreeSet<String>) {
    if let Some(output) = git_stdout(working_dir, args) {
        for path in output.lines().map(str::trim).filter(|p| !p.is_empty()) {
            changed_files.insert(path.to_string());
        }
    }
}

fn collect_diff_excerpt(working_dir: &Path, base_commit: Option<&str>) -> (Option<String>, bool) {
    let output = if let Some(base) = base_commit {
        git_output(working_dir, &["diff", "--binary", base])
    } else {
        git_output(working_dir, &["diff", "--binary", "HEAD"])
    };

    let Some(output) = output.filter(|o| o.status.success() && !o.stdout.is_empty()) else {
        return (None, false);
    };

    let truncated = output.stdout.len() > DIFF_EXCERPT_BYTES;
    let bytes = if truncated {
        &output.stdout[..DIFF_EXCERPT_BYTES]
    } else {
        &output.stdout
    };
    let mut excerpt = String::from_utf8_lossy(bytes).to_string();
    while !excerpt.is_char_boundary(excerpt.len()) {
        excerpt.pop();
    }
    (Some(excerpt), truncated)
}

fn git_stdout(working_dir: &Path, args: &[&str]) -> Option<String> {
    let output = git_output(working_dir, args)?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string(),
    )
}

fn git_output(working_dir: &Path, args: &[&str]) -> Option<std::process::Output> {
    Command::new("git")
        .current_dir(working_dir)
        .args(args)
        .output()
        .ok()
}

fn parse_status_porcelain_paths(status: &str) -> Vec<String> {
    status
        .lines()
        .filter_map(|line| {
            let path = line.get(3..)?.trim();
            if path.is_empty() {
                return None;
            }
            Some(
                path.split_once(" -> ")
                    .map(|(_, new_path)| new_path)
                    .unwrap_or(path)
                    .to_string(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_name_format() {
        let name = format!("cortex/step/{}", "abc-123");
        assert_eq!(name, "cortex/step/abc-123");
    }

    #[test]
    fn parses_porcelain_paths() {
        let paths = parse_status_porcelain_paths(
            " M crates/core/src/protocol.rs\n\
             ?? crates/worker/src/worktree.rs\n\
             R  old.rs -> new.rs\n",
        );
        assert_eq!(
            paths,
            vec![
                "crates/core/src/protocol.rs",
                "crates/worker/src/worktree.rs",
                "new.rs"
            ]
        );
    }
}
