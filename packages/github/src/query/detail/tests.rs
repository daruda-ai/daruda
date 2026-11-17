//! `pr_detail_15003.json` is a real `gh api graphql` answer to
//! `pr_detail_query` for `cli/cli` #15003 (gh 2.74.2, 2026-11-13), its body
//! trimmed: a fork PR blocked on a review, with a request the caller cannot
//! see (`requestedReviewer: null`) and five jobs run twice.
//! `viewerDefaultMergeMethod` was added to the query afterwards; its value
//! is GitHub's answer for `cli/cli` to that field alone, the same day.
//! `changedFiles` was added likewise on 2026-11-17: GitHub answered 1.
//! `viewerCanClose` and `viewerCanReopen` likewise on 2026-11-18, read
//! from the PR as it was then (closed) and spliced into this open snapshot:
//! both false. The auto-merge fields likewise, the same day: no auto-merge,
//! no queue entry, neither permission.

use serde_json::{Value, json};

use super::{mergeability, parse_pr_detail, pr_detail_query, reviewer_state};
use crate::{
    BlockReason, CheckRollup, CheckStatus, GhError, MergeMethod, Mergeability, ReviewerState,
};

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn a_real_detail_answer_parses() {
    let detail = parse_pr_detail(&fixture("pr_detail_15003.json")).unwrap();
    assert_eq!(detail.summary.number, 15003);
    assert!(detail.summary.cross_repo);
    assert_eq!(detail.summary.checks, CheckRollup::Passing);
    assert!(!detail.checks_truncated);
    assert_eq!(
        detail.body,
        "### Description\n\nTrimmed from the recorded answer.\n"
    );
    assert_eq!(detail.head_sha, "38aef55a786788888bd90ddaeaac0bf6b0b98bb1");
    assert_eq!(detail.base_branch, "trunk");
    assert!(
        !detail.can_close && !detail.can_reopen,
        "as GitHub answered once the PR was closed"
    );
    assert_eq!(detail.author.as_deref(), Some("pad895"));
    assert_eq!(
        detail.mergeability,
        Mergeability::Blocked(BlockReason::Requirements)
    );
    assert_eq!(
        detail.methods,
        [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase]
    );
    assert_eq!(detail.default_method, Some(MergeMethod::Merge));
    assert_eq!(
        detail.reviewers,
        [
            (
                "copilot-pull-request-reviewer".to_owned(),
                ReviewerState::Commented
            ),
            ("sergiou87".to_owned(), ReviewerState::Requested),
        ],
        "the hidden team request is left out"
    );
    assert_eq!(detail.open_threads, 2);

    // 14 runs; the five jobs run twice collapse to their latest run, 9
    // checks in all, passing before skipped.
    let checks: Vec<(&str, CheckStatus)> = detail
        .checks
        .iter()
        .map(|e| (e.run.name.as_str(), e.run.status))
        .collect();
    assert_eq!(checks.len(), 9, "{checks:?}");
    let passing = checks
        .iter()
        .take_while(|(_, s)| *s == CheckStatus::Success)
        .count();
    assert_eq!(passing, 2, "{checks:?}");
    assert!(
        checks[passing..]
            .iter()
            .all(|(_, s)| *s == CheckStatus::Ignored)
    );
    let rerun = &detail.checks[1];
    assert_eq!(rerun.run.name, "check-requirements / check-requirements");
    assert_eq!(
        rerun.run.started_at.as_deref(),
        Some("2026-11-09T23:22:03Z"),
        "the latest run"
    );
    assert_eq!(rerun.duration, Some(std::time::Duration::from_secs(4)));
}

/// The fixture's answer with `edit` applied to its repository object.
fn detail_answer(edit: impl FnOnce(&mut Value)) -> String {
    let mut answer: Value = serde_json::from_str(&fixture("pr_detail_15003.json")).unwrap();
    edit(&mut answer["data"]["repository"]);
    answer.to_string()
}

#[test]
fn a_partial_check_list_keeps_githubs_overall_verdict() {
    let cases = [
        ("FAILURE", CheckRollup::Failing { failed: None }),
        ("PENDING", CheckRollup::Pending),
        ("SUCCESS", CheckRollup::Passing),
    ];
    for (state, expected) in cases {
        let answer = detail_answer(|repo| {
            let rollup = &mut repo["pullRequest"]["statusCheckRollup"];
            rollup["state"] = json!(state);
            rollup["contexts"]["pageInfo"]["hasNextPage"] = json!(true);
        });
        let loaded = parse_pr_detail(&answer).unwrap();
        assert!(loaded.checks_truncated, "{state}");
        assert_eq!(loaded.summary.checks, expected, "{state}");
        assert!(loaded.summary.failed.is_empty(), "the listed checks passed");
    }
}

#[test]
fn the_allowed_merge_methods_follow_the_repository() {
    use MergeMethod::*;
    let cases = [
        (true, true, true, vec![Merge, Squash, Rebase]),
        (true, true, false, vec![Merge, Squash]),
        (true, false, true, vec![Merge, Rebase]),
        (false, true, true, vec![Squash, Rebase]),
        (true, false, false, vec![Merge]),
        (false, true, false, vec![Squash]),
        (false, false, true, vec![Rebase]),
        (false, false, false, vec![]),
    ];
    for (merge, squash, rebase, expected) in cases {
        let json = detail_answer(|repo| {
            repo["mergeCommitAllowed"] = json!(merge);
            repo["squashMergeAllowed"] = json!(squash);
            repo["rebaseMergeAllowed"] = json!(rebase);
        });
        let methods = parse_pr_detail(&json).unwrap().methods;
        assert_eq!(methods, expected, "{merge} {squash} {rebase}");
    }
}

#[test]
fn the_viewers_default_merge_method_is_read() {
    use MergeMethod::*;
    let cases = [
        (json!("SQUASH"), Some(Squash)),
        (json!("REBASE"), Some(Rebase)),
        (json!("MERGE"), Some(Merge)),
        (json!("SOMETHING_NEW"), None),
        (Value::Null, None),
    ];
    for (value, expected) in cases {
        let json = detail_answer(|repo| repo["viewerDefaultMergeMethod"] = value.clone());
        let method = parse_pr_detail(&json).unwrap().default_method;
        assert_eq!(method, expected, "{value}");
    }
    let absent = detail_answer(|repo| {
        repo.as_object_mut()
            .unwrap()
            .remove("viewerDefaultMergeMethod");
    });
    assert_eq!(parse_pr_detail(&absent).unwrap().default_method, None);
}

#[test]
fn a_default_method_the_repository_does_not_allow_is_dropped() {
    use MergeMethod::*;
    // (label, squash allowed, GitHub's default, the default kept)
    let cases = [
        ("allowed", true, "SQUASH", Some(Squash)),
        ("not allowed", false, "SQUASH", None),
        ("another one is allowed", false, "MERGE", Some(Merge)),
    ];
    for (label, squash, value, expected) in cases {
        let json = detail_answer(|repo| {
            repo["squashMergeAllowed"] = json!(squash);
            repo["viewerDefaultMergeMethod"] = json!(value);
        });
        let detail = parse_pr_detail(&json).unwrap();
        assert_eq!(detail.default_method, expected, "{label}");
        assert_eq!(detail.methods.contains(&Squash), squash, "{label}");
    }
}

#[test]
fn githubs_merge_state_maps_to_whether_it_can_merge() {
    use BlockReason::*;
    use Mergeability::*;
    let cases = [
        ("MERGEABLE", "CLEAN", Ready),
        ("MERGEABLE", "UNSTABLE", Ready),
        ("MERGEABLE", "HAS_HOOKS", Ready),
        ("MERGEABLE", "BLOCKED", Blocked(Requirements)),
        ("MERGEABLE", "BEHIND", Blocked(Behind)),
        ("MERGEABLE", "DRAFT", Blocked(Draft)),
        ("CONFLICTING", "DIRTY", Conflicting),
        // A conflict is certain whatever the state says.
        ("CONFLICTING", "BLOCKED", Conflicting),
        ("CONFLICTING", "UNKNOWN", Conflicting),
        ("UNKNOWN", "UNKNOWN", Unknown),
        ("MERGEABLE", "SOMETHING_NEW", Unknown),
    ];
    for (mergeable, state, expected) in cases {
        assert_eq!(
            mergeability(mergeable, state),
            expected,
            "{mergeable} {state}"
        );
    }
}

#[test]
fn githubs_review_states_map_to_a_reviewers_state() {
    use ReviewerState::*;
    let cases = [
        ("APPROVED", Approved),
        ("CHANGES_REQUESTED", ChangesRequested),
        ("COMMENTED", Commented),
        ("DISMISSED", Dismissed),
        ("PENDING", Pending),
        ("SOMETHING_NEW", Other),
    ];
    for (state, expected) in cases {
        assert_eq!(reviewer_state(state), expected, "{state}");
    }
}

#[test]
fn a_reviewer_asked_again_is_requested_and_teams_are_named() {
    let json = detail_answer(|repo| {
        let pr = &mut repo["pullRequest"];
        pr["latestReviews"]["nodes"] = json!([
            {"author": {"login": "ann"}, "state": "APPROVED"},
            {"author": {"login": "bob"}, "state": "CHANGES_REQUESTED"},
            {"author": null, "state": "COMMENTED"},
        ]);
        pr["reviewRequests"]["nodes"] = json!([
            {"requestedReviewer": {"__typename": "User", "login": "bob"}},
            {"requestedReviewer": {"__typename": "Team", "combinedSlug": "cli/core"}},
            {"requestedReviewer": {"__typename": "SomethingNew"}},
            {"requestedReviewer": null},
        ]);
    });
    let reviewers = parse_pr_detail(&json).unwrap().reviewers;
    let expected = [
        ("ann", ReviewerState::Approved),
        ("bob", ReviewerState::Requested),
        ("cli/core", ReviewerState::Requested),
    ]
    .map(|(name, state)| (name.to_owned(), state));
    assert_eq!(reviewers, expected);
}

#[test]
fn a_missing_pull_request_is_a_parse_error_carrying_the_message() {
    let json = r#"{"data":{"repository":{"mergeCommitAllowed":true,"squashMergeAllowed":true,
        "rebaseMergeAllowed":true,"pullRequest":null}},
        "errors":[{"message":"Could not resolve to a PullRequest with the number of 9."}]}"#;
    match parse_pr_detail(json) {
        Err(GhError::Parse(message)) => assert!(message.contains("number of 9"), "{message}"),
        other => panic!("expected a parse error, got {other:?}"),
    }
}

#[test]
fn the_detail_query_names_every_field_the_parser_reads() {
    let query = pr_detail_query();
    for field in [
        "$number: Int!",
        "pullRequest(number: $number)",
        "mergeCommitAllowed",
        "squashMergeAllowed",
        "rebaseMergeAllowed",
        "viewerDefaultMergeMethod",
        "...pr",
        "fragment pr on PullRequest",
        "body",
        "headRefOid",
        "baseRefName",
        "changedFiles",
        "author { login }",
        "mergeable",
        "mergeStateStatus",
        "latestReviews",
        "reviewRequests",
        "... on Team { combinedSlug }",
        "reviewThreads",
        "isResolved",
        "completedAt",
    ] {
        assert!(query.contains(field), "{field}");
    }
}

#[test]
fn the_viewers_close_and_reopen_permissions_are_read() {
    let answer = detail_answer(|repo| {
        repo["pullRequest"]["viewerCanClose"] = json!(true);
        repo["pullRequest"]["viewerCanReopen"] = json!(true);
    });
    let loaded = parse_pr_detail(&answer).unwrap();
    assert!(loaded.can_close && loaded.can_reopen);
    assert!(pr_detail_query().contains("viewerCanClose viewerCanReopen"));
}

#[test]
fn auto_merge_and_the_merge_queue_are_read() {
    use crate::{AutoMergeOn, QueueEntry, QueueState};
    let plain = parse_pr_detail(&fixture("pr_detail_15003.json")).unwrap();
    assert_eq!((plain.auto_merge, plain.queue), (None, None));
    let answer = detail_answer(|repo| {
        let pr = &mut repo["pullRequest"];
        pr["autoMergeRequest"] = json!({"mergeMethod": "SQUASH", "enabledBy": {"login": "al"}});
        pr["mergeQueueEntry"] = json!({"position": 2, "state": "AWAITING_CHECKS"});
        pr["viewerCanDisableAutoMerge"] = json!(true);
    });
    let loaded = parse_pr_detail(&answer).unwrap();
    assert_eq!(
        loaded.auto_merge,
        Some(AutoMergeOn {
            method: Some(MergeMethod::Squash),
            by: Some("al".into()),
        })
    );
    assert_eq!(
        loaded.queue,
        Some(QueueEntry {
            position: Some(2),
            state: QueueState::AwaitingChecks,
        })
    );
    assert!(loaded.can_disable_auto_merge && !loaded.can_enable_auto_merge);
    let unknown = detail_answer(|repo| {
        repo["pullRequest"]["mergeQueueEntry"] = json!({"position": null, "state": "NEW_STATE"});
    });
    let queue = parse_pr_detail(&unknown).unwrap().queue.unwrap();
    assert_eq!((queue.position, queue.state), (None, QueueState::Other));
}
