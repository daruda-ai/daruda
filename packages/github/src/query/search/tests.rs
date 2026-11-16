//! `search_prs.json` answers `search_prs_query` (with `first: 3`) for
//! `is:pr is:open archived:false sort:updated-desc repo:cli/cli`, 2026-11-13:
//! fork PRs and a same-repository draft, 56 matches in all; recorded again
//! the same day when the query began asking for `headRepositoryOwner`.

use super::{newest_first, parse_search_prs, search_prs_query, search_string};
use crate::{CheckRollup, GhError, PrFilter, PrListItem, PrState, RepoId};

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn a_real_search_answer_parses_fork_and_same_repository_prs() {
    let search = parse_search_prs(&fixture("search_prs.json"), "github.com").unwrap();
    assert!(search.truncated, "56 matches, 3 answered");
    let cli = RepoId::new("github.com", "cli", "cli");
    let numbers: Vec<u64> = search.items.iter().map(|i| i.pr.number).collect();
    assert_eq!(
        numbers,
        [15003, 13781, 14920],
        "GitHub's updated-desc order"
    );
    assert!(
        search
            .items
            .iter()
            .all(|i| i.repo == cli && i.comments == 1)
    );

    let fork = &search.items[0];
    assert!(fork.pr.cross_repo);
    assert_eq!(fork.pr.state, PrState::Open);
    assert_eq!(fork.pr.checks, CheckRollup::Passing);
    assert_eq!(fork.author.as_deref(), Some("pad895"));
    assert_eq!(fork.head_owner.as_deref(), Some("pad895"));
    assert_eq!(fork.base_branch, "trunk");

    let fork_draft = &search.items[1];
    assert!(fork_draft.pr.cross_repo);
    assert_eq!(fork_draft.pr.state, PrState::Draft);
    assert_eq!(fork_draft.pr.checks, CheckRollup::None, "no checks ran");

    let draft = &search.items[2];
    assert!(!draft.pr.cross_repo);
    assert_eq!(draft.pr.state, PrState::Draft);
    assert_eq!(draft.pr.checks, CheckRollup::Passing);
    assert_eq!(draft.author.as_deref(), Some("williammartin"));
    assert_eq!(
        draft.head_owner.as_deref(),
        Some("cli"),
        "a same-repository head is owned by the repository, not the author"
    );
}

/// One search node of `count` matches; `repo`, `author` and `rollup` are
/// raw JSON values.
fn search_answer_of(count: u32, repo: &str, author: &str, rollup: &str) -> String {
    format!(
        r#"{{"data":{{"search":{{"issueCount":{count},"nodes":[{{"number":7,"url":"u","title":"t","state":"OPEN",
        "isDraft":false,"headRefName":"h","isCrossRepository":false,"reviewDecision":null,
        "updatedAt":"2026-01-01T00:00:00Z","statusCheckRollup":{rollup},
        "repository":{{"nameWithOwner":{repo}}},"headRepositoryOwner":null,"author":{author},"baseRefName":"main",
        "comments":{{"totalCount":3}}}}]}}}}}}"#
    )
}

fn search_answer(repo: &str, author: &str, rollup: &str) -> String {
    search_answer_of(1, repo, author, rollup)
}

#[test]
fn search_nodes_parse_into_list_items() {
    let truncated = r#"{"state":"PENDING","contexts":{"pageInfo":{"hasNextPage":true},
        "nodes":[{"__typename":"CheckRun","name":"x","status":"COMPLETED",
        "conclusion":"SUCCESS","detailsUrl":null,"startedAt":null}]}}"#;
    let cases = [
        (
            "mixed-case repository and host are lowercased",
            search_answer(r#""Octo-Org/Hello-World""#, r#"{"login":"Me"}"#, "null"),
            "GHE.Corp",
            RepoId::new("ghe.corp", "octo-org", "hello-world"),
            Some("Me"),
            CheckRollup::None,
        ),
        (
            "a truncated check list defers to GitHub's state",
            search_answer(r#""o/r""#, r#"{"login":"a"}"#, truncated),
            "github.com",
            RepoId::new("github.com", "o", "r"),
            Some("a"),
            CheckRollup::Pending,
        ),
        (
            "a deleted author is none",
            search_answer(r#""o/r""#, "null", "null"),
            "github.com",
            RepoId::new("github.com", "o", "r"),
            None,
            CheckRollup::None,
        ),
        (
            "entries that are not pull requests are skipped",
            search_answer(r#""o/r""#, r#"{"login":"a"}"#, "null")
                .replace(r#""nodes":["#, r#""nodes":[null,{},"#),
            "github.com",
            RepoId::new("github.com", "o", "r"),
            Some("a"),
            CheckRollup::None,
        ),
    ];
    for (label, json, host, repo, author, checks) in cases {
        let search = parse_search_prs(&json, host).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert!(!search.truncated, "{label}");
        assert_eq!(search.items.len(), 1, "{label}");
        let item = &search.items[0];
        assert_eq!(item.repo, repo, "{label}");
        assert_eq!(item.author.as_deref(), author, "{label}");
        assert_eq!(
            item.head_owner, None,
            "{label}: a deleted fork has no owner"
        );
        assert_eq!(item.pr.checks, checks, "{label}");
        assert_eq!(
            (item.comments, item.base_branch.as_str()),
            (3, "main"),
            "{label}"
        );
    }
}

#[test]
fn a_bad_search_answer_is_a_parse_error() {
    let cases = [
        (
            "an error answer carries GitHub's message",
            r#"{"data":null,"errors":[{"message":"Something went wrong"}]}"#.to_owned(),
            "Something went wrong",
        ),
        (
            "a repository that is not owner/name",
            search_answer(r#""noslash""#, "null", "null"),
            "noslash",
        ),
    ];
    for (label, json, expected) in cases {
        let error = parse_search_prs(&json, "github.com").unwrap_err();
        assert!(
            matches!(&error, GhError::Parse(m) if m.contains(expected)),
            "{label}: {error}"
        );
    }
}

#[test]
fn the_search_text_names_every_repository_and_the_filter() {
    let repos = [
        RepoId::new("github.com", "a", "b"),
        RepoId::new("github.com", "C", "D"),
    ];
    let base = "is:pr is:open archived:false sort:updated-desc repo:a/b repo:c/d";
    let cases = [
        (PrFilter::Mine, format!("{base} author:@me")),
        (
            PrFilter::ReviewRequested,
            format!("{base} review-requested:@me"),
        ),
        (PrFilter::AllOpen, base.to_owned()),
    ];
    for (filter, expected) in cases {
        assert_eq!(search_string(&repos, filter), expected, "{filter:?}");
    }
}

#[test]
fn the_search_query_names_every_field_the_parser_reads() {
    let query = search_prs_query();
    for field in [
        "search(query: $q, type: ISSUE",
        "issueCount",
        "... on PullRequest",
        "...pr",
        "fragment pr on PullRequest",
        "nameWithOwner",
        "headRepositoryOwner { login }",
        "author { login }",
        "baseRefName",
        "comments { totalCount }",
    ] {
        assert!(query.contains(field), "{field}");
    }
}

#[test]
fn more_matches_than_nodes_is_truncated() {
    let cases = [(1, false), (2, true)];
    for (count, truncated) in cases {
        let json = search_answer_of(count, r#""o/r""#, "null", "null");
        let search = parse_search_prs(&json, "github.com").unwrap();
        assert_eq!(search.truncated, truncated, "{count} matches");
    }
}

#[test]
fn list_items_sort_most_recently_updated_first() {
    let json = search_answer(r#""o/r""#, "null", "null");
    let template = parse_search_prs(&json, "github.com")
        .unwrap()
        .items
        .remove(0);
    let item = |number: u64, updated_at: &str| {
        let mut item: PrListItem = template.clone();
        item.pr.number = number;
        item.pr.updated_at = updated_at.to_owned();
        item
    };
    // Two chunks joined: each newest first, the whole not.
    let mut items = vec![
        item(1, "2026-11-02T00:00:00Z"),
        item(2, "2026-11-01T00:00:00Z"),
        item(3, "2026-11-03T00:00:00Z"),
        item(4, "2026-11-02T00:00:00Z"),
    ];
    newest_first(&mut items);
    let numbers: Vec<u64> = items.iter().map(|i| i.pr.number).collect();
    assert_eq!(numbers, [3, 1, 4, 2], "ties keep their order");
}
