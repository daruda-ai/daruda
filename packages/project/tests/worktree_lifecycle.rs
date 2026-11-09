//! A lane's git worktree through the package's public API alone: create it
//! on a new branch, find it again, remove it, and remove it a second time.

use std::path::Path;

use daruda_project::lane::git::{Git, GitWorktreeInfo, SystemGit, isolate_git_config_for_tests};

/// The real git backend, with the developer's own config swapped out first.
fn system_git() -> SystemGit {
    isolate_git_config_for_tests();
    SystemGit
}

// The twin of `tests/it/fixture.rs::git`; this file is its own test binary.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = daruda_core::process::command("git")
        .envs(isolate_git_config_for_tests())
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git spawns");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn lanes(repo: &Path) -> Vec<GitWorktreeInfo> {
    system_git().probe(repo).expect("a repository").lanes
}

#[test]
fn a_lane_worktree_is_created_listed_and_removed() {
    if !system_git().available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    // `/tmp` is a symlink on macOS; git reports resolved paths.
    let base = daruda_core::path::canonicalize(temp.path()).unwrap();
    let repo = base.join("repo");
    let lane = base.join("repo-feature");

    std::fs::create_dir_all(&repo).unwrap();
    system_git().init(&repo).unwrap();
    git(
        &repo,
        &["commit", "--allow-empty", "--quiet", "-m", "initial"],
    );
    assert_eq!(
        system_git().probe(&repo).map(|probe| probe.repo_root),
        Some(repo.clone())
    );

    let main = system_git().open(&repo);
    main.add_lane(&lane, Some("feature"), None).unwrap();
    assert_eq!(git(&lane, &["symbolic-ref", "--short", "HEAD"]), "feature");
    let listed = lanes(&repo);
    assert!(
        listed
            .iter()
            .any(|w| w.path == lane && w.branch.as_deref() == Some("feature")),
        "new lane is listed: {listed:?}"
    );

    main.remove_lane(&lane, false).unwrap();
    assert!(!lane.exists());
    assert!(lanes(&repo).iter().all(|w| w.path != lane));
    // A lane already gone is the outcome the caller asked for.
    main.remove_lane(&lane, false).unwrap();
    main.delete_branch("feature").unwrap();
}
