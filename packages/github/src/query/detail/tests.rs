//! `pr_detail_15003.json` is a real `gh api graphql` answer to
//! `pr_detail_query` for `cli/cli` #15003 (gh 2.74.2, 2026-11-13), its body
//! trimmed: a fork PR blocked on a review, with a request the caller cannot
//! see (`requestedReviewer: null`) and five jobs run twice.

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
    assert_eq!(
        detail.body,
        "### Description\n\nTrimmed from the recorded answer.\n"
    );
    assert_eq!(detail.head_sha, "38aef55a786788888bd90ddaeaac0bf6b0b98bb1");
    assert_eq!(detail.base_branch, "trunk");
    assert_eq!(detail.author.as_deref(), Some("pad895"));
    assert_eq!(
        detail.mergeability,
        Mergeability::Blocked(BlockReason::Requirements)
    );
    assert_eq!(
        detail.methods,
        [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase]
    );
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
        "...pr",
        "fragment pr on PullRequest",
        "body",
        "headRefOid",
        "baseRefName",
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
