//! Public API tests for how daruda configures its `git` children.

use crate::fixture::{git, git_output, has_git, open};

/// The reason this module exists: a status read must leave `.git/index`
/// byte-identical. The plain run at the end is what proves the fixture
/// was actually stale — without it the first assert could pass on a repo
/// git had nothing to refresh in.
#[test]
fn a_status_read_does_not_rewrite_the_index() {
    if !has_git() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "daruda@test"]);
    git(root, &["config", "user.name", "daruda"]);
    std::fs::write(root.join("f.txt"), b"one\n").unwrap();
    git(root, &["add", "f.txt"]);
    git(root, &["commit", "-qm", "initial"]);

    // Re-write the same bytes under a later mtime so the index's stat
    // cache goes stale. The sleep buys a distinct timestamp: within one
    // filesystem tick git sees nothing to refresh and the test would pass
    // for the wrong reason.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(root.join("f.txt"), b"one\n").unwrap();

    let index = root.join(".git").join("index");
    let before = std::fs::read(&index).unwrap();

    // `status_scoped` is a bare `git status`; `worktree_status` also runs
    // `git diff HEAD --numstat`, which refreshes a stale index regardless.
    open(root).status_scoped().unwrap();
    assert_eq!(
        std::fs::read(&index).unwrap(),
        before,
        "a status read through daruda must not touch the index"
    );

    git_output(root, &["status", "--porcelain=v1"]);
    assert_ne!(
        std::fs::read(&index).unwrap(),
        before,
        "the fixture was not stale — the assert above proved nothing"
    );
}
