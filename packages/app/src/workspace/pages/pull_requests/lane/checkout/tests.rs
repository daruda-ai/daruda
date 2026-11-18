use std::path::Path;

use daruda_github::{CheckRollup, PrState, PrSummary};

use daruda_github::RepoId;

use super::*;
use crate::lane::git::{FakeGit, FakeRepo, Git as _, GitCall};

fn item(cross_repo: bool, author: Option<&str>, head_owner: Option<&str>) -> PrListItem {
    PrListItem {
        repo: RepoId::new("github.com", "o", "r"),
        pr: PrSummary {
            number: 9,
            url: "https://github.com/o/r/pull/9".into(),
            title: "t".into(),
            state: PrState::Open,
            head_branch: "fix/scroll".into(),
            cross_repo,
            checks: CheckRollup::None,
            failed: Vec::new(),
            review: None,
            updated_at: String::new(),
        },
        author: author.map(str::to_owned),
        head_owner: head_owner.map(str::to_owned),
        base_branch: "main".into(),
        comments: 0,
    }
}

const ROOT: &str = "/fake/repo";
const PUSH_URL: &str = "git@github.com:o/r.git";

/// `check_out` into `new_path`, with the stages it said.
fn run_into(
    repo: FakeRepo,
    cross_repo: bool,
    new_path: &Path,
) -> (
    Result<CheckedOut, CheckoutError>,
    Vec<GitCall>,
    Vec<CheckoutStage>,
) {
    let git = FakeGit::new().with(repo);
    let checkout = PrCheckout::of(&item(cross_repo, Some("me"), None), "up");
    let stages = std::cell::RefCell::new(Vec::new());
    let made = check_out(
        &*git.open(Path::new(ROOT)),
        &checkout,
        PUSH_URL,
        new_path,
        &|stage| stages.borrow_mut().push(stage),
    );
    (made, git.calls(), stages.into_inner())
}

fn left_at(repo: FakeRepo, path: &Path, branch: &str) -> FakeRepo {
    repo.worktrees(vec![
        crate::lane::git::GitWorktreeInfo {
            path: Path::new(ROOT).to_owned(),
            branch: Some("main".into()),
            head: None,
            bare: false,
        },
        crate::lane::git::GitWorktreeInfo {
            path: path.to_owned(),
            branch: Some(branch.into()),
            head: None,
            bare: false,
        },
    ])
}

#[test]
fn a_worktree_left_at_the_path_is_taken_only_when_it_is_this_ones() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    let checkout = PrCheckout::of(&item(false, Some("me"), None), "up");
    let branch = checkout.branch.clone();

    // Ours: fetched and measured, nothing made.
    let (made, calls, stages) = run_into(left_at(FakeRepo::at(ROOT), path, &branch), false, path);
    assert!(matches!(made, Ok(CheckedOut::Existing { .. })), "{made:?}");
    assert!(
        calls
            .iter()
            .all(|call| matches!(call, GitCall::FetchRef { .. })),
        "{calls:?}"
    );
    assert_eq!(stages, [CheckoutStage::Fetching]);

    // Another branch there: left alone, nothing asked of git.
    let (made, calls, stages) = run_into(
        left_at(FakeRepo::at(ROOT), path, "someone-elses"),
        false,
        path,
    );
    assert!(matches!(made, Err(CheckoutError::Occupied(_))), "{made:?}");
    assert!(
        calls.is_empty() && stages.is_empty(),
        "{calls:?} {stages:?}"
    );

    // A review branch whose mark names another pull request.
    let review = PrCheckout::of(&item(true, Some("me"), None), "up");
    let repo = left_at(FakeRepo::at(ROOT), path, &review.branch)
        .review_mark(review.branch.clone(), "github.com/o/r#999");
    let (made, _, _) = run_into(repo, true, path);
    assert!(matches!(made, Err(CheckoutError::Occupied(_))), "{made:?}");
}

#[test]
fn a_listed_worktree_whose_directory_is_gone_is_made_again() {
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().join("gone");
    let checkout = PrCheckout::of(&item(false, Some("me"), None), "up");
    let (made, calls, _) = run_into(
        left_at(FakeRepo::at(ROOT), &gone, &checkout.branch),
        false,
        &gone,
    );
    assert!(matches!(made, Ok(CheckedOut::AtHead)), "{made:?}");
    assert!(
        calls
            .iter()
            .any(|call| matches!(call, GitCall::AddLane { .. }))
    );
}

/// What `check_out` asked git to change, the reads left out.
fn run(repo: FakeRepo, cross_repo: bool) -> (Result<CheckedOut, CheckoutError>, Vec<GitCall>) {
    let git = FakeGit::new().with(repo);
    let checkout = PrCheckout::of(&item(cross_repo, Some("me"), None), "up");
    let stages = std::cell::RefCell::new(Vec::new());
    let made = check_out(
        &*git.open(Path::new(ROOT)),
        &checkout,
        PUSH_URL,
        Path::new("/fake/lane"),
        &|stage| stages.borrow_mut().push(stage),
    );
    // Every checkout that makes a worktree says so in order.
    if matches!(made, Ok(CheckedOut::AtHead | CheckedOut::KeptLocal)) {
        assert_eq!(
            *stages.borrow(),
            [CheckoutStage::Fetching, CheckoutStage::Creating]
        );
    }
    (made, git.calls())
}

fn fetch(cross_repo: bool) -> GitCall {
    let refspec = if cross_repo {
        "+refs/pull/9/head:refs/daruda/pr/9"
    } else {
        "+refs/heads/fix/scroll:refs/remotes/up/fix/scroll"
    };
    GitCall::FetchRef {
        repo: ROOT.into(),
        remote: PUSH_URL.into(),
        refspec: refspec.into(),
    }
}

fn moved(branch: &str, head: &str) -> GitCall {
    GitCall::MoveBranch {
        repo: ROOT.into(),
        branch: branch.into(),
        head: head.into(),
    }
}

fn added(branch: &str) -> GitCall {
    GitCall::AddLane {
        repo: ROOT.into(),
        new_path: "/fake/lane".into(),
        start: LaneStart::existing_branch(branch),
    }
}

const MARK: &str = "github.com/o/r#9";
const REVIEW_HEAD: &str = "refs/daruda/pr/9";
const SAME_HEAD: &str = "refs/remotes/up/fix/scroll";

#[test]
fn a_branch_is_brought_to_the_fetched_head_unless_it_has_commits_of_its_own() {
    let mark = GitCall::SetReviewMark {
        repo: ROOT.into(),
        branch: "pr-9".into(),
        mark: MARK.into(),
    };
    let at = FakeRepo::at(ROOT);
    // (label, repository, cross_repo, outcome, calls)
    let cases = vec![
        (
            "new review branch: made at the head and marked",
            at.clone(),
            true,
            CheckedOut::AtHead,
            vec![fetch(true), moved("pr-9", REVIEW_HEAD), mark, added("pr-9")],
        ),
        (
            "an old review branch is reset to the fresh head",
            at.clone()
                .local_branches(["pr-9"])
                .review_mark("pr-9", MARK),
            true,
            CheckedOut::AtHead,
            vec![fetch(true), moved("pr-9", REVIEW_HEAD), added("pr-9")],
        ),
        (
            "a review branch with commits of its own is kept",
            at.clone()
                .ahead_branches(["pr-9"])
                .review_mark("pr-9", MARK),
            true,
            CheckedOut::KeptLocal,
            vec![fetch(true), added("pr-9")],
        ),
        (
            "same repository, new: made at the head",
            at.clone(),
            false,
            CheckedOut::AtHead,
            vec![
                fetch(false),
                moved("fix/scroll", SAME_HEAD),
                GitCall::SetPushRemote {
                    repo: ROOT.into(),
                    branch: "fix/scroll".into(),
                    remote: "up".into(),
                },
                added("fix/scroll"),
            ],
        ),
        (
            "same repository, behind: fast-forwarded",
            at.clone().local_branches(["fix/scroll"]),
            false,
            CheckedOut::AtHead,
            vec![
                fetch(false),
                moved("fix/scroll", SAME_HEAD),
                added("fix/scroll"),
            ],
        ),
        (
            "same repository, with unpushed commits: kept",
            at.clone().ahead_branches(["fix/scroll"]),
            false,
            CheckedOut::KeptLocal,
            vec![fetch(false), added("fix/scroll")],
        ),
    ];
    for (label, repo, cross_repo, outcome, calls) in cases {
        let (made, logged) = run(repo, cross_repo);
        assert_eq!(made.ok(), Some(outcome), "{label}");
        assert_eq!(logged, calls, "{label}");
    }
}

#[test]
fn the_named_remote_is_fetched_only_when_it_fetches_the_pushed_repository() {
    // (label, repository, where the ref is fetched from)
    let cases = [
        (
            "same repository over another transport: the remote",
            FakeRepo::at(ROOT).remote_url("up", "https://github.com/o/r.git"),
            "up",
        ),
        (
            "fetches another repository: the push URL",
            FakeRepo::at(ROOT).remote_url("up", "https://github.com/elsewhere/r.git"),
            PUSH_URL,
        ),
        ("unknown remote: the push URL", FakeRepo::at(ROOT), PUSH_URL),
    ];
    for (label, repo, source) in cases {
        let (made, calls) = run(repo, true);
        assert!(made.is_ok(), "{label}: {made:?}");
        assert!(
            matches!(calls.first(), Some(GitCall::FetchRef { remote, .. }) if remote == source),
            "{label}: {calls:?}"
        );
    }
}

#[test]
fn a_branch_of_the_review_branchs_name_made_otherwise_is_left_alone() {
    // (label, repository)
    let cases = [
        ("unmarked", FakeRepo::at(ROOT).local_branches(["pr-9"])),
        (
            "marked for another pull request",
            FakeRepo::at(ROOT)
                .local_branches(["pr-9"])
                .review_mark("pr-9", "github.com/fork/r#9"),
        ),
    ];
    for (label, repo) in cases {
        let (made, calls) = run(repo, true);
        assert!(
            matches!(made, Err(CheckoutError::Foreign(ref b)) if b == "pr-9"),
            "{label}: {made:?}"
        );
        assert!(
            calls.is_empty(),
            "{label}: nothing fetched or moved: {calls:?}"
        );
    }
}

#[test]
fn only_a_review_worktree_is_described_by_its_forks_owner() {
    assert_eq!(description(&item(false, Some("me"), Some("o"))), None);
    // (label, author, head owner, the `owner:branch` described)
    let cases = [
        (
            "the fork's owner, not the author",
            Some("alice"),
            Some("acme"),
            "acme:fix/scroll",
        ),
        (
            "a deleted fork falls back to the author",
            Some("alice"),
            None,
            "alice:fix/scroll",
        ),
    ];
    for (label, author, head_owner, expected) in cases {
        let described = description(&item(true, author, head_owner)).unwrap();
        assert!(described.contains("#9"), "{label}: {described}");
        assert!(described.contains(expected), "{label}: {described}");
    }
}
