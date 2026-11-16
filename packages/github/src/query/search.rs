//! The search that lists open pull requests across repositories and the
//! parser for its answer.

use serde::Deserialize;

use super::{ApiError, Author, PrNode, api_messages, parse_error, pr_fragment};
use crate::{GhError, PrFilter, PrListItem, PrSearch, RepoId};

/// Pull requests one search answers; more are left out of the list.
const SEARCH_RESULTS: u32 = 50;

/// The search text for open pull requests of `repos` matching `filter`,
/// most recently updated first so a truncated answer keeps the live ones.
/// Archived repositories take no new work, so their PRs are left out.
pub(crate) fn search_string(repos: &[RepoId], filter: PrFilter) -> String {
    let mut text = String::from("is:pr is:open archived:false sort:updated-desc");
    for repo in repos {
        text.push_str(&format!(" repo:{}/{}", repo.owner(), repo.name()));
    }
    match filter {
        PrFilter::Mine => text.push_str(" author:@me"),
        PrFilter::ReviewRequested => text.push_str(" review-requested:@me"),
        PrFilter::AllOpen => {}
    }
    text
}

/// The search for open pull requests, bound to `$q` ([`search_string`]).
pub(crate) fn search_prs_query() -> String {
    format!(
        "query($q: String!) {{ search(query: $q, type: ISSUE, first: {SEARCH_RESULTS}) {{ \
         issueCount nodes {{ ... on PullRequest {{ ...pr repository {{ nameWithOwner }} \
         headRepositoryOwner {{ login }} author {{ login }} baseRefName \
         comments {{ totalCount }} }} }} }} }} {}",
        pr_fragment()
    )
}

/// Parse the answer to [`search_prs_query`] asked of `host`.
pub(crate) fn parse_search_prs(json: &str, host: &str) -> Result<PrSearch, GhError> {
    let response: SearchResponse = serde_json::from_str(json).map_err(parse_error)?;
    let Some(search) = response.data.and_then(|d| d.search) else {
        return Err(GhError::Parse(format!(
            "no search in the answer: {}",
            api_messages(response.errors)
        )));
    };
    let truncated = search.issue_count as usize > search.nodes.len();
    let items = search
        .nodes
        .into_iter()
        .filter(|node| !is_not_a_pull_request(node))
        .map(|node| {
            let node: SearchNode = serde_json::from_value(node).map_err(parse_error)?;
            let (owner, name) =
                node.repository
                    .name_with_owner
                    .split_once('/')
                    .ok_or_else(|| {
                        GhError::Parse(format!(
                            "repository {} is not owner/name",
                            node.repository.name_with_owner
                        ))
                    })?;
            Ok(PrListItem {
                repo: RepoId::new(host, owner, name),
                head_owner: node.head_repository_owner.map(|a| a.login),
                author: node.author.map(|a| a.login),
                base_branch: node.base_ref_name,
                comments: node.comments.total_count,
                pr: node.pr.into_summary()?,
            })
        })
        .collect::<Result<Vec<_>, GhError>>()?;
    Ok(PrSearch { items, truncated })
}

/// A search node that carries no pull request: `null`, or the empty object
/// `... on PullRequest` leaves for a match of another type. One such entry
/// must not fail the whole list; a node with fields still has to parse.
fn is_not_a_pull_request(node: &serde_json::Value) -> bool {
    match node {
        serde_json::Value::Null => true,
        serde_json::Value::Object(fields) => fields.is_empty(),
        _ => false,
    }
}

/// Order list items most recently updated first. `updated_at` is RFC 3339
/// in UTC, so string order is time order; the sort is stable for ties.
pub(crate) fn newest_first(items: &mut [PrListItem]) {
    items.sort_by(|a, b| b.pr.updated_at.cmp(&a.pr.updated_at));
}

#[derive(Deserialize)]
struct SearchResponse {
    data: Option<SearchData>,
    errors: Option<Vec<ApiError>>,
}

#[derive(Deserialize)]
struct SearchData {
    search: Option<SearchConnection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchConnection {
    issue_count: u32,
    /// Parsed one by one, so an entry that is not a pull request is skipped.
    nodes: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchNode {
    #[serde(flatten)]
    pr: PrNode,
    repository: NameWithOwner,
    head_repository_owner: Option<Author>,
    author: Option<Author>,
    base_ref_name: String,
    comments: TotalCount,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameWithOwner {
    name_with_owner: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TotalCount {
    total_count: u32,
}

#[cfg(test)]
mod tests;
