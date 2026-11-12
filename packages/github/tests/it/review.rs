//! `review_threads_*.json` are real answers to `REVIEW_THREADS_QUERY`
//! (`cli/cli` PRs 13506 and 13982, gh 2.74.2, 2026-11-12).

use daruda_github::prompt::review_threads;
use daruda_github::query::{REVIEW_THREADS_QUERY, parse_review_threads};
use daruda_github::{CheckRollup, PrState, PrSummary, ReviewComment, ReviewThread, ReviewThreads};

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

fn pr() -> PrSummary {
    PrSummary {
        number: 9,
        url: String::new(),
        title: "Tidy".into(),
        state: PrState::Open,
        head_branch: "feat".into(),
        cross_repo: false,
        checks: CheckRollup::None,
        failed: Vec::new(),
        review: None,
        updated_at: String::new(),
    }
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

fn comment(author: Option<&str>, body: &str) -> ReviewComment {
    ReviewComment {
        author: author.map(str::to_owned),
        body: body.into(),
    }
}

#[test]
fn the_prompt_places_each_comment_and_indents_replies() {
    let threads = ReviewThreads {
        threads: vec![
            ReviewThread {
                path: "src/a.rs".into(),
                lines: Some((12, 20)),
                outdated: false,
                comments: vec![
                    comment(Some("rev"), "Rename this.\nIt reads oddly."),
                    comment(None, "Agreed"),
                ],
                more_comments: true,
            },
            ReviewThread {
                path: "src/b.rs".into(),
                lines: Some((7, 7)),
                outdated: true,
                comments: vec![comment(Some("rev"), "\u{1b}[31mstale\u{1b}[0m")],
                more_comments: false,
            },
            ReviewThread {
                path: "README.md".into(),
                lines: None,
                outdated: false,
                comments: vec![comment(Some("rev"), "Whole file")],
                more_comments: false,
            },
        ],
        truncated: true,
    };
    let prompt = review_threads(&pr(), &threads).unwrap();
    assert!(prompt.starts_with("Pull request #9 (Tidy) has unresolved review comments."));
    assert!(prompt.contains(
        "In src/a.rs:L12-L20:\n> rev: Rename this.\n> It reads oddly.\n  > (deleted user): Agreed\n  (more replies on GitHub)\n"
    ));
    assert!(prompt.contains("In src/b.rs:L7 (outdated):\n> rev: stale\n"));
    assert!(prompt.contains("In README.md:\n> rev: Whole file\n"));
    assert!(prompt.contains("more threads exist on GitHub"));
}

#[test]
fn huge_comments_stay_within_the_budget() {
    let body = "x".repeat(60_000);
    let threads = ReviewThreads {
        threads: (0..20)
            .map(|i| ReviewThread {
                path: format!("src/{i}.rs"),
                lines: Some((1, 1)),
                outdated: false,
                comments: vec![comment(Some("rev"), &body)],
                more_comments: false,
            })
            .collect(),
        truncated: false,
    };
    let prompt = review_threads(&pr(), &threads).unwrap();
    assert!(
        prompt.chars().count() <= 30_000,
        "{}",
        prompt.chars().count()
    );
    assert!(prompt.contains("the prompt is full"));
}

#[test]
fn one_thread_longer_than_the_budget_is_cut_not_dropped() {
    let replies = (0..20)
        .map(|_| comment(Some("rev"), &"y".repeat(5_000)))
        .collect();
    let threads = ReviewThreads {
        threads: vec![ReviewThread {
            path: "src/big.rs".into(),
            lines: Some((1, 2)),
            outdated: false,
            comments: replies,
            more_comments: false,
        }],
        truncated: false,
    };
    let prompt = review_threads(&pr(), &threads).unwrap();
    assert!(
        prompt.contains("In src/big.rs:L1-L2:"),
        "the thread is still there"
    );
    assert!(
        prompt.chars().count() <= 30_000,
        "{}",
        prompt.chars().count()
    );
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

#[test]
fn no_open_thread_is_no_prompt() {
    assert!(review_threads(&pr(), &ReviewThreads::default()).is_none());
}
