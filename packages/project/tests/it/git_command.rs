//! Public API tests for how daruda configures its `git` children.

use std::path::{Path, PathBuf};

use crate::fixture::{git, git_output, has_git, open};

/// A committed repo whose index stat cache is stale for `f.txt`: the file
/// holds the bytes the index recorded, under a later mtime.
fn stale_repo(root: &Path) {
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "daruda@test"]);
    git(root, &["config", "user.name", "daruda"]);
    std::fs::write(root.join("f.txt"), b"one\n").unwrap();
    git(root, &["add", "f.txt"]);
    git(root, &["commit", "-qm", "initial"]);
    restale(root);
}

/// Re-write the same bytes under a later mtime. The sleep buys a distinct
/// timestamp: within one filesystem tick git sees nothing to refresh and
/// the test would pass for the wrong reason.
fn restale(worktree: &Path) {
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(worktree.join("f.txt"), b"one\n").unwrap();
}

fn index_of(worktree: &Path) -> PathBuf {
    let git_dir = git(worktree, &["rev-parse", "--absolute-git-dir"]);
    PathBuf::from(git_dir.trim()).join("index")
}

/// Every lane read daruda repeats on a watcher event must leave the index
/// byte-identical: the git-dir watcher reports an `index` write as a
/// working-tree change, so a read that writes re-triggers itself. The
/// plain `git status` at the end proves the fixture was actually stale —
/// without it the asserts could pass on a repo git had nothing to refresh in.
fn assert_reads_leave_index_alone(worktree: &Path) {
    let index = index_of(worktree);
    let before = std::fs::read(&index).unwrap();

    let repo = open(worktree);
    repo.status_scoped().unwrap();
    assert!(
        std::fs::read(&index).unwrap() == before,
        "a scoped status read must not touch the index"
    );
    repo.worktree_status().unwrap();
    assert!(
        std::fs::read(&index).unwrap() == before,
        "a worktree status read, diffstat included, must not touch the index"
    );

    git_output(worktree, &["status", "--porcelain=v1"]);
    assert!(
        std::fs::read(&index).unwrap() != before,
        "the fixture was not stale — the asserts above proved nothing"
    );
}

#[test]
fn a_status_read_does_not_rewrite_the_index() {
    if !has_git() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    stale_repo(temp.path());
    assert_reads_leave_index_alone(temp.path());
}

/// A linked lane keeps its index under the common dir's `worktrees/`, and
/// its own watcher sees that file the same way.
#[test]
fn a_linked_lane_status_read_does_not_rewrite_its_index() {
    if !has_git() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main");
    let linked = temp.path().join("linked");
    std::fs::create_dir(&root).unwrap();
    stale_repo(&root);
    git(&root, &["branch", "side"]);
    git(
        &root,
        &["worktree", "add", "-q", linked.to_str().unwrap(), "side"],
    );
    restale(&linked);
    assert_reads_leave_index_alone(&linked);
}
