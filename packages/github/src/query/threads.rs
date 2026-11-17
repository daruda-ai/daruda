//! The requests for one pull request's unresolved review threads, page by
//! page with each thread's later replies, and the parsers for their answers.

use serde::Deserialize;

use super::{ApiError, Author, api_messages, parse_error};
use crate::{DiffSide, GhError, ReviewComment, ReviewThread, ReviewThreads};

/// Threads per page, GitHub's largest.
const THREADS_PER_PAGE: u32 = 100;
/// Replies each thread brings with its page; later ones are read apart.
const REPLIES_WITH_THREAD: u32 = 20;
/// Pages of threads read for one pull request: a thousand threads.
pub(crate) const THREAD_PAGES_MAX: usize = 10;
/// Pages of replies read for one thread past those it came with.
pub(crate) const REPLY_PAGES_MAX: usize = 10;
/// Reply pages read for one pull request in all: each is a `gh` run.
pub(crate) const REPLY_REQUESTS_MAX: usize = 50;

const COMMENT_FIELDS: &str = "id url author { login } body";

/// One page of review threads, bound to `$owner`, `$name`, `$number` and
/// `$after` (null for the first page).
pub(crate) fn review_threads_query() -> String {
    format!(
        "query($owner: String!, $name: String!, $number: Int!, $after: String) {{ \
         repository(owner: $owner, name: $name) {{ pullRequest(number: $number) {{ \
         reviewThreads(first: {THREADS_PER_PAGE}, after: $after) {{ pageInfo {{ hasNextPage endCursor }} \
         nodes {{ id isResolved isOutdated path line startLine originalLine originalStartLine \
         diffSide startDiffSide subjectType viewerCanReply viewerCanResolve viewerCanUnresolve \
         comments(first: {REPLIES_WITH_THREAD}) {{ totalCount pageInfo {{ hasNextPage endCursor }} \
         nodes {{ {COMMENT_FIELDS} }} }} }} }} }} }} }}"
    )
}

/// One page of a thread's replies, bound to `$id` (the thread) and `$after`.
pub(crate) fn review_replies_query() -> String {
    format!(
        "query($id: ID!, $after: String) {{ node(id: $id) {{ ... on PullRequestReviewThread {{ \
         comments(first: 100, after: $after) {{ pageInfo {{ hasNextPage endCursor }} \
         nodes {{ {COMMENT_FIELDS} }} }} }} }} }}"
    )
}

/// The unresolved threads of a pull request, read through `threads_page`
/// (the answer to [`review_threads_query`] after a cursor) and
/// `replies_page` (the answer to [`review_replies_query`] for a thread
/// after a cursor). Reading stops at [`THREAD_PAGES_MAX`], and replies at
/// [`REPLY_PAGES_MAX`] per thread and [`REPLY_REQUESTS_MAX`] in all; what is
/// left is marked, never dropped silently. A thread whose later replies
/// fail to read keeps those it has, marked: the other threads still land.
pub(crate) fn read_review_threads(
    mut threads_page: impl FnMut(Option<&str>) -> Result<String, GhError>,
    mut replies_page: impl FnMut(&str, &str) -> Result<String, GhError>,
) -> Result<ReviewThreads, GhError> {
    let mut threads = Vec::new();
    let mut after: Option<String> = None;
    let mut truncated = false;
    for read in 0.. {
        if read == THREAD_PAGES_MAX {
            truncated = true;
            break;
        }
        let page = parse_threads_page(&threads_page(after.as_deref())?)?;
        threads.extend(page.threads);
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    let mut read = Vec::with_capacity(threads.len());
    let mut requests = 0;
    for (mut thread, mut cursor) in threads {
        let mut pages = 0;
        while let Some(after) = cursor.take() {
            if pages == REPLY_PAGES_MAX || requests == REPLY_REQUESTS_MAX || thread.id.is_empty() {
                thread.more_comments = true;
                break;
            }
            pages += 1;
            requests += 1;
            match replies_page(&thread.id, &after).and_then(|json| parse_replies_page(&json)) {
                Ok((comments, next)) => {
                    thread.comments.extend(comments);
                    cursor = next;
                }
                Err(_) => thread.more_comments = true,
            }
        }
        read.push(thread);
    }
    Ok(ReviewThreads {
        threads: read,
        truncated,
    })
}

/// One page's unresolved threads, each with the cursor of its later
/// replies, and the cursor of the next page.
#[derive(Debug)]
pub(crate) struct ThreadsPage {
    pub threads: Vec<(ReviewThread, Option<String>)>,
    pub next: Option<String>,
}

/// Parse an answer to [`review_threads_query`], keeping the unresolved
/// threads. An outdated thread has no current line; where it was written is
/// used instead.
pub(crate) fn parse_threads_page(json: &str) -> Result<ThreadsPage, GhError> {
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
            let file_level = thread.subject_type.as_deref() == Some("FILE");
            let end = thread.line.or(thread.original_line);
            // A range that starts on the other side counts its first line
            // there; only its last line is on `diffSide`.
            let one_side =
                thread.start_diff_side.is_none() || thread.start_diff_side == thread.diff_side;
            let start = thread
                .start_line
                .or(thread.original_start_line)
                .filter(|_| one_side)
                .or(end);
            let replies = thread
                .comments
                .page_info
                .as_ref()
                .filter(|page| page.has_next_page)
                .and_then(|page| page.end_cursor.clone());
            let more_comments = replies.is_none()
                && thread
                    .comments
                    .total_count
                    .is_some_and(|total| total as usize > thread.comments.nodes.len());
            let read = ReviewThread {
                id: thread.id.unwrap_or_default(),
                path: thread.path,
                lines: end
                    .zip(start)
                    .filter(|_| !file_level)
                    .map(|(end, start)| (start.min(end), end)),
                side: match thread.diff_side.as_deref() {
                    Some("LEFT") => DiffSide::Left,
                    _ => DiffSide::Right,
                },
                outdated: thread.is_outdated,
                more_comments,
                comments: thread.comments.nodes.into_iter().map(comment).collect(),
                may: crate::ThreadPermissions {
                    reply: thread.viewer_can_reply.unwrap_or(true),
                    resolve: thread.viewer_can_resolve.unwrap_or(true),
                    unresolve: thread.viewer_can_unresolve.unwrap_or(true),
                },
            };
            (read, replies)
        })
        .collect();
    Ok(ThreadsPage {
        threads,
        next: connection
            .page_info
            .has_next_page
            .then_some(connection.page_info.end_cursor)
            .flatten(),
    })
}

/// Parse an answer to [`review_replies_query`]: the replies, and the cursor
/// of the next page.
pub(crate) fn parse_replies_page(
    json: &str,
) -> Result<(Vec<ReviewComment>, Option<String>), GhError> {
    let response: RepliesResponse = serde_json::from_str(json).map_err(parse_error)?;
    let Some(comments) = response
        .data
        .and_then(|d| d.node)
        .and_then(|node| node.comments)
    else {
        return Err(GhError::Parse(format!(
            "no review thread in the answer: {}",
            api_messages(response.errors)
        )));
    };
    let next = comments
        .page_info
        .as_ref()
        .filter(|page| page.has_next_page)
        .and_then(|page| page.end_cursor.clone());
    Ok((comments.nodes.into_iter().map(comment).collect(), next))
}

fn comment(node: CommentNode) -> ReviewComment {
    ReviewComment {
        id: node.id.unwrap_or_default(),
        url: node.url,
        author: node.author.map(|a| a.login),
        body: node.body,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    #[serde(default)]
    end_cursor: Option<String>,
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
    #[serde(default)]
    id: Option<String>,
    is_resolved: bool,
    is_outdated: bool,
    path: String,
    line: Option<u32>,
    start_line: Option<u32>,
    original_line: Option<u32>,
    original_start_line: Option<u32>,
    #[serde(default)]
    diff_side: Option<String>,
    #[serde(default)]
    start_diff_side: Option<String>,
    #[serde(default)]
    subject_type: Option<String>,
    #[serde(default)]
    viewer_can_reply: Option<bool>,
    #[serde(default)]
    viewer_can_resolve: Option<bool>,
    #[serde(default)]
    viewer_can_unresolve: Option<bool>,
    comments: CommentConnection,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentConnection {
    #[serde(default)]
    total_count: Option<u32>,
    #[serde(default)]
    page_info: Option<PageInfo>,
    nodes: Vec<CommentNode>,
}

#[derive(Deserialize)]
struct CommentNode {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    url: Option<String>,
    author: Option<Author>,
    body: String,
}

#[derive(Deserialize)]
struct RepliesResponse {
    data: Option<RepliesData>,
    errors: Option<Vec<ApiError>>,
}

#[derive(Deserialize)]
struct RepliesData {
    node: Option<RepliesNode>,
}

#[derive(Deserialize)]
struct RepliesNode {
    #[serde(default)]
    comments: Option<CommentConnection>,
}

#[cfg(test)]
mod tests;
