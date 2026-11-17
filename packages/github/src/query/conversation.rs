//! The request for a pull request's conversation: the comments on the pull
//! request itself, not on its diff; page by page, and the parser for it.

use serde::Deserialize;

use super::{ApiError, Author, api_messages, parse_error};
use crate::{Conversation, GhError, ReviewComment};

/// Pages of comments read for one pull request: a thousand comments.
pub(crate) const CONVERSATION_PAGES_MAX: usize = 10;

/// One page of comments, bound to `$owner`, `$name`, `$number` and `$after`.
pub(crate) const CONVERSATION_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $after: String) { \
    repository(owner: $owner, name: $name) { pullRequest(number: $number) { \
    comments(first: 100, after: $after) { pageInfo { hasNextPage endCursor } \
    nodes { id url author { login } body } } } } }";

/// Every page through `page` (the answer to [`CONVERSATION_QUERY`] after a
/// cursor), up to [`CONVERSATION_PAGES_MAX`]; beyond that it is marked.
pub(crate) fn read_conversation(
    mut page: impl FnMut(Option<&str>) -> Result<String, GhError>,
) -> Result<Conversation, GhError> {
    let mut comments = Vec::new();
    let mut after: Option<String> = None;
    for read in 0.. {
        if read == CONVERSATION_PAGES_MAX {
            return Ok(Conversation {
                comments,
                truncated: true,
            });
        }
        let (more, next) = parse_conversation_page(&page(after.as_deref())?)?;
        comments.extend(more);
        match next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    Ok(Conversation {
        comments,
        truncated: false,
    })
}

/// One page's comments, oldest first, and the cursor of the next page.
pub(crate) fn parse_conversation_page(
    json: &str,
) -> Result<(Vec<ReviewComment>, Option<String>), GhError> {
    let response: Response = serde_json::from_str(json).map_err(parse_error)?;
    let Some(comments) = response
        .data
        .and_then(|d| d.repository)
        .and_then(|r| r.pull_request)
        .map(|pr| pr.comments)
    else {
        return Err(GhError::Parse(format!(
            "no pull request in the answer: {}",
            api_messages(response.errors)
        )));
    };
    let next = comments
        .page_info
        .has_next_page
        .then_some(comments.page_info.end_cursor)
        .flatten();
    let read = comments
        .nodes
        .into_iter()
        .map(|node| ReviewComment {
            id: node.id,
            url: node.url,
            author: node.author.map(|a| a.login),
            body: node.body,
        })
        .collect();
    Ok((read, next))
}

#[derive(Deserialize)]
struct Response {
    data: Option<Data>,
    errors: Option<Vec<ApiError>>,
}

#[derive(Deserialize)]
struct Data {
    repository: Option<Repository>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    pull_request: Option<Pr>,
}

#[derive(Deserialize)]
struct Pr {
    comments: Comments,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Comments {
    page_info: PageInfo,
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    #[serde(default)]
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    #[serde(default)]
    url: Option<String>,
    author: Option<Author>,
    body: String,
}

#[cfg(test)]
mod tests;
