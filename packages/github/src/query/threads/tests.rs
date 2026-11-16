//! `review_threads_*.json` are real answers to `REVIEW_THREADS_QUERY`
//! (`cli/cli` PRs 13506 and 13982, gh 2.74.2, 2026-11-12).

use super::{REVIEW_THREADS_QUERY, parse_review_threads};

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn resolved_threads_are_dropped() {
    let threads = parse_review_threads(&fixture("review_threads_13506.json")).unwrap();
    // 7 threads, 3 of them resolved.
    assert_eq!(threads.threads.len(), 4);
    assert!(!threads.truncated);
    let multi = threads
        .threads
        .iter()
        .find(|t| t.comments.len() == 2 && !t.outdated)
        .expect("an open thread with a reply");
    assert_eq!(multi.lines.map(|(_, end)| end), Some(48));
}

#[test]
fn an_outdated_thread_falls_back_to_where_it_was_written() {
    let threads = parse_review_threads(&fixture("review_threads_13982.json")).unwrap();
    assert_eq!(threads.threads.len(), 14, "none resolved");
    let moved = threads
        .threads
        .iter()
        .find(|t| t.path == "pkg/cmd/extension/http.go")
        .unwrap();
    assert!(moved.outdated);
    assert_eq!(
        moved.lines,
        Some((157, 160)),
        "line is null: originalStartLine..originalLine"
    );
    assert!(threads.threads.iter().all(|t| t.lines.is_some()));
}

#[test]
fn a_missing_pull_request_is_an_error() {
    let json = r#"{"data":{"repository":{"pullRequest":null}},"errors":[{"message":"Could not resolve to a PullRequest"}]}"#;
    assert!(parse_review_threads(json).is_err());
    assert!(REVIEW_THREADS_QUERY.contains("originalLine"));
}

#[test]
fn a_thread_with_more_replies_than_read_says_so() {
    let json = r#"{"data":{"repository":{"pullRequest":{"reviewThreads":{"pageInfo":{"hasNextPage":false},
        "nodes":[{"isResolved":false,"isOutdated":false,"path":"a.rs","line":3,"startLine":null,
        "originalLine":3,"originalStartLine":null,"comments":{"totalCount":25,
        "nodes":[{"author":{"login":"rev"},"body":"first"}]}}]}}}}}"#;
    let threads = parse_review_threads(json).unwrap();
    assert!(threads.threads[0].more_comments, "25 replies, 1 read");
}
