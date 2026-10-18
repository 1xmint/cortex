//! Git for missions: disposable worktrees and hardened git calls.
//!
//! A worker controls every file in its worktree, so git run there must not
//! execute anything the worker planted: hooks, an fsmonitor, a filter
//! driver chosen by `.gitattributes`, or a `.git` file pointing at a gitdir
//! the worker made. Every call goes through `git()`, which turns those off.

use crate::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Git's well-known empty tree: reading attributes from it means a
/// worktree's own `.gitattributes` is ignored.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg(format!("--attr-source={EMPTY_TREE}"))
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(["-c", "core.fsmonitor=false"])
        .args(["-c", "core.attributesFile=/dev/null"])
        .args([
            "-c",
            "user.name=Cortex",
            "-c",
            "user.email=cortex@localhost",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()?;
    if !out.status.success() {
        return Err(Error::Invalid(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn head(repo: &Path) -> Result<String> {
    git(repo, &["rev-parse", "HEAD"])
}

/// A disposable checkout for one worker session, detached at its base.
#[derive(Debug)]
pub struct Worktree {
    pub path: PathBuf,
    repo: PathBuf,
    git_file: String,
}

pub fn create_worktree(repo: &Path, base: &str, dest: &Path) -> Result<Worktree> {
    let dest_str = dest
        .to_str()
        .ok_or_else(|| Error::Invalid(format!("path is not UTF-8: {}", dest.display())))?;
    git(repo, &["worktree", "add", "-q", "--detach", dest_str, base])?;
    let git_file = std::fs::read_to_string(dest.join(".git"))?;
    Ok(Worktree {
        path: dest.to_path_buf(),
        repo: repo.to_path_buf(),
        git_file,
    })
}

impl Worktree {
    /// Refuses to run git if the worker replaced the `.git` file.
    fn checked_git(&self, args: &[&str]) -> Result<String> {
        match std::fs::read_to_string(self.path.join(".git")) {
            Ok(now) if now == self.git_file => git(&self.path, args),
            _ => Err(Error::Blocked("worktree .git was modified".into())),
        }
    }

    /// Commits every change; `None` when there was nothing to commit.
    pub fn commit_all(&self, message: &str) -> Result<Option<String>> {
        self.checked_git(&["add", "-A"])?;
        if self.checked_git(&["status", "--porcelain"])?.is_empty() {
            return Ok(None);
        }
        self.checked_git(&["commit", "-q", "--no-verify", "-m", message])?;
        self.checked_git(&["rev-parse", "HEAD"]).map(Some)
    }

    /// Removes the worktree and its folder, even if the worker damaged it.
    pub fn discard(self) -> Result<()> {
        let path = self.path.to_string_lossy().into_owned();
        let removed = git(&self.repo, &["worktree", "remove", "--force", &path]);
        if self.path.exists() {
            std::fs::remove_dir_all(&self.path)?;
        }
        if removed.is_err() {
            git(&self.repo, &["worktree", "prune"])?;
        }
        Ok(())
    }
}

/// Points `refs/heads/<branch>` at `commit`. The person's checked-out
/// branch is never moved.
pub fn accept(repo: &Path, branch: &str, commit: &str) -> Result<()> {
    let refname = format!("refs/heads/{branch}");
    git(repo, &["check-ref-format", &refname])?;
    if let Ok(current) = git(repo, &["symbolic-ref", "-q", "HEAD"])
        && current == refname
    {
        return Err(Error::Blocked(format!(
            "`{branch}` is the checked-out branch; Cortex will not move it"
        )));
    }
    git(repo, &["update-ref", &refname, commit])?;
    Ok(())
}
