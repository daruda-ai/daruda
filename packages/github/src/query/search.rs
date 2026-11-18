//! The search query's shape and the parser for one of its nodes, which
//! the paged search shares.

use serde::Deserialize;

use super::{Author, PrNode, parse_error, pr_fragment};
use crate::{GhError, PrListItem, RepoId};

/// The search query for pull requests bound to `$q`, with the connection
/// `arguments`, their variable `declarations` and extra connection `fields`.
pub(super) fn search_query(arguments: &str, declarations: &str, fields: &str) -> String {
    format!(
        "query($q: String!{declarations}) {{ search(query: $q, type: ISSUE, {arguments}) {{ \
         issueCount {fields} nodes {{ ... on PullRequest {{ ...pr repository {{ nameWithOwner }} \
         headRepositoryOwner {{ login }} author {{ login }} baseRefName \
         comments {{ totalCount }} }} }} }} }} {}",
        pr_fragment()
    )
}

/// A search node that carries no pull request: `null`, or the empty object
/// `... on PullRequest` leaves for a match of another type. One such entry
/// must not fail the whole list; a node with fields still has to parse.
pub(super) fn is_not_a_pull_request(node: &serde_json::Value) -> bool {
    match node {
        serde_json::Value::Null => true,
        serde_json::Value::Object(fields) => fields.is_empty(),
        _ => false,
    }
}

pub(super) fn parse_search_node(
    node: serde_json::Value,
    host: &str,
) -> Result<PrListItem, GhError> {
    let node: SearchNode = serde_json::from_value(node).map_err(parse_error)?;
    let (owner, name) = node
        .repository
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
