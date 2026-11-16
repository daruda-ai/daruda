//! What commit, push and fetch do to a real repository: the git side of the
//! app's Git panel tests, which drive the same ops against a scripted fake.

use std::path::{Path, PathBuf};

use crate::fixture::{git, git_output, has_git, init, open};
use daruda_project::lane::git::{GitError, LaneStart, LocalBranch};

/// A repository with one commit pushed to a bare `origin.git` beside it,
/// tracking `origin/main`. Returns `(origin, work)`.
fn pushed_repo(temp: &Path) -> (PathBuf, PathBuf) {
    let origin = temp.join("origin.git");
    let work = temp.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(temp, &["init", "-q", "--bare", "-b", "main", "origin.git"]);
    git(&work, &["init", "-q", "-b", "main"]);
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
    open(&work).add(Path::new("f.txt")).unwrap();

    open(&work).commit("mine").unwrap();
    assert_eq!(open(&work).head_message().unwrap(), "mine");
    assert!(
        open(&work).worktree_status().unwrap().staged.is_empty(),
        "the staged change is committed"
    );
    assert_eq!(
        git(&origin, &["log", "-1", "--format=%s", "main"]),
        "initial"
    );

    open(&work).push().unwrap();
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
    open(&clone)
        .add_lane(&linked, LaneStart::new_branch("side", None))
        .unwrap();
    git(&clone, &["branch", "--set-upstream-to=origin/main", "side"]);
    // A commit the clone cannot see until it fetches.
    std::fs::write(seed.join("f.txt"), "two\n").unwrap();
    git(&seed, &["commit", "-qam", "second"]);
    git(&seed, &["push", "-q", "origin", "main"]);

    for lane in [&clone, &linked] {
        let tracking = open(lane).tracking().unwrap();
        assert_eq!(tracking.upstream.as_deref(), Some("origin/main"));
        assert_eq!(tracking.behind, 0, "{lane:?} before the fetch");
    }

    open(&clone).fetch().unwrap();

    let common = open(&clone).dirs().unwrap().common_dir;
    assert_eq!(open(&linked).dirs().unwrap().common_dir, common);
    let moved = std::fs::read_to_string(common.join("refs/remotes/origin/main")).unwrap();
    assert_eq!(moved.trim(), git(&origin, &["rev-parse", "main"]));
    for lane in [&clone, &linked] {
        assert_eq!(open(lane).tracking().unwrap().behind, 1, "{lane:?} after");
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
    init(&root);
    git(&root, &["commit", "-q", "--allow-empty", "-m", "initial"]);
    let linked = temp.path().join("side-wt");
    open(&root)
        .add_lane(&linked, LaneStart::new_branch("side", None))
        .unwrap();
    std::fs::write(linked.join("side.txt"), "staged\n").unwrap();
    open(&linked).add(Path::new("side.txt")).unwrap();
    assert_eq!(open(&linked).worktree_status().unwrap().staged.len(), 1);

    let index = open(&linked).dirs().unwrap().git_dir.join("index");
    let before = std::fs::read(&index).unwrap();
    open(&linked).commit("outside daruda").unwrap();

    assert_ne!(std::fs::read(&index).unwrap(), before, "the index moved");
    assert!(open(&linked).worktree_status().unwrap().staged.is_empty());
}

/// What opening a worktree from a pull request asks of git: one ref fetched
/// to a name of the caller's choosing, a branch started from it, and an
/// existing branch checked out rather than detached at its commit.
#[test]
fn a_fetched_ref_starts_a_lane_and_an_existing_branch_is_checked_out() {
    if !has_git() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let (origin, seed) = pushed_repo(temp.path());
    // A pull request's head, as GitHub keeps it, outside `refs/heads`.
    std::fs::write(seed.join("f.txt"), "pr\n").unwrap();
    git(&seed, &["commit", "-qam", "pr"]);
    git(&seed, &["push", "-q", "origin", "HEAD:refs/pull/7/head"]);
    let head = git(&seed, &["rev-parse", "HEAD"]);
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
    let repo = open(&clone);

    repo.fetch_ref("origin", "+refs/pull/7/head:refs/daruda/pr/7")
        .unwrap();
    assert!(!repo.has_local_branch("pr-7").unwrap());
    let review = temp.path().join("review-wt");
    repo.add_lane(
        &review,
        LaneStart::new_branch("pr-7", Some("refs/daruda/pr/7")),
    )
    .unwrap();
    assert_eq!(git(&review, &["rev-parse", "HEAD"]), head);
    assert!(repo.has_local_branch("pr-7").unwrap());

    git(&clone, &["branch", "kept", "main"]);
    let kept = temp.path().join("kept-wt");
    repo.add_lane(&kept, LaneStart::existing_branch("kept"))
        .unwrap();
    assert_eq!(
        git(&kept, &["symbolic-ref", "--short", "HEAD"]),
        "kept",
        "on the branch, not detached"
    );
}

/// Git itself would accept each of these and do something else: track a
/// remote branch under a new local name, or detach HEAD at a commit.
#[test]
fn only_a_local_branch_is_checked_out_as_an_existing_one() {
    if !has_git() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let (origin, seed) = pushed_repo(temp.path());
    git(&seed, &["push", "-q", "origin", "HEAD:refs/heads/theirs"]);
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
    let head = git(&clone, &["rev-parse", "HEAD"]);
    git(
        &clone,
        &["rev-parse", "--verify", "refs/remotes/origin/theirs"],
    );
    let repo = open(&clone);

    // (label, what was asked for as an existing branch)
    let cases = [
        ("known only as a remote's branch", "theirs"),
        ("a remote-tracking name", "origin/theirs"),
        ("a commit", head.as_str()),
        ("a full ref name", "refs/heads/main"),
        ("no such name", "absent"),
    ];
    for (index, (label, name)) in cases.into_iter().enumerate() {
        let lane = temp.path().join(format!("lane-{index}"));
        match repo.add_lane(&lane, LaneStart::existing_branch(name)) {
            Err(GitError::NoLocalBranch(refused)) => assert_eq!(refused, name, "{label}"),
            other => panic!("{label}: expected a refusal, got {other:?}"),
        }
        assert!(!lane.exists(), "{label}: no worktree was made");
    }
    assert!(
        !repo.has_local_branch("theirs").unwrap(),
        "no tracking branch was created"
    );
}

/// A local branch is measured against a fetched head, moved only when that
/// loses nothing of its own, and never while a worktree has it checked out.
#[test]
fn a_local_branch_moves_to_a_fetched_head_that_contains_it() {
    if !has_git() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let (_origin, work) = pushed_repo(temp.path());
    let repo = open(&work);
    git(&work, &["branch", "old", "main"]);
    git(&work, &["checkout", "-qb", "next"]);
    std::fs::write(work.join("f.txt"), "next\n").unwrap();
    git(&work, &["commit", "-qam", "next"]);
    git(&work, &["update-ref", "refs/daruda/pr/1", "next"]);
    git(&work, &["checkout", "-q", "main"]);

    // (label, branch, head, expected)
    let cases = [
        ("absent", "nope", "refs/daruda/pr/1", LocalBranch::Missing),
        (
            "behind the head",
            "old",
            "refs/daruda/pr/1",
            LocalBranch::Contained,
        ),
        (
            "at the head",
            "next",
            "refs/daruda/pr/1",
            LocalBranch::Contained,
        ),
        ("past the head", "next", "main", LocalBranch::Ahead),
        (
            "a head that does not resolve",
            "old",
            "refs/daruda/pr/2",
            LocalBranch::Ahead,
        ),
    ];
    for (label, branch, head, expected) in cases {
        assert_eq!(
            repo.local_branch(branch, head).unwrap(),
            expected,
            "{label}"
        );
    }

    let tip = git(&work, &["rev-parse", "refs/daruda/pr/1"]);
    repo.move_branch("old", "refs/daruda/pr/1").unwrap();
    assert_eq!(git(&work, &["rev-parse", "refs/heads/old"]), tip);
    repo.move_branch("made", "refs/daruda/pr/1").unwrap();
    assert_eq!(git(&work, &["rev-parse", "refs/heads/made"]), tip);
    assert!(
        git_output(&work, &["config", "--get", "branch.made.remote"])
            .stdout
            .is_empty(),
        "a branch made from a non-remote ref has no upstream"
    );

    // `main` tracks `origin/main`; moving it keeps that upstream.
    let lane = temp.path().join("lane");
    repo.add_lane(&lane, LaneStart::existing_branch("old"))
        .unwrap();
    assert!(
        repo.move_branch("old", "main").is_err(),
        "a branch a worktree has checked out is not moved under it"
    );
    git(&work, &["checkout", "-q", "next"]);
    repo.move_branch("main", "refs/daruda/pr/1").unwrap();
    assert_eq!(
        git(&work, &["config", "--get", "branch.main.remote"]),
        "origin"
    );

    for (label, branch, head) in [("branch", "-x", "main"), ("head", "old", "--all")] {
        assert!(
            matches!(
                repo.move_branch(branch, head),
                Err(GitError::OptionLike { .. })
            ),
            "{label}"
        );
        assert!(
            matches!(
                repo.local_branch(branch, head),
                Err(GitError::OptionLike { .. })
            ),
            "{label}"
        );
    }
}

/// The review mark lives in the branch's config: the push target reports
/// it, and deleting the branch takes it along.
#[test]
fn a_review_mark_follows_its_branch() {
    if !has_git() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let (_origin, work) = pushed_repo(temp.path());
    let repo = open(&work);
    git(&work, &["branch", "pr-7", "main"]);
    assert_eq!(repo.review_mark("pr-7").unwrap(), None);

    repo.set_review_mark("pr-7", "github.com/o/r#7").unwrap();
    assert_eq!(
        repo.review_mark("pr-7").unwrap().as_deref(),
        Some("github.com/o/r#7")
    );
    let lane = temp.path().join("review");
    repo.add_lane(&lane, LaneStart::existing_branch("pr-7"))
        .unwrap();
    let target = open(&lane).push_target().unwrap().expect("origin");
    assert_eq!(target.review.as_deref(), Some("github.com/o/r#7"));
    assert_eq!(
        repo.push_target().unwrap().unwrap().review,
        None,
        "main has none"
    );

    repo.remove_lane(&lane, false).unwrap();
    repo.delete_branch("pr-7").unwrap();
    git(&work, &["branch", "pr-7", "main"]);
    assert_eq!(
        repo.review_mark("pr-7").unwrap(),
        None,
        "a new branch of the same name is not the old review branch"
    );
    assert!(matches!(
        repo.set_review_mark("pr-7", "-x"),
        Err(GitError::OptionLike { .. })
    ));
}
