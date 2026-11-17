//! The latest-review answer shape is as GitHub answered for cli/cli#13506
//! (2026-11-18, read-only); the mutation answers follow GitHub's schema, as
//! no review was submitted to record one.

use super::*;

#[test]
fn a_review_is_sent_with_its_lines_and_verdict() {
    let threads = [
        ReviewThreadDraft {
            path: "a.rs".into(),
            line: 5,
            side: DiffSide::Right,
            start_line: Some(3),
            body: "Simpler.".into(),
        },
        ReviewThreadDraft {
            path: "b.rs".into(),
            line: 9,
            side: DiffSide::Left,
            start_line: Some(9),
            body: "Why?".into(),
        },
    ];
    let request = submit_request(
        "PR_1",
        "abc",
        ReviewEvent::RequestChanges,
        "See inline.",
        &threads,
    );
    let vars = &request["variables"];
    assert_eq!(vars["event"], "REQUEST_CHANGES");
    assert_eq!(vars["commit"], "abc");
    assert_eq!(vars["threads"][0]["startLine"], 3);
    assert_eq!(vars["threads"][0]["startSide"], "RIGHT");
    assert!(
        vars["threads"][1].get("startLine").is_none(),
        "a one-line comment names no start"
    );
    assert_eq!(vars["threads"][1]["side"], "LEFT");
}

#[test]
fn answers_read_as_the_review_or_the_reason_it_is_not() {
    let done = r#"{"data":{"addPullRequestReview":{"pullRequestReview":{"id":"PRR_1","url":"https://github.com/o/n/pull/7#pullrequestreview-1","state":"COMMENTED"}}}}"#;
    assert_eq!(parse_submitted(done).unwrap().id, "PRR_1");
    let refused = r#"{"data":{"addPullRequestReview":null},"errors":[{"message":"Can not approve your own pull request"}]}"#;
    assert!(
        matches!(parse_submitted(refused), Err(GhError::Parse(m)) if m.contains("own pull request"))
    );
    let none = r#"{"data":{"repository":{"pullRequest":{"viewerLatestReview":null}}}}"#;
    assert_eq!(parse_latest_review(none).unwrap(), None);
    let some = r#"{"data":{"repository":{"pullRequest":{"viewerLatestReview":{"id":"PRR_2","state":"COMMENTED","submittedAt":"2026-11-18T00:00:00Z","body":"x"}}}}}"#;
    assert_eq!(
        parse_latest_review(some).unwrap(),
        Some(LatestReview {
            id: "PRR_2".into(),
            body: "x".into(),
            state: LatestReviewState::Given(ReviewEvent::Comment),
        })
    );
}
