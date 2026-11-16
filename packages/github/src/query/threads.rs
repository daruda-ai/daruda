//! The request for one pull request's unresolved review threads and the
//! parser for its answer.

use serde::Deserialize;

use super::{ApiError, Author, PageInfo, api_messages, parse_error};
use crate::{GhError, ReviewComment, ReviewThread, ReviewThreads};

/// Unresolved review threads of one pull request, bound to `$owner`,
/// `$name` and `$number`.
pub(crate) const REVIEW_THREADS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) { \
    repository(owner: $owner, name: $name) { pullRequest(number: $number) { \
    reviewThreads(first: 100) { pageInfo { hasNextPage } nodes { isResolved isOutdated path \
    line startLine originalLine originalStartLine \
    comments(first: 20) { totalCount nodes { author { login } body } } } } } } }";

/// Parse the answer to [`REVIEW_THREADS_QUERY`], keeping the unresolved
/// threads. An outdated thread has no current line; where it was written is
/// used instead.
pub(crate) fn parse_review_threads(json: &str) -> Result<ReviewThreads, GhError> {
    let response: ThreadsResponse = serde_json::from_str(json).map_err(parse_error)?;
    let Some(connection) = response
        .data
        .and_then(|d| d.repository)
        .and_then(|r| r.pull_request)
        .map(|pr| pr.review_threads)
    else {
        return Err(GhError::Parse(format!(
            "no pull request in the answer: {}",
            api_messages(response.errors)
        )));
    };
    let threads = connection
        .nodes
        .into_iter()
        .filter(|thread| !thread.is_resolved)
        .map(|thread| {
            let end = thread.line.or(thread.original_line);
            let start = thread.start_line.or(thread.original_start_line).or(end);
            let more_comments = thread
                .comments
                .total_count
                .is_some_and(|total| total as usize > thread.comments.nodes.len());
            ReviewThread {
                path: thread.path,
                lines: end.zip(start).map(|(end, start)| (start.min(end), end)),
                outdated: thread.is_outdated,
                more_comments,
                comments: thread
                    .comments
                    .nodes
                    .into_iter()
                    .map(|c| ReviewComment {
                        author: c.author.map(|a| a.login),
                        body: c.body,
                    })
                    .collect(),
            }
        })
        .collect();
    Ok(ReviewThreads {
        threads,
        truncated: connection.page_info.has_next_page,
    })
}

#[derive(Deserialize)]
struct ThreadsResponse {
    data: Option<ThreadsData>,
    errors: Option<Vec<ApiError>>,
}

#[derive(Deserialize)]
struct ThreadsData {
    repository: Option<ThreadsRepository>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadsRepository {
    pull_request: Option<ThreadsPr>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadsPr {
    review_threads: ThreadConnection,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadConnection {
    page_info: PageInfo,
    nodes: Vec<ThreadNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadNode {
    is_resolved: bool,
    is_outdated: bool,
    path: String,
    line: Option<u32>,
    start_line: Option<u32>,
    original_line: Option<u32>,
    original_start_line: Option<u32>,
    comments: CommentConnection,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentConnection {
    #[serde(default)]
    total_count: Option<u32>,
    nodes: Vec<CommentNode>,
}

#[derive(Deserialize)]
struct CommentNode {
    author: Option<Author>,
    body: String,
}

#[cfg(test)]
mod tests;
