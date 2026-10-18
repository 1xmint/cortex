//! Git for missions: disposable worktrees and hardened git calls.
//!
//! A worker controls every file in its worktree, so git run there must not
//! execute anything the worker planted: hooks, an fsmonitor, a filter
//! driver chosen by `.gitattributes`, or a `.git` file pointing at a gitdir
//! the worker made. Every call goes through `git()`, which turns those off.
//! A repository the worker made inside its worktree has its own config and
//! attributes, which those switches do not reach, so it is removed before
//! any git call looks at the files.

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
        .args(["-c", "diff.ignoreSubmodules=all"])
        .args(["-c", "submodule.recurse=false"])
        .args([
            "-c",
            "user.name=Cortex",
            "-c",
            "user.email=cortex@localhost",
        ])
        .args(args)
        .current_dir(dir)
        .env_clear()
        .envs(std::env::var_os("PATH").map(|p| ("PATH", p)))
        .envs(std::env::var_os("SYSTEMROOT").map(|p| ("SYSTEMROOT", p)))
        .env("HOME", "/")
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

/// Makes `dir` a repository on branch `main` whose first commit holds
/// everything already in it.
pub fn init_and_commit(dir: &Path, message: &str) -> Result<()> {
    git(dir, &["init", "-q", "-b", "main"])?;
    git(dir, &["add", "-A"])?;
    git(dir, &["commit", "-q", "--allow-empty", "-m", message])?;
    Ok(())
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

/// Reattaches to a worktree Cortex made earlier, refusing it unless its
/// `.git` file still points at the repository's own record of it.
pub fn open_worktree(repo: &Path, dest: &Path) -> Result<Worktree> {
    let modified = || Error::Blocked("worktree .git was modified".into());
    let git_file = std::fs::read_to_string(dest.join(".git")).map_err(|_| modified())?;
    let gitdir = git_file
        .trim()
        .strip_prefix("gitdir: ")
        .ok_or_else(modified)?;
    let common = git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let gitdir = std::fs::canonicalize(gitdir).map_err(|_| modified())?;
    let records = std::fs::canonicalize(Path::new(&common).join("worktrees"))?;
    let back = std::fs::read_to_string(gitdir.join("gitdir")).map_err(|_| modified())?;
    let points_back =
        std::fs::canonicalize(back.trim()).ok() == std::fs::canonicalize(dest.join(".git")).ok();
    if gitdir.parent() != Some(records.as_path()) || !points_back {
        return Err(modified());
    }
    Ok(Worktree {
        path: dest.to_path_buf(),
        repo: repo.to_path_buf(),
        git_file,
    })
}

/// Removes a worktree's folder and git's record of it, whether or not
/// either still exists.
pub fn forget_worktree(repo: &Path, dest: &Path) -> Result<()> {
    let path = dest.to_string_lossy().into_owned();
    let _ = git(repo, &["worktree", "remove", "--force", &path]);
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    git(repo, &["worktree", "prune"])?;
    Ok(())
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
        remove_nested_git(&self.path, true)?;
        self.checked_git(&["add", "-A"])?;
        if self
            .checked_git(&["status", "--porcelain", "--ignore-submodules=all"])?
            .is_empty()
        {
            return Ok(None);
        }
        self.checked_git(&["commit", "-q", "--no-verify", "-m", message])?;
        self.checked_git(&["rev-parse", "HEAD"]).map(Some)
    }

    /// Removes the worktree and its folder, even if the worker damaged it.
    pub fn discard(self) -> Result<()> {
        forget_worktree(&self.repo, &self.path)
    }
}

/// Deletes every `.git` below `dir` (but not the worktree's own), so files
/// in a repository the worker made are committed as plain files and no
/// git call reads that repository's config. Symlinks are never followed.
fn remove_nested_git(dir: &Path, top: bool) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if entry.file_name() == ".git" {
            if top {
                continue;
            }
            if kind.is_dir() {
                std::fs::remove_dir_all(entry.path())?;
            } else {
                std::fs::remove_file(entry.path())?;
            }
        } else if kind.is_dir() {
            remove_nested_git(&entry.path(), false)?;
        }
    }
    Ok(())
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
