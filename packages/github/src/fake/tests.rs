use super::*;
use crate::{CheckRollup, Mergeability, PrSearch, PrState};

fn repo() -> RepoId {
    RepoId::new("github.com", "o", "r")
}

fn pr(number: u64, head: &str) -> PrSummary {
    PrSummary {
        number,
        url: String::new(),
        title: String::new(),
        state: PrState::Open,
        head_branch: head.into(),
        cross_repo: false,
        checks: CheckRollup::None,
        failed: Vec::new(),
        review: None,
        updated_at: format!("2026-01-0{number}"),
    }
}

#[test]
fn scripted_prs_answer_per_branch_and_calls_are_logged() {
    let gh = FakeGh::new();
    gh.set_prs(&repo(), vec![pr(1, "a"), pr(2, "b"), pr(3, "a")]);
    let branches = vec!["a".to_owned(), "c".to_owned()];
    let answer = gh.prs_for_branches(&repo(), &branches).unwrap();
    assert_eq!(answer.default_branch.as_deref(), Some("main"));
    let numbers: Vec<u64> = answer.by_branch["a"].iter().map(|p| p.number).collect();
    assert_eq!(numbers, [3, 1], "most recently updated first");
    assert!(answer.by_branch["c"].is_empty());
    assert_eq!(gh.count(GhOp::PrsForBranches), 1);
    assert_eq!(
        gh.calls(),
        vec![GhCall::PrsForBranches {
            repo: repo(),
            branches
        }]
    );
}

#[test]
fn a_created_pr_is_found_by_the_next_lookup() {
    let gh = FakeGh::new();
    gh.set_prs(&repo(), vec![pr(4, "old")]);
    let new = NewPr {
        head: "feat".into(),
        base: None,
        title: "Add it".into(),
        body: String::new(),
        draft: true,
        worktree: "/w".into(),
    };
    let url = gh.create_pr(&repo(), &new).unwrap();
    assert_eq!(url, "https://github.com/o/r/pull/5");
    let found = gh.prs_for_branches(&repo(), &["feat".to_owned()]).unwrap();
    let pr = &found.by_branch["feat"][0];
    assert_eq!((pr.number, pr.state), (5, PrState::Draft));
}

#[test]
fn an_injected_failure_lasts_until_cleared() {
    let gh = FakeGh::new();
    gh.set_auth("ghe.corp", GhAuth::LoggedOut);
    assert_eq!(gh.auth("ghe.corp").unwrap(), GhAuth::LoggedOut);
    assert_eq!(gh.auth("github.com").unwrap(), GhAuth::Ready);
    gh.fail(
        GhOp::Auth,
        GhFailure::Exit {
            code: Some(4),
            stderr: String::new(),
        },
    );
    assert!(gh.auth("github.com").is_err());
    gh.clear_failure(GhOp::Auth);
    assert!(gh.auth("github.com").is_ok());
    assert_eq!(gh.count(GhOp::Auth), 4);
}

#[test]
fn a_scripted_list_answers_only_the_searched_repositories() {
    let other = RepoId::new("github.com", "o", "other");
    let item = |repo: &RepoId, number| PrListItem {
        repo: repo.clone(),
        pr: pr(number, "a"),
        author: Some("me".into()),
        base_branch: "main".into(),
        comments: 0,
    };
    let gh = FakeGh::new();
    gh.set_pr_list(
        PrFilter::Mine,
        vec![item(&repo(), 1), item(&other, 2), item(&repo(), 3)],
    );
    gh.set_search_truncated(PrFilter::Mine, true);
    let found = gh
        .search_prs("github.com", &[repo()], PrFilter::Mine)
        .unwrap();
    assert_eq!(
        found,
        PrSearch {
            items: vec![item(&repo(), 3), item(&repo(), 1)],
            truncated: true,
        },
        "own repositories, most recently updated first"
    );
    let unscripted = gh.search_prs("github.com", &[repo()], PrFilter::AllOpen);
    assert_eq!(unscripted.unwrap(), PrSearch::default());
    assert_eq!(gh.count(GhOp::SearchPrs), 2);
    assert_eq!(
        gh.calls()[0],
        GhCall::SearchPrs {
            host: "github.com".into(),
            repos: vec![repo()],
            filter: PrFilter::Mine,
        }
    );
}

#[test]
fn a_scripted_detail_answers_its_repository_and_number_only() {
    let detail = PrDetail {
        summary: pr(7, "a"),
        body: String::new(),
        head_sha: "abc".into(),
        base_branch: "main".into(),
        author: None,
        mergeability: Mergeability::Ready,
        methods: Vec::new(),
        reviewers: Vec::new(),
        checks: Vec::new(),
        open_threads: 0,
    };
    let other = RepoId::new("github.com", "o", "other");
    let gh = FakeGh::new();
    gh.set_pr_detail(&repo(), 7, detail.clone());
    assert_eq!(gh.pr_detail(&repo(), 7).unwrap(), detail);
    let cases = [("other number", repo(), 8), ("other repository", other, 7)];
    for (label, repo, number) in cases {
        let answer = gh.pr_detail(&repo, number);
        assert!(
            matches!(answer, Err(GhError::Exit { code: Some(1), .. })),
            "{label}: {answer:?}"
        );
    }
    assert_eq!(gh.count(GhOp::PrDetail), 3);
    assert_eq!(
        gh.calls()[0],
        GhCall::PrDetail {
            repo: repo(),
            number: 7
        }
    );
}
