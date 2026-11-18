//! The mutations a review thread takes: a reply, and resolving or
//! unresolving it; and the parsers for their answers.

use serde::Deserialize;

use super::{Answer, Author, api_messages, parse_error};
use crate::{GhError, ReviewComment};

/// Reply `$body` to thread `$thread`.
pub(crate) const REPLY_MUTATION: &str = "mutation($thread: ID!, $body: String!) { \
    addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $thread, body: $body}) { \
    comment { id url author { login } body } } }";

/// Resolve thread `$thread`.
pub(crate) const RESOLVE_MUTATION: &str = "mutation($thread: ID!) { \
    resolveReviewThread(input: {threadId: $thread}) { thread { id isResolved } } }";

/// Unresolve thread `$thread`.
pub(crate) const UNRESOLVE_MUTATION: &str = "mutation($thread: ID!) { \
    unresolveReviewThread(input: {threadId: $thread}) { thread { id isResolved } } }";

/// The comment a reply posted.
pub(crate) fn parse_reply(json: &str) -> Result<ReviewComment, GhError> {
    let response: Answer<ReplyData> = serde_json::from_str(json).map_err(parse_error)?;
    let Some(comment) = response
        .data
        .and_then(|data| data.add_pull_request_review_thread_reply)
        .and_then(|payload| payload.comment)
    else {
        return Err(GhError::Parse(format!(
            "no reply in the answer: {}",
            api_messages(response.errors)
        )));
    };
    Ok(ReviewComment {
        id: comment.id,
        url: comment.url,
        author: comment.author.map(|a| a.login),
        body: comment.body,
    })
}

/// That the thread now stands `resolved`, as resolving or unresolving it
/// answered.
pub(crate) fn parse_resolved(json: &str, resolved: bool) -> Result<(), GhError> {
    let response: Answer<ResolveData> = serde_json::from_str(json).map_err(parse_error)?;
    let thread = response.data.and_then(|data| {
        data.resolve_review_thread
            .or(data.unresolve_review_thread)
            .and_then(|payload| payload.thread)
    });
    match thread {
        Some(thread) if thread.is_resolved == resolved => Ok(()),
        Some(_) => Err(GhError::Parse(format!(
            "the thread is still {}",
            if resolved { "unresolved" } else { "resolved" }
        ))),
        None => Err(GhError::Parse(format!(
            "no thread in the answer: {}",
            api_messages(response.errors)
        ))),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplyData {
    add_pull_request_review_thread_reply: Option<ReplyPayload>,
}

#[derive(Deserialize)]
struct ReplyPayload {
    comment: Option<CommentNode>,
}

#[derive(Deserialize)]
struct CommentNode {
    id: String,
    #[serde(default)]
    url: Option<String>,
    author: Option<Author>,
    body: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveData {
    #[serde(default)]
    resolve_review_thread: Option<ResolvePayload>,
    #[serde(default)]
    unresolve_review_thread: Option<ResolvePayload>,
}

#[derive(Deserialize)]
struct ResolvePayload {
    thread: Option<ThreadState>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadState {
    is_resolved: bool,
}

#[cfg(test)]
mod tests;
