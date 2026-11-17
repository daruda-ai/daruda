use crate::prompt::review_threads;
use crate::{CheckRollup, PrState, PrSummary, ReviewComment, ReviewThread, ReviewThreads};

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

fn comment(author: Option<&str>, body: &str) -> ReviewComment {
    ReviewComment {
        author: author.map(str::to_owned),
        body: body.into(),
        ..Default::default()
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
                ..Default::default()
            },
            ReviewThread {
                path: "src/b.rs".into(),
                lines: Some((7, 7)),
                outdated: true,
                comments: vec![comment(Some("rev"), "\u{1b}[31mstale\u{1b}[0m")],
                more_comments: false,
                ..Default::default()
            },
            ReviewThread {
                path: "README.md".into(),
                lines: None,
                outdated: false,
                comments: vec![comment(Some("rev"), "Whole file")],
                more_comments: false,
                ..Default::default()
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
                ..Default::default()
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
            ..Default::default()
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
fn no_open_thread_is_no_prompt() {
    assert!(review_threads(&pr(), &ReviewThreads::default()).is_none());
}
