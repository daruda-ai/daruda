use super::*;
use crate::{CheckRollup, PrState};

fn repo() -> RepoId {
    RepoId {
        host: "github.com".into(),
        owner: "o".into(),
        name: "r".into(),
    }
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
