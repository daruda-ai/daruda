//! Turning auto-merge on for a pull request, pinned to the head commit the
//! user saw, and off; and the parsers for their answers.

use serde::Deserialize;

use super::{ApiError, api_messages, parse_error};
use crate::{GhError, MergeMethod};

/// Turn auto-merge on for pull request `$pr` with `$method`, only while
/// its head is `$head`.
pub(crate) const ENABLE_AUTO_MERGE_MUTATION: &str = "mutation($pr: ID!, \
    $method: PullRequestMergeMethod!, $head: GitObjectID!) { \
    enablePullRequestAutoMerge(input: {pullRequestId: $pr, mergeMethod: $method, \
    expectedHeadOid: $head}) { pullRequest { autoMergeRequest { mergeMethod } \
    mergeQueueEntry { position } } } }";

/// Turn auto-merge off for pull request `$pr`.
pub(crate) const DISABLE_AUTO_MERGE_MUTATION: &str = "mutation($pr: ID!) { \
    disablePullRequestAutoMerge(input: {pullRequestId: $pr}) { \
    pullRequest { autoMergeRequest { mergeMethod } mergeQueueEntry { position } } } }";

/// GitHub's name for `method`.
pub(crate) fn method_name(method: MergeMethod) -> &'static str {
    match method {
        MergeMethod::Merge => "MERGE",
        MergeMethod::Squash => "SQUASH",
        MergeMethod::Rebase => "REBASE",
    }
}

/// Check the answer to either mutation: auto-merge is now `on` or not.
/// Turning it on counts as done when the pull request went into the merge
/// queue instead.
// ASSUMPTION: not measured — on a branch that requires a merge queue,
// GitHub may enqueue at once and leave `autoMergeRequest` null.
pub(crate) fn parse_auto_merge(json: &str, on: bool) -> Result<(), GhError> {
    let response: Response = serde_json::from_str(json).map_err(parse_error)?;
    let pr = response.data.and_then(|data| {
        data.enable_pull_request_auto_merge
            .or(data.disable_pull_request_auto_merge)
            .and_then(|payload| payload.pull_request)
    });
    match pr {
        Some(pr) if on && (pr.auto_merge_request.is_some() || pr.merge_queue_entry.is_some()) => {
            Ok(())
        }
        Some(pr) if !on && pr.auto_merge_request.is_none() => Ok(()),
        Some(_) => Err(GhError::Parse(format!(
            "auto-merge is still {}",
            if on { "off" } else { "on" }
        ))),
        None => Err(GhError::Parse(format!(
            "no pull request in the answer: {}",
            api_messages(response.errors)
        ))),
    }
}

#[derive(Deserialize)]
struct Response {
    data: Option<Data>,
    errors: Option<Vec<ApiError>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Data {
    #[serde(default)]
    enable_pull_request_auto_merge: Option<Payload>,
    #[serde(default)]
    disable_pull_request_auto_merge: Option<Payload>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Payload {
    pull_request: Option<PrNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrNode {
    auto_merge_request: Option<serde_json::Value>,
    #[serde(default)]
    merge_queue_entry: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests;
