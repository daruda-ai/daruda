//! What commit, push and fetch do to a real repository: the git side of the
//! app's Git panel tests, which drive the same ops against a scripted fake.

use std::path::{Path, PathBuf};

use daruda_project::lane::git::*;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_command(dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?} in {dir:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn identify(repo: &Path) {
    git(repo, &["config", "user.email", "daruda@test"]);
    git(repo, &["config", "user.name", "daruda"]);
}

/// A repository with one commit pushed to a bare `origin.git` beside it,
/// tracking `origin/main`. Returns `(origin, work)`.
fn pushed_repo(temp: &Path) -> (PathBuf, PathBuf) {
    let origin = temp.join("origin.git");
    let work = temp.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(temp, &["init", "-q", "--bare", "-b", "main", "origin.git"]);
    git(&work, &["init", "-q", "-b", "main"]);
    identify(&work);
    std::fs::write(work.join("f.txt"), "one\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-qm", "initial"]);
    git(
        &work,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&work, &["push", "-q", "-u", "origin", "main"]);
    (origin, work)
}

#[test]
fn a_commit_lands_its_message_and_a_push_reaches_the_remote() {
    if !has_git() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let (origin, work) = pushed_repo(temp.path());
    std::fs::write(work.join("f.txt"), "two\n").unwrap();
    git_add(&work, Path::new("f.txt")).unwrap();

    git_commit(&work, "mine").unwrap();
    assert_eq!(git_head_message(&work).unwrap(), "mine");
    assert!(
        git_worktree_status(&work).unwrap().staged.is_empty(),
        "the staged change is committed"
    );
    assert_eq!(
        git(&origin, &["log", "-1", "--format=%s", "main"]),
        "initial"
    );

    git_push(&work).unwrap();
    assert_eq!(git(&origin, &["log", "-1", "--format=%s", "main"]), "mine");
}

/// A fetch moves one remote-tracking ref under the shared common dir — the
/// directory the app's watcher fans out from — and every lane tracking that
/// ref then reads itself behind.
#[test]
fn a_fetch_moves_the_shared_remote_ref_and_every_lanes_tracking() {
    if !has_git() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let (origin, seed) = pushed_repo(temp.path());
    let clone = temp.path().join("clone");
    git(
        temp.path(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    let linked = temp.path().join("side-wt");
    add_lane(&clone, &linked, Some("side"), None).unwrap();
    git(&clone, &["branch", "--set-upstream-to=origin/main", "side"]);
    // A commit the clone cannot see until it fetches.
    std::fs::write(seed.join("f.txt"), "two\n").unwrap();
    git(&seed, &["commit", "-qam", "second"]);
    git(&seed, &["push", "-q", "origin", "main"]);

    for lane in [&clone, &linked] {
        let tracking = git_tracking(lane).unwrap();
        assert_eq!(tracking.upstream.as_deref(), Some("origin/main"));
        assert_eq!(tracking.behind, 0, "{lane:?} before the fetch");
    }

    git_fetch(&clone).unwrap();

    let common = git_dirs(&clone).unwrap().common_dir;
    assert_eq!(git_dirs(&linked).unwrap().common_dir, common);
    let moved = std::fs::read_to_string(common.join("refs/remotes/origin/main")).unwrap();
    assert_eq!(moved.trim(), git(&origin, &["rev-parse", "main"]));
    for lane in [&clone, &linked] {
        assert_eq!(git_tracking(lane).unwrap().behind, 1, "{lane:?} after");
    }
}

/// A commit in a linked lane rewrites the `index` in that lane's own git
/// dir, which is what the app's per-lane watcher keys a status refresh on.
#[test]
fn a_commit_in_a_linked_lane_rewrites_its_own_index() {
    if !has_git() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("work");
    std::fs::create_dir_all(&root).unwrap();
    init(&root).unwrap();
    identify(&root);
    git(&root, &["commit", "-q", "--allow-empty", "-m", "initial"]);
    let linked = temp.path().join("side-wt");
    add_lane(&root, &linked, Some("side"), None).unwrap();
    std::fs::write(linked.join("side.txt"), "staged\n").unwrap();
    git_add(&linked, Path::new("side.txt")).unwrap();
    assert_eq!(git_worktree_status(&linked).unwrap().staged.len(), 1);

    let index = git_dirs(&linked).unwrap().git_dir.join("index");
    let before = std::fs::read(&index).unwrap();
    git_commit(&linked, "outside daruda").unwrap();

    assert_ne!(std::fs::read(&index).unwrap(), before, "the index moved");
    assert!(git_worktree_status(&linked).unwrap().staged.is_empty());
}
