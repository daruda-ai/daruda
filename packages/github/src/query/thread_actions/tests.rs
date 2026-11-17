//! Answers shaped as GitHub's GraphQL schema documents them for
//! `addPullRequestReviewThreadReply`, `resolveReviewThread` and
//! `unresolveReviewThread`; no mutation was sent to record them.

use super::*;

#[test]
fn a_reply_answers_the_comment_posted() {
    let json = r#"{"data":{"addPullRequestReviewThreadReply":{"comment":{
        "id":"PRRC_1","url":"https://github.com/o/n/pull/7#discussion_r1",
        "author":{"login":"me"},"body":"Done."}}}}"#;
    let comment = parse_reply(json).unwrap();
    assert_eq!(
        (
            comment.id.as_str(),
            comment.author.as_deref(),
            comment.body.as_str()
        ),
        ("PRRC_1", Some("me"), "Done.")
    );
    let refused = r#"{"data":{"addPullRequestReviewThreadReply":null},
        "errors":[{"message":"Resource not accessible by integration"}]}"#;
    assert!(matches!(parse_reply(refused), Err(GhError::Parse(m)) if m.contains("not accessible")));
}

#[test]
fn resolving_answers_only_when_the_thread_stands_as_asked() {
    let resolved = r#"{"data":{"resolveReviewThread":{"thread":{"id":"t","isResolved":true}}}}"#;
    let unresolved =
        r#"{"data":{"unresolveReviewThread":{"thread":{"id":"t","isResolved":false}}}}"#;
    // (label, answer, asked resolved, ok)
    let cases = [
        ("resolved", resolved, true, true),
        ("unresolved", unresolved, false, true),
        ("still unresolved", unresolved, true, false),
        (
            "refused",
            r#"{"data":null,"errors":[{"message":"no"}]}"#,
            true,
            false,
        ),
    ];
    for (label, json, asked, ok) in cases {
        assert_eq!(parse_resolved(json, asked).is_ok(), ok, "{label}");
    }
    assert!(RESOLVE_MUTATION.contains("resolveReviewThread"));
    assert!(UNRESOLVE_MUTATION.contains("unresolveReviewThread"));
}
