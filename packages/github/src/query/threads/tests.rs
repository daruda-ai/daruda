//! `review_threads_*.json` are real answers to `review_threads_query`
//! (`cli/cli` PRs 13506 and 13982), and `review_replies_13982.json` to
//! `review_replies_query` for one of 13982's threads; gh 2.74.2,
//! 2026-11-18, recorded with the queries exactly as built here.

use super::*;

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

fn one_page(name: &str) -> ReviewThreads {
    read_review_threads(
        |after| {
            assert_eq!(after, None, "one page");
            Ok(fixture(name))
        },
        |_, _| panic!("no thread has more replies than it came with"),
    )
    .unwrap()
}

#[test]
fn resolved_threads_are_dropped() {
    let threads = one_page("review_threads_13506.json");
    // 7 threads, 3 of them resolved.
    assert_eq!(threads.threads.len(), 4);
    assert!(!threads.truncated);
    let multi = threads
        .threads
        .iter()
        .find(|t| t.comments.len() == 2 && !t.outdated)
        .expect("an open thread with a reply");
    assert_eq!(multi.lines.map(|(_, end)| end), Some(48));
    assert!(multi.id.starts_with("PRRT_"), "{}", multi.id);
    assert_eq!(multi.side, DiffSide::Right);
    assert_eq!(
        multi.may,
        crate::ThreadPermissions {
            reply: true,
            resolve: false,
            unresolve: false,
        },
        "the recording account may reply on cli/cli but not resolve"
    );
    let first = &multi.comments[0];
    assert!(first.id.starts_with("PRRC_"), "{}", first.id);
    assert!(
        first.url.as_deref().is_some_and(
            |url| url.starts_with("https://github.com/cli/cli/pull/13506#discussion_r")
        ),
        "{:?}",
        first.url
    );
}

#[test]
fn an_outdated_thread_falls_back_to_where_it_was_written() {
    let threads = one_page("review_threads_13982.json");
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
fn a_missing_pull_request_or_thread_is_an_error() {
    let json = r#"{"data":{"repository":{"pullRequest":null}},"errors":[{"message":"Could not resolve to a PullRequest"}]}"#;
    assert!(parse_threads_page(json).is_err());
    assert!(parse_replies_page(r#"{"data":{"node":null}}"#).is_err());
    let query = review_threads_query();
    assert!(query.contains("after: $after") && query.contains("diffSide startDiffSide"));
}

/// A page of one unresolved thread `id` on `a.rs`, with its first reply;
/// `replies` is the cursor of its later replies and `next` of the next page.
fn page(id: &str, replies: Option<&str>, next: Option<&str>) -> String {
    let cursor = |c: Option<&str>| c.map_or("null".to_owned(), |c| format!("\"{c}\""));
    format!(
        r#"{{"data":{{"repository":{{"pullRequest":{{"reviewThreads":{{
        "pageInfo":{{"hasNextPage":{},"endCursor":{}}},
        "nodes":[{{"id":"{id}","isResolved":false,"isOutdated":false,"path":"a.rs","line":3,
        "startLine":null,"originalLine":3,"originalStartLine":null,"diffSide":"LEFT",
        "subjectType":"LINE","comments":{{"totalCount":2,
        "pageInfo":{{"hasNextPage":{},"endCursor":{}}},
        "nodes":[{{"id":"{id}-c1","url":null,"author":{{"login":"rev"}},"body":"first"}}]}}}}]}}}}}}}}}}"#,
        next.is_some(),
        cursor(next),
        replies.is_some(),
        cursor(replies),
    )
}

#[test]
fn later_pages_and_later_replies_are_read_in_turn() {
    let mut asked = Vec::new();
    let threads = read_review_threads(
        |after| {
            asked.push(format!("threads after {after:?}"));
            Ok(match after {
                None => page("t1", Some("r1"), Some("p2")),
                Some("p2") => page("t2", None, None),
                other => panic!("{other:?}"),
            })
        },
        |thread, after| {
            assert_eq!((thread, after), ("t1", "r1"));
            Ok(fixture("review_replies_13982.json"))
        },
    )
    .unwrap();
    assert_eq!(asked, ["threads after None", "threads after Some(\"p2\")"]);
    assert_eq!(threads.threads.len(), 2, "a later page's threads are kept");
    assert_eq!(threads.threads[0].side, DiffSide::Left);
    assert_eq!(threads.threads[0].comments.len(), 1 + 3, "replies read on");
    assert!(!threads.threads[0].more_comments);
    assert!(!threads.truncated);
}

#[test]
fn reading_stops_at_its_caps_and_says_so() {
    let threads = read_review_threads(
        |_| Ok(page("t", Some("r"), Some("more"))),
        |_, _| Ok(r#"{"data":{"node":{"comments":{"pageInfo":{"hasNextPage":true,"endCursor":"r"},"nodes":[]}}}}"#.to_owned()),
    )
    .unwrap();
    assert_eq!(threads.threads.len(), THREAD_PAGES_MAX);
    assert!(threads.truncated, "more threads than are read");
    assert!(
        threads.threads.iter().all(|t| t.more_comments),
        "more replies than are read"
    );
}

#[test]
fn a_file_level_thread_has_no_lines() {
    let json = page("t", None, None).replace(r#""subjectType":"LINE""#, r#""subjectType":"FILE""#);
    let read = parse_threads_page(&json).unwrap();
    assert_eq!(read.threads[0].0.lines, None);
}

#[test]
fn a_failed_reply_page_marks_its_thread_and_the_rest_still_land() {
    let threads = read_review_threads(
        |_| Ok(page("t1", Some("r1"), None)),
        |_, _| Err(GhError::Parse("node is null".into())),
    )
    .unwrap();
    assert_eq!(threads.threads.len(), 1);
    assert_eq!(
        threads.threads[0].comments.len(),
        1,
        "what came with it is kept"
    );
    assert!(threads.threads[0].more_comments);
}

#[test]
fn a_range_starting_on_the_other_side_points_at_its_last_line() {
    let json = page("t", None, None).replace(
        r#""startLine":null"#,
        r#""startLine":1,"startDiffSide":"RIGHT""#,
    );
    let read = parse_threads_page(&json).unwrap();
    assert_eq!(
        read.threads[0].0.lines,
        Some((3, 3)),
        "starts RIGHT, ends LEFT"
    );
    let same = page("t", None, None).replace(
        r#""startLine":null"#,
        r#""startLine":1,"startDiffSide":"LEFT""#,
    );
    assert_eq!(
        parse_threads_page(&same).unwrap().threads[0].0.lines,
        Some((1, 3))
    );
}
