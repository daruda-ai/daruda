//! `search_nodes.json` is a real answer to the search query with
//! `first: 3` for `is:pr is:open archived:false sort:updated-desc
//! repo:cli/cli`, 2026-11-13: fork PRs and a same-repository draft, 56
//! matches in all; recorded again the same day when the query began asking
//! for `headRepositoryOwner`. Its nodes carry the fields the paged search
//! asks for each pull request.

use serde_json::Value;

use super::{is_not_a_pull_request, parse_search_node, search_query};
use crate::{CheckRollup, GhError, PrListItem, PrState, RepoId};

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

/// The list items of a search answer's nodes, those that are pull requests.
fn parse_nodes(json: &str, host: &str) -> Result<Vec<PrListItem>, GhError> {
    let answer: Value = serde_json::from_str(json).unwrap();
    let Value::Array(nodes) = answer["data"]["search"]["nodes"].clone() else {
        panic!("no nodes in {json}");
    };
    nodes
        .into_iter()
        .filter(|node| !is_not_a_pull_request(node))
        .map(|node| parse_search_node(node, host))
        .collect()
}

#[test]
fn real_search_nodes_parse_fork_and_same_repository_prs() {
    let items = parse_nodes(&fixture("search_nodes.json"), "github.com").unwrap();
    let cli = RepoId::new("github.com", "cli", "cli");
    let numbers: Vec<u64> = items.iter().map(|i| i.pr.number).collect();
    assert_eq!(
        numbers,
        [15003, 13781, 14920],
        "GitHub's updated-desc order"
    );
    assert!(items.iter().all(|i| i.repo == cli && i.comments == 1));

    let fork = &items[0];
    assert!(fork.pr.cross_repo);
    assert_eq!(fork.pr.state, PrState::Open);
    assert_eq!(fork.pr.checks, CheckRollup::Passing);
    assert_eq!(fork.author.as_deref(), Some("pad895"));
    assert_eq!(fork.head_owner.as_deref(), Some("pad895"));
    assert_eq!(fork.base_branch, "trunk");

    let fork_draft = &items[1];
    assert!(fork_draft.pr.cross_repo);
    assert_eq!(fork_draft.pr.state, PrState::Draft);
    assert_eq!(fork_draft.pr.checks, CheckRollup::None, "no checks ran");

    let draft = &items[2];
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

/// A search answer of one node; `repo`, `author` and `rollup` are raw JSON
/// values.
fn search_answer(repo: &str, author: &str, rollup: &str) -> String {
    format!(
        r#"{{"data":{{"search":{{"issueCount":1,"nodes":[{{"number":7,"url":"u","title":"t","state":"OPEN",
        "isDraft":false,"headRefName":"h","isCrossRepository":false,"reviewDecision":null,
        "updatedAt":"2026-01-01T00:00:00Z","statusCheckRollup":{rollup},
        "repository":{{"nameWithOwner":{repo}}},"headRepositoryOwner":null,"author":{author},"baseRefName":"main",
        "comments":{{"totalCount":3}}}}]}}}}}}"#
    )
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
        let items = parse_nodes(&json, host).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(items.len(), 1, "{label}");
        let item = &items[0];
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
fn a_repository_that_is_not_owner_name_is_a_parse_error() {
    let json = search_answer(r#""noslash""#, "null", "null");
    let error = parse_nodes(&json, "github.com").unwrap_err();
    assert!(
        matches!(&error, GhError::Parse(m) if m.contains("noslash")),
        "{error}"
    );
}

#[test]
fn the_search_query_names_every_field_the_parser_reads() {
    let query = search_query("first: 1", "", "");
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
