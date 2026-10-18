use cortex::Error;
use cortex::git::{accept, create_worktree, head};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Plain, unhardened git, as a person would run it.
fn plain_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository with one commit on `main`.
fn repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    plain_git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("readme.txt"), "hello\n").unwrap();
    plain_git(&repo, &["add", "."]);
    plain_git(&repo, &["commit", "-qm", "first"]);
    (dir, repo)
}

#[test]
fn worktree_starts_at_base() {
    let (dir, repo) = repo();
    let base = head(&repo).unwrap();
    assert_eq!(base.len(), 40);
    let wt = create_worktree(&repo, &base, &dir.path().join("wt")).unwrap();
    assert_eq!(plain_git(&wt.path, &["rev-parse", "HEAD"]), base);
    assert_eq!(
        std::fs::read_to_string(wt.path.join("readme.txt")).unwrap(),
        "hello\n"
    );
}

#[test]
fn commit_all_returns_none_when_clean() {
    let (dir, repo) = repo();
    let base = head(&repo).unwrap();
    let wt = create_worktree(&repo, &base, &dir.path().join("wt")).unwrap();
    assert_eq!(wt.commit_all("nothing").unwrap(), None);
    // Positive control: a real change does produce a commit.
    std::fs::write(wt.path.join("new.txt"), "x").unwrap();
    let commit = wt.commit_all("add new").unwrap().expect("a commit");
    assert_eq!(plain_git(&wt.path, &["rev-parse", "HEAD"]), commit);
    assert_eq!(plain_git(&wt.path, &["rev-parse", "HEAD~1"]), base);
}

#[test]
fn discard_removes_worktree() {
    let (dir, repo) = repo();
    let base = head(&repo).unwrap();
    let dest = dir.path().join("wt");
    let wt = create_worktree(&repo, &base, &dest).unwrap();
    std::fs::write(dest.join("junk.txt"), "x").unwrap();
    assert!(plain_git(&repo, &["worktree", "list"]).contains("wt"));
    wt.discard().unwrap();
    assert!(!dest.exists());
    assert!(!plain_git(&repo, &["worktree", "list"]).contains("wt"));
}

#[test]
fn accept_moves_branch_only() {
    let (dir, repo) = repo();
    let base = head(&repo).unwrap();
    let wt = create_worktree(&repo, &base, &dir.path().join("wt")).unwrap();
    std::fs::write(wt.path.join("new.txt"), "x").unwrap();
    let commit = wt.commit_all("add new").unwrap().unwrap();
    accept(&repo, "cortex/m1", &commit).unwrap();
    assert_eq!(
        plain_git(&repo, &["rev-parse", "refs/heads/cortex/m1"]),
        commit
    );
    // The person's checkout did not move and gained no files.
    assert_eq!(plain_git(&repo, &["rev-parse", "HEAD"]), base);
    assert_eq!(
        plain_git(&repo, &["symbolic-ref", "HEAD"]),
        "refs/heads/main"
    );
    assert!(!repo.join("new.txt").exists());
    assert_eq!(plain_git(&repo, &["status", "--porcelain"]), "");
    // Moving the checked-out branch is refused.
    assert!(matches!(
        accept(&repo, "main", &commit),
        Err(Error::Blocked(_))
    ));
    assert_eq!(plain_git(&repo, &["rev-parse", "main"]), base);
}

#[cfg(unix)]
fn executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn git_ignores_planted_hooks_and_gitdir() {
    let (dir, repo) = repo();
    let base = head(&repo).unwrap();
    let marker = dir.path().join("marker");
    let touch = format!("#!/bin/sh\ntouch {}\nexit 0\n", marker.display());

    // Case 1: the worker points `.git` at a gitdir it controls, whose hook
    // and fsmonitor would run code.
    let evil = dir.path().join("evil");
    std::fs::create_dir(&evil).unwrap();
    plain_git(&evil, &["init", "-q"]);
    let evil_git = evil.join(".git");
    executable(&evil_git.join("hooks/pre-commit"), &touch);
    executable(&dir.path().join("fsmon.sh"), &touch);
    plain_git(
        &evil,
        &[
            "config",
            "core.fsmonitor",
            &dir.path().join("fsmon.sh").display().to_string(),
        ],
    );
    let wt = create_worktree(&repo, &base, &dir.path().join("wt")).unwrap();
    std::fs::write(
        wt.path.join(".git"),
        format!("gitdir: {}\n", evil_git.display()),
    )
    .unwrap();
    std::fs::write(wt.path.join("a.txt"), "a").unwrap();
    // Positive control: plain git in that folder does run the planted code.
    plain_git(&wt.path, &["add", "."]);
    plain_git(&wt.path, &["commit", "-qm", "x"]);
    assert!(
        marker.exists(),
        "control: planted code should run under plain git"
    );
    std::fs::remove_file(&marker).unwrap();
    assert!(matches!(wt.commit_all("x"), Err(Error::Blocked(_))));
    assert!(!marker.exists());

    // Case 2: a `.gitattributes` filter whose driver exists in config.
    plain_git(
        &repo,
        &[
            "config",
            "filter.evil.clean",
            &format!("touch {}; cat", marker.display()),
        ],
    );
    let wt2 = create_worktree(&repo, &base, &dir.path().join("wt2")).unwrap();
    std::fs::write(wt2.path.join(".gitattributes"), "* filter=evil\n").unwrap();
    std::fs::write(wt2.path.join("b.txt"), "b").unwrap();
    // Positive control, in a separate checkout of the same repo.
    let ctl = dir.path().join("ctl");
    plain_git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            ctl.to_str().unwrap(),
            &base,
        ],
    );
    std::fs::write(ctl.join(".gitattributes"), "* filter=evil\n").unwrap();
    std::fs::write(ctl.join("b.txt"), "b").unwrap();
    plain_git(&ctl, &["add", "."]);
    assert!(
        marker.exists(),
        "control: filter should run under plain git"
    );
    std::fs::remove_file(&marker).unwrap();
    let commit = wt2.commit_all("with attributes").unwrap();
    assert!(commit.is_some());
    assert!(!marker.exists(), "filter driver ran under Cortex's git");
}
