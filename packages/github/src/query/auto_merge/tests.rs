//! The answers are shaped as GitHub documents the mutations' payloads; no
//! auto-merge was turned on or off on GitHub to record them.

use super::*;

#[test]
fn an_answer_says_whether_auto_merge_changed() {
    let on = r#"{"data":{"enablePullRequestAutoMerge":{"pullRequest":{"autoMergeRequest":{"mergeMethod":"SQUASH"}}}}}"#;
    let off =
        r#"{"data":{"disablePullRequestAutoMerge":{"pullRequest":{"autoMergeRequest":null}}}}"#;
    let queued = r#"{"data":{"enablePullRequestAutoMerge":{"pullRequest":{"autoMergeRequest":null,"mergeQueueEntry":{"position":1}}}}}"#;
    let refused = r#"{"data":{"enablePullRequestAutoMerge":null},"errors":[{"message":"Pull request is in clean status"}]}"#;
    // (label, answer, asked on, accepted)
    let cases = [
        ("turned on", on, true, true),
        ("turned off", off, false, true),
        ("still off after asking on", off, true, false),
        ("still on after asking off", on, false, false),
        ("refused", refused, true, false),
        ("queued at once", queued, true, true),
    ];
    for (label, answer, asked, accepted) in cases {
        assert_eq!(parse_auto_merge(answer, asked).is_ok(), accepted, "{label}");
    }
    let Err(GhError::Parse(said)) = parse_auto_merge(refused, true) else {
        panic!("refused");
    };
    assert!(said.contains("clean status"), "GitHub's words: {said}");
}

#[test]
fn methods_have_githubs_names() {
    assert_eq!(method_name(MergeMethod::Squash), "SQUASH");
    assert_eq!(method_name(MergeMethod::Merge), "MERGE");
    assert_eq!(method_name(MergeMethod::Rebase), "REBASE");
    assert!(ENABLE_AUTO_MERGE_MUTATION.contains("expectedHeadOid: $head"));
    assert!(ENABLE_AUTO_MERGE_MUTATION.contains("includeStack: $stack"));
}

#[test]
fn an_answer_says_whether_it_left_the_queue() {
    let out = r#"{"data":{"dequeuePullRequest":{"mergeQueueEntry":{"position":2}}}}"#;
    let refused = r#"{"data":{"dequeuePullRequest":null},"errors":[{"message":"Viewer does not have permission"}]}"#;
    assert!(parse_dequeue(out).is_ok());
    let Err(GhError::Parse(said)) = parse_dequeue(refused) else {
        panic!("refused");
    };
    assert!(said.contains("permission"), "GitHub's words: {said}");
    assert!(DEQUEUE_MUTATION.contains("dequeuePullRequest(input: {id: $pr})"));
}
