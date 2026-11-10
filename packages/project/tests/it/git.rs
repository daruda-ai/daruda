//! Public API tests for git.

use crate::fixture::{git, git_output, has_git, init, lanes, open, repo_root, system_git};
use daruda_project::lane::git::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

// Temp-dir helpers — each test gets a unique path so parallel
// runs don't collide. The directory is created on demand and
// torn down at the end of the test.
fn unique_tmpdir(prefix: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let id = SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("daruda_git_{prefix}_{pid}_{nonce}_{id}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Resolve symlinks (on macOS `/tmp` → `/private/tmp`) so
    // direct PathBuf comparisons against git's resolved output
    // don't break.
    daruda_core::path::canonicalize(&dir).unwrap()
}

fn teardown(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
}

// Skip-if-no-git guard for the whole module.
fn require_git() -> bool {
    has_git()
}

fn commit_initial(repo: &Path) {
    git(repo, &["config", "user.email", "daruda@test"]);
    git(repo, &["config", "user.name", "daruda"]);
    git(repo, &["commit", "--allow-empty", "-m", "initial"]);
}

/// What `-z` is for: without it git C-quotes a path holding a space or a
/// non-ASCII byte (`"\355\225\234…"`), and the quoted text was taken as the
/// path — so staging, diffs and opening that file all failed.
#[test]
fn a_real_status_reports_spaced_and_non_ascii_paths_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let git = |args: &[&str]| {
        git(root, args);
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    std::fs::write(root.join("old name.rs"), "a\nb\n").unwrap();
    std::fs::write(root.join("한글.md"), "x\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "init"]);
    git(&["mv", "old name.rs", "새 이름.rs"]);
    std::fs::write(root.join("한글.md"), "x\ny\n").unwrap();
    std::fs::write(root.join("untracked 파일.txt"), "").unwrap();

    let status = open(root).worktree_status().unwrap();
    let staged: Vec<_> = status.staged.iter().map(|e| e.path.clone()).collect();
    let unstaged: Vec<_> = status.unstaged.iter().map(|e| e.path.clone()).collect();
    assert_eq!(staged, vec![PathBuf::from("새 이름.rs")]);
    assert_eq!(
        status.staged[0].original_path,
        Some(PathBuf::from("old name.rs"))
    );
    assert_eq!(
        unstaged,
        vec![
            PathBuf::from("한글.md"),
            PathBuf::from("untracked 파일.txt")
        ]
    );
    assert_eq!(status.diffstat.get(Path::new("한글.md")), Some(&(1, 0)));
    // A pure rename counts no lines, keyed by its destination only — what
    // rename detection gives; without it the move is a delete plus an add.
    assert_eq!(status.diffstat.get(Path::new("새 이름.rs")), Some(&(0, 0)));
    assert!(!status.diffstat.contains_key(Path::new("old name.rs")));
}

#[test]
fn init_creates_repo() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("init");
    init(&dir);
    assert_eq!(repo_root(&dir).as_deref(), Some(dir.as_path()));
    teardown(&dir);
}

#[test]
fn probe_finds_no_repo_in_a_plain_dir() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("plain");
    assert!(system_git().probe(&dir).is_none());
    teardown(&dir);
}

#[test]
fn default_branch_falls_back_to_the_checked_out_branch() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("branch");
    init(&dir);
    commit_initial(&dir);
    // Neither `origin/HEAD` nor `main`/`master` exists, so only the
    // checked-out branch can answer.
    git(&dir, &["branch", "-m", "trunk"]);
    assert_eq!(open(&dir).default_branch().as_deref(), Some("trunk"));
    teardown(&dir);
}

#[test]
fn head_message_errors_when_no_commits() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("headmsg_empty");
    init(&dir);
    // No commit yet → `git log -1` fails; the empty-box amend path turns this
    // into the "nothing to amend" toast.
    assert!(open(&dir).head_message().is_err());
    teardown(&dir);
}

#[test]
fn default_branch_none_on_non_repo() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("default_nonrepo");
    // Plain directory, never `git init`-ed.
    assert_eq!(open(&dir).default_branch(), None);
    teardown(&dir);
}

#[test]
fn add_and_list_worktrees_round_trip() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("addlist");
    init(&dir);
    commit_initial(&dir);

    let wt_path = dir.join("wt-feature");
    open(&dir)
        .add_lane(&wt_path, Some("feat/xyz"), None)
        .unwrap();

    let listed = lanes(&dir);
    assert_eq!(listed.len(), 2);
    let feat = listed.iter().find(|w| w.path == wt_path).unwrap();
    assert_eq!(feat.branch.as_deref(), Some("feat/xyz"));
    teardown(&dir);
}

#[test]
fn remove_worktree_deletes_entry() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("remove");
    init(&dir);
    commit_initial(&dir);

    let wt_path = dir.join("wt-remove");
    open(&dir)
        .add_lane(&wt_path, Some("scratch"), None)
        .unwrap();
    assert_eq!(lanes(&dir).len(), 2);

    open(&dir).remove_lane(&wt_path, false).unwrap();
    let listed = lanes(&dir);
    assert_eq!(listed.len(), 1);
    assert!(listed.iter().all(|w| w.path != wt_path));
    teardown(&dir);
}

#[test]
fn remove_lane_succeeds_when_the_worktree_was_already_removed_elsewhere() {
    // Mirrors the reported bug: the worktree was already fully removed
    // (directory + git registration) from a *different* environment —
    // e.g. another terminal running `git worktree remove` directly, or a
    // prior daruda removal that raced. A second removal attempt from
    // daruda's UI must not get stuck: git reports "is not a working
    // tree" (exit 128) because there is genuinely nothing left to
    // remove, and that outcome — no worktree at this path — is already
    // what the user wanted.
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("extfullremove");
    init(&dir);
    commit_initial(&dir);

    let wt_path = dir.join("wt-extfull");
    open(&dir)
        .add_lane(&wt_path, Some("scratch/extfull"), None)
        .unwrap();
    open(&dir).remove_lane(&wt_path, false).unwrap(); // the "external" removal

    // daruda's Lane entry is still sitting in the list at this point —
    // the user now clicks Remove in the app for the same target.
    open(&dir).remove_lane(&wt_path, false).unwrap();
    teardown(&dir);
}

#[test]
fn add_worktree_with_existing_branch_fails_cleanly() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("dup");
    init(&dir);
    commit_initial(&dir);

    let first = dir.join("wt-first");
    open(&dir)
        .add_lane(&first, Some("dup-branch"), None)
        .unwrap();

    // Re-using `-b dup-branch` must fail — ensures GitError::Exit
    // carries a useful message.
    let second = dir.join("wt-second");
    let err = open(&dir)
        .add_lane(&second, Some("dup-branch"), None)
        .unwrap_err();
    match err {
        GitError::Exit { stderr, .. } => {
            assert!(!stderr.is_empty());
        }
        other => panic!("expected Exit error, got {other:?}"),
    }
    teardown(&dir);
}

#[test]
fn delete_branch_after_worktree_removal() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("delbranch");
    init(&dir);
    commit_initial(&dir);

    // Create a lane (creates the branch as a side effect),
    // then remove the lane so the branch is no longer checked
    // out anywhere — the modal flow does this same sequence.
    let wt = dir.join("wt-temp");
    open(&dir).add_lane(&wt, Some("temp/work"), None).unwrap();
    open(&dir).remove_lane(&wt, false).unwrap();

    // Branch still exists post-remove (git keeps it).
    let pre = git_output(&dir, &["rev-parse", "--verify", "temp/work"]);
    assert!(pre.status.success(), "branch should exist before delete");

    open(&dir).delete_branch("temp/work").unwrap();

    let post = git_output(&dir, &["rev-parse", "--verify", "temp/work"]);
    assert!(!post.status.success(), "branch must be gone after delete");
    teardown(&dir);
}

#[test]
fn delete_branch_rejects_currently_checked_out() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("delbranch_curr");
    init(&dir);
    commit_initial(&dir);
    // The default branch is currently checked out at `dir`.
    // Attempting to delete it must fail cleanly.
    let current = git(&dir, &["symbolic-ref", "--short", "HEAD"]);
    let err = open(&dir).delete_branch(&current).unwrap_err();
    match err {
        GitError::Exit { stderr, .. } => assert!(!stderr.is_empty()),
        other => panic!("expected Exit error, got {other:?}"),
    }
    teardown(&dir);
}

#[test]
fn git_add_stages_file() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("git_add");
    init(&dir);
    commit_initial(&dir);

    let file = dir.join("hello.txt");
    std::fs::write(&file, "hello").unwrap();

    // File is untracked — not in staged before add.
    let before = open(&dir).worktree_status().unwrap();
    assert!(before.staged.is_empty(), "nothing staged before git_add");

    open(&dir).add(std::path::Path::new("hello.txt")).unwrap();

    let after = open(&dir).worktree_status().unwrap();
    assert_eq!(after.staged.len(), 1, "one staged entry after git_add");
    assert_eq!(after.staged[0].x, 'A');
    teardown(&dir);
}

#[test]
fn git_add_all_stages_all_files() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("git_add_all");
    init(&dir);
    commit_initial(&dir);

    std::fs::write(dir.join("a.txt"), "a").unwrap();
    std::fs::write(dir.join("b.txt"), "b").unwrap();

    open(&dir).add_all().unwrap();

    let status = open(&dir).worktree_status().unwrap();
    assert_eq!(status.staged.len(), 2, "both files staged after add --all");
    teardown(&dir);
}

#[test]
fn git_restore_staged_unstages_file() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("git_restore");
    init(&dir);
    commit_initial(&dir);

    let rel = std::path::Path::new("restore_me.txt");
    std::fs::write(dir.join(rel), "data").unwrap();
    open(&dir).add(rel).unwrap();

    let after_add = open(&dir).worktree_status().unwrap();
    assert_eq!(after_add.staged.len(), 1, "file staged after add");

    open(&dir).restore_staged(rel).unwrap();

    let after_restore = open(&dir).worktree_status().unwrap();
    assert!(
        after_restore.staged.is_empty(),
        "nothing staged after restore --staged"
    );
    assert_eq!(
        after_restore.unstaged.len(),
        1,
        "file appears as untracked after unstage"
    );
    teardown(&dir);
}

#[test]
fn add_worktree_with_explicit_base_ref() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("base_ref");
    init(&dir);
    commit_initial(&dir);
    // Branch "side" at the initial commit, then advance "main"
    // by another empty commit so HEAD diverges from "side".
    git(&dir, &["branch", "side"]);
    git(&dir, &["commit", "--allow-empty", "-m", "advance-main"]);

    // Build the lane with explicit base = "side". The new
    // branch must point at "side"'s commit, not the current HEAD.
    let wt_path = dir.join("wt-from-side");
    open(&dir)
        .add_lane(&wt_path, Some("feat/from-side"), Some("side"))
        .unwrap();

    let side_sha = git_output(&dir, &["rev-parse", "side"]).stdout;
    let wt_sha = git_output(&wt_path, &["rev-parse", "HEAD"]).stdout;
    assert_eq!(side_sha, wt_sha, "lane must inherit base ref's commit");
    teardown(&dir);
}

/// The split this probe exists for. A linked worktree keeps `index` and
/// `HEAD` to itself while sharing `refs/`, so watching only one of the two
/// directories misses either the staging or the fetch.
#[test]
fn git_dirs_separate_a_linked_worktree_from_the_shared_repository() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("git_dirs");
    init(&dir);
    commit_initial(&dir);
    let linked = dir.join("side-wt");
    open(&dir).add_lane(&linked, Some("side"), None).unwrap();

    let main = open(&dir).dirs().unwrap();
    assert!(
        main.git_dir.is_absolute(),
        "relative paths must be resolved"
    );
    assert_eq!(
        main.git_dir, main.common_dir,
        "the main worktree's own dir is the shared one"
    );

    let side = open(&linked).dirs().unwrap();
    assert_ne!(
        side.git_dir, side.common_dir,
        "a linked worktree keeps its own index and HEAD"
    );
    assert_eq!(
        side.common_dir, main.common_dir,
        "both lanes share the refs a fetch moves"
    );
    assert!(side.git_dir.starts_with(&side.common_dir));

    teardown(&dir);
}

fn regression_repo(committed: bool) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    init(temp.path());
    if committed {
        commit_initial(temp.path());
    }
    temp
}

#[test]
fn explicit_file_staging_and_diffs_use_literal_pathspecs() {
    for batch in [false, true] {
        let temp = regression_repo(true);
        let root = temp.path();
        for name in ["[id].tsx", "i.tsx"] {
            std::fs::write(root.join(name), "base\n").unwrap();
        }
        open(root).add_all().unwrap();
        open(root).commit("base").unwrap();
        for (name, text) in [("[id].tsx", "selected\n"), ("i.tsx", "sibling\n")] {
            std::fs::write(root.join(name), text).unwrap();
        }
        if batch {
            open(root).add_paths(&[PathBuf::from("[id].tsx")]).unwrap();
        } else {
            open(root).add(Path::new("[id].tsx")).unwrap();
        }
        let staged = open(root).worktree_status().unwrap().staged;
        assert_eq!(
            staged.iter().map(|e| e.path.as_path()).collect::<Vec<_>>(),
            vec![Path::new("[id].tsx")]
        );
        open(root).add_all().unwrap();
        let diff = open(root).diff(Path::new("[id].tsx"), true).unwrap();
        assert!(diff.contains("selected"));
        assert!(!diff.contains("sibling"), "{diff}");
        let from = git(root, &["rev-parse", "HEAD"]);
        open(root).commit("changes").unwrap();
        let range = open(root)
            .diff_range(from.trim(), "HEAD", &[Path::new("[id].tsx")])
            .unwrap();
        assert!(!range.contains("sibling"), "{range}");
    }
}

#[test]
fn explicit_file_unstaging_uses_literal_pathspecs() {
    for batch in [false, true] {
        let temp = regression_repo(true);
        let root = temp.path();
        for name in ["[id].tsx", "i.tsx"] {
            std::fs::write(root.join(name), "new\n").unwrap();
        }
        open(root).add_all().unwrap();
        if batch {
            open(root)
                .restore_staged_paths(&[PathBuf::from("[id].tsx")])
                .unwrap();
        } else {
            open(root).restore_staged(Path::new("[id].tsx")).unwrap();
        }
        let staged = open(root).worktree_status().unwrap().staged;
        assert_eq!(
            staged.iter().map(|e| e.path.as_path()).collect::<Vec<_>>(),
            vec![Path::new("i.tsx")]
        );
    }
}

#[test]
fn unstaging_a_rename_restores_both_index_paths() {
    for batch in [false, true] {
        let temp = regression_repo(true);
        let root = temp.path();
        std::fs::write(root.join("old.txt"), "original\n").unwrap();
        open(root).add_all().unwrap();
        open(root).commit("base").unwrap();
        std::fs::create_dir(root.join("other")).unwrap();
        std::fs::rename(root.join("old.txt"), root.join("other/new.txt")).unwrap();
        open(root).add_all().unwrap();
        if batch {
            open(root)
                .restore_staged_paths(&[PathBuf::from("other/new.txt")])
                .unwrap();
        } else {
            open(root)
                .restore_staged(Path::new("other/new.txt"))
                .unwrap();
        }
        assert!(open(root).worktree_status().unwrap().staged.is_empty());
        assert_eq!(
            std::fs::read(root.join("other/new.txt")).unwrap(),
            b"original\n"
        );
        assert!(!root.join("old.txt").exists());
    }
}

#[test]
fn all_unstage_entry_points_preserve_files_before_the_first_commit() {
    for mode in 0..3 {
        let temp = regression_repo(false);
        let root = temp.path();
        std::fs::write(root.join("new.txt"), "staged\n").unwrap();
        open(root).add_all().unwrap();
        std::fs::write(root.join("new.txt"), "later worktree bytes\n").unwrap();
        match mode {
            0 => open(root).restore_staged(Path::new("new.txt")),
            1 => open(root).restore_staged_paths(&[PathBuf::from("new.txt")]),
            _ => open(root).restore_all_staged(),
        }
        .unwrap();
        assert!(open(root).worktree_status().unwrap().staged.is_empty());
        assert_eq!(
            std::fs::read(root.join("new.txt")).unwrap(),
            b"later worktree bytes\n"
        );
    }
}

#[test]
fn unstaging_an_empty_unborn_index_succeeds() {
    let temp = regression_repo(false);
    open(temp.path()).restore_all_staged().unwrap();
    assert!(
        open(temp.path())
            .worktree_status()
            .unwrap()
            .staged
            .is_empty()
    );
}

#[test]
fn environment_failure_classification_is_narrow() {
    use daruda_project::lane::git::{GitEnvironmentFailure, GitError};
    let license = "You have not agreed to the Xcode license agreements. Please run 'sudo xcodebuild -license' from within a Terminal window to review and agree to the Xcode and Apple SDKs license.";
    assert_eq!(
        GitError::NotFound.environment_failure(),
        Some(GitEnvironmentFailure::ExecutableMissing)
    );
    for ending in ["", "\n", "\r\n"] {
        assert_eq!(
            GitError::Exit {
                code: Some(69),
                stderr: format!("{license}{ending}")
            }
            .environment_failure(),
            Some(GitEnvironmentFailure::XcodeLicenseNotAccepted)
        );
    }
    for stderr in [
        "unrelated error".to_owned(),
        format!("{license}\nfatal: other failure"),
        format!("hook: {license}"),
    ] {
        assert_eq!(
            GitError::Exit {
                code: Some(69),
                stderr
            }
            .environment_failure(),
            None
        );
    }
    for error in [
        GitError::Utf8,
        GitError::Parse("bad".into()),
        GitError::Timeout(std::time::Duration::from_secs(1)),
        GitError::Spawn(std::io::ErrorKind::NotFound.into()),
    ] {
        assert_eq!(error.environment_failure(), None);
    }
}

#[test]
fn base_problem_preserves_environment_failure() {
    let problem = base::BaseProblem::from(GitError::NotFound);
    assert_eq!(
        problem.environment_failure(),
        Some(GitEnvironmentFailure::ExecutableMissing)
    );
    assert_eq!(
        base::BaseProblem::NoBaseConfigured.environment_failure(),
        None
    );
}

/// A handle from [`SystemGit`] is rooted where it was opened and reads the
/// worktree through the real CLI.
#[test]
fn system_git_delegates_to_the_git_cli() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("system_git");
    init(&dir);
    commit_initial(&dir);
    std::fs::write(dir.join("new.txt"), "x\n").unwrap();

    let git = system_git();
    assert!(git.available());
    let repo = git.open(&dir);
    assert_eq!(repo.path(), dir.as_path());
    let status = repo.worktree_status().unwrap();
    assert!(status.staged.is_empty());
    assert_eq!(status.unstaged.len(), 1);
    assert_eq!(status.unstaged[0].path, PathBuf::from("new.txt"));
    assert_eq!(repo.default_branch().as_deref(), Some("main"));
    assert_eq!(repo.status_scoped().unwrap(), "?? new.txt");
    teardown(&dir);
}

/// [`FakeGit`]'s plain-directory column, checked against real git on a
/// directory that is no repository.
#[test]
fn the_fakes_plain_directory_answers_match_real_git() {
    if !require_git() {
        return;
    }
    let dir = unique_tmpdir("plain_vs_fake");
    let real = system_git().open(&dir);
    let fake = FakeGit::new().open(&dir);
    for repo in [&real, &fake] {
        assert!(
            matches!(
                repo.worktree_status(),
                Err(GitError::Exit {
                    code: Some(128),
                    ..
                })
            ),
            "worktree_status in a plain directory"
        );
        assert_eq!(repo.default_branch(), None);
        assert!(
            matches!(
                repo.base_tips(Some("main"), None),
                Err(base::BaseProblem::Git(_))
            ),
            "base_tips in a plain directory"
        );
    }
    teardown(&dir);
}
