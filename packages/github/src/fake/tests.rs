use super::*;
use crate::{CheckRollup, MergeMethod, Mergeability, Permission, PrState, ViewerCan};

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
    let answer = gh.prs_for_branches(&repo(), &branches, &[2, 8]).unwrap();
    assert_eq!(answer.default_branch.as_deref(), Some("main"));
    let numbers: Vec<u64> = answer.by_branch["a"].iter().map(|p| p.number).collect();
    assert_eq!(numbers, [3, 1], "most recently updated first");
    assert!(answer.by_branch["c"].is_empty());
    let named = answer.by_number[&2].as_ref().expect("PR 2");
    assert_eq!(named.head_branch, "b", "a number names its PR");
    assert_eq!(
        answer.by_number[&8], None,
        "one the repo lacks answers none"
    );
    assert_eq!(gh.count(GhOp::PrsForBranches), 1);
    assert_eq!(
        gh.calls(),
        vec![GhCall::PrsForBranches {
            repo: repo(),
            branches,
            numbers: vec![2, 8],
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
    let found = gh
        .prs_for_branches(&repo(), &["feat".to_owned()], &[])
        .unwrap();
    let pr = &found.by_branch["feat"][0];
    assert_eq!((pr.number, pr.state), (5, PrState::Draft));
}

#[test]
fn an_injected_failure_lasts_until_cleared() {
    let gh = FakeGh::new();
    gh.set_auth("ghe.corp", GhAuth::LoggedOut);
    assert_eq!(gh.auth("ghe.corp").unwrap().auth, GhAuth::LoggedOut);
    assert_eq!(gh.auth("github.com").unwrap().auth, GhAuth::Ready);
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
fn a_scripted_detail_answers_its_repository_and_number_only() {
    let detail = PrDetail {
        summary: pr(7, "a"),
        body: String::new(),
        head_sha: "abc".into(),
        base_branch: "main".into(),
        author: None,
        mergeability: Mergeability::Ready,
        methods: Vec::new(),
        default_method: None,
        reviewers: Vec::new(),
        checks: Vec::new(),
        checks_truncated: false,
        open_threads: 0,
        open_threads_more: false,
        viewer_can: ViewerCan::all().with(Permission::Update, false),
        auto_merge: None,
        queue: None,
        node_id: "PR_7".into(),
        changed_files: None,
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

fn detail(number: u64, state: PrState, head_sha: &str) -> PrDetail {
    PrDetail {
        summary: PrSummary {
            state,
            ..pr(number, "a")
        },
        body: String::new(),
        head_sha: head_sha.into(),
        base_branch: "main".into(),
        author: None,
        mergeability: Mergeability::Ready,
        methods: Vec::new(),
        default_method: None,
        reviewers: Vec::new(),
        checks: Vec::new(),
        checks_truncated: false,
        open_threads: 0,
        open_threads_more: false,
        viewer_can: ViewerCan::all().with(Permission::Update, false),
        auto_merge: None,
        queue: None,
        node_id: format!("PR_{number}"),
        changed_files: None,
    }
}

#[test]
fn a_mutation_moves_the_scripted_pr_to_its_new_state() {
    type Mutate = fn(&FakeGh, &RepoId, u64) -> Result<(), GhError>;
    let cases: [(&str, PrState, Mutate, PrState); 4] = [
        (
            "merge",
            PrState::Open,
            |gh, repo, n| gh.merge_pr(repo, n, MergeMethod::Squash, "abc"),
            PrState::Merged,
        ),
        ("ready", PrState::Draft, FakeGh::set_ready, PrState::Open),
        ("close", PrState::Open, FakeGh::close_pr, PrState::Closed),
        ("reopen", PrState::Closed, FakeGh::reopen_pr, PrState::Open),
    ];
    for (label, before, mutate, after) in cases {
        let gh = FakeGh::new();
        gh.set_prs(
            &repo(),
            vec![PrSummary {
                state: before,
                ..pr(7, "a")
            }],
        );
        gh.set_pr_detail(&repo(), 7, detail(7, before, "abc"));
        mutate(&gh, &repo(), 7).unwrap();
        let listed = gh
            .prs_for_branches(&repo(), &["a".to_owned()], &[])
            .unwrap();
        assert_eq!(listed.by_branch["a"][0].state, after, "{label}: listed");
        let shown = gh.pr_detail(&repo(), 7).unwrap();
        assert_eq!(shown.summary.state, after, "{label}: detail");
    }
}

#[test]
fn a_merge_is_refused_unless_pinned_to_the_scripted_head() {
    let gh = FakeGh::new();
    gh.set_pr_detail(&repo(), 7, detail(7, PrState::Open, "abc"));
    let cases = [
        ("moved head", "def", "head moved"),
        ("not a commit", "", "bad argument"),
    ];
    for (label, sha, expected) in cases {
        let answer = gh.merge_pr(&repo(), 7, MergeMethod::Rebase, sha);
        let got = match answer {
            Err(GhError::HeadMoved) => "head moved",
            Err(GhError::BadArgument(_)) => "bad argument",
            _ => "other",
        };
        assert_eq!(got, expected, "{label}: {answer:?}");
    }
    assert_eq!(
        gh.pr_detail(&repo(), 7).unwrap().summary.state,
        PrState::Open
    );
    assert_eq!(
        gh.calls()[0],
        GhCall::MergePr {
            repo: repo(),
            number: 7,
            method: MergeMethod::Rebase,
            head_sha: "def".into(),
        }
    );
}

#[test]
fn rerun_is_logged_and_a_scripted_failure_answers_each_mutation() {
    let gh = FakeGh::new();
    gh.rerun_failed(&repo(), 991).unwrap();
    assert_eq!(
        gh.calls(),
        vec![GhCall::RerunFailed {
            repo: repo(),
            run: 991
        }]
    );
    let ops = [
        GhOp::MergePr,
        GhOp::SetReady,
        GhOp::ClosePr,
        GhOp::ReopenPr,
        GhOp::RerunFailed,
    ];
    for op in ops {
        gh.fail(op, GhFailure::HeadMoved);
    }
    let answers = [
        ("merge", gh.merge_pr(&repo(), 1, MergeMethod::Merge, "abc")),
        ("ready", gh.set_ready(&repo(), 1)),
        ("close", gh.close_pr(&repo(), 1)),
        ("reopen", gh.reopen_pr(&repo(), 1)),
        ("rerun", gh.rerun_failed(&repo(), 1)),
    ];
    for (label, answer) in answers {
        assert!(matches!(answer, Err(GhError::HeadMoved)), "{label}");
    }
    for op in ops {
        assert_eq!(gh.count(op), 1 + usize::from(op == GhOp::RerunFailed));
    }
}

#[test]
fn auto_merge_follows_the_scripted_head() {
    let gh = FakeGh::new();
    let scripted = detail(7, PrState::Open, "abc");
    gh.set_pr_detail(&repo(), 7, scripted);
    assert!(matches!(
        gh.enable_auto_merge(&repo(), "PR_7", MergeMethod::Squash, "def", false),
        Err(GhError::Exit { .. })
    ));
    assert_eq!(gh.pr_detail(&repo(), 7).unwrap().auto_merge, None);
    gh.enable_auto_merge(&repo(), "PR_7", MergeMethod::Squash, "abc", false)
        .unwrap();
    let on = gh.pr_detail(&repo(), 7).unwrap().auto_merge.unwrap();
    assert_eq!(on.method, Some(MergeMethod::Squash));
    gh.disable_auto_merge(&repo(), "PR_7").unwrap();
    assert_eq!(gh.pr_detail(&repo(), 7).unwrap().auto_merge, None);
    assert_eq!(gh.count(GhOp::EnableAutoMerge), 2);
    assert_eq!(gh.count(GhOp::DisableAutoMerge), 1);
}

#[test]
fn a_merge_behind_a_queue_enqueues_and_merges_nothing() {
    let gh = FakeGh::new();
    gh.set_pr_detail(&repo(), 7, detail(7, PrState::Open, "abc"));
    gh.set_merge_queued(&repo(), 7);
    gh.merge_pr(&repo(), 7, MergeMethod::Squash, "abc").unwrap();
    let read = gh.pr_detail(&repo(), 7).unwrap();
    assert_eq!(read.summary.state, PrState::Open, "not merged");
    assert_eq!(
        read.queue.map(|entry| entry.state),
        Some(crate::QueueState::Queued)
    );
}

#[test]
fn review_threads_belong_to_their_repository() {
    let gh = FakeGh::new();
    let other = RepoId::new("github.com", "o", "other");
    let threads = ReviewThreads {
        threads: vec![crate::ReviewThread {
            id: "t1".into(),
            ..crate::ReviewThread::default()
        }],
        ..ReviewThreads::default()
    };
    gh.set_review_threads(&repo(), 7, threads);
    assert_eq!(gh.review_threads(&repo(), 7).unwrap().threads.len(), 1);
    assert!(
        gh.review_threads(&other, 7).unwrap().threads.is_empty(),
        "the same number in another repository"
    );
}
