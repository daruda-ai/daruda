//! Submitting a review — its verdict, text and comments on lines, at one
//! head commit — and reading the viewer's latest review, which tells
//! whether a submission whose answer was lost went through.

use serde::Deserialize;

use super::{ApiError, api_messages, parse_error};
use crate::{
    DiffSide, GhError, LatestReview, LatestReviewState, ReviewEvent, ReviewThreadDraft,
    SubmittedReview,
};

/// Submit a review on pull request `$pr` at commit `$commit`.
pub(crate) const SUBMIT_REVIEW_MUTATION: &str = "mutation($pr: ID!, $commit: GitObjectID!, \
    $event: PullRequestReviewEvent!, $body: String, $threads: [DraftPullRequestReviewThread]) { \
    addPullRequestReview(input: {pullRequestId: $pr, commitOID: $commit, event: $event, \
    body: $body, threads: $threads}) { pullRequestReview { id url state } } }";

/// The viewer's latest review on pull request `$number`.
pub(crate) const LATEST_REVIEW_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) { \
    repository(owner: $owner, name: $name) { pullRequest(number: $number) { \
    viewerLatestReview { id state submittedAt body } } } }";

/// The request [`SUBMIT_REVIEW_MUTATION`] is sent as: query and variables
/// in one JSON body, so long text never rides on the command line.
pub(crate) fn submit_request(
    pr: &str,
    commit: &str,
    event: ReviewEvent,
    body: &str,
    threads: &[ReviewThreadDraft],
) -> serde_json::Value {
    let side = |side: DiffSide| match side {
        DiffSide::Left => "LEFT",
        DiffSide::Right => "RIGHT",
    };
    let threads: Vec<serde_json::Value> = threads
        .iter()
        .map(|thread| {
            let mut value = serde_json::json!({
                "path": thread.path,
                "line": thread.line,
                "side": side(thread.side),
                "body": thread.body,
            });
            if let Some(start) = thread.start_line.filter(|start| *start < thread.line) {
                value["startLine"] = start.into();
                value["startSide"] = side(thread.side).into();
            }
            value
        })
        .collect();
    serde_json::json!({
        "query": SUBMIT_REVIEW_MUTATION,
        "variables": {
            "pr": pr,
            "commit": commit,
            "event": match event {
                ReviewEvent::Comment => "COMMENT",
                ReviewEvent::Approve => "APPROVE",
                ReviewEvent::RequestChanges => "REQUEST_CHANGES",
            },
            "body": body,
            "threads": threads,
        },
    })
}

pub(crate) fn parse_submitted(json: &str) -> Result<SubmittedReview, GhError> {
    let response: Response<SubmitData> = serde_json::from_str(json).map_err(parse_error)?;
    let Some(review) = response
        .data
        .and_then(|data| data.add_pull_request_review)
        .and_then(|payload| payload.pull_request_review)
    else {
        return Err(GhError::Parse(format!(
            "no review in the answer: {}",
            api_messages(response.errors)
        )));
    };
    Ok(SubmittedReview {
        id: review.id,
        url: review.url,
    })
}

pub(crate) fn parse_latest_review(json: &str) -> Result<Option<LatestReview>, GhError> {
    let response: Response<LatestData> = serde_json::from_str(json).map_err(parse_error)?;
    let Some(pr) = response
        .data
        .and_then(|data| data.repository)
        .and_then(|repository| repository.pull_request)
    else {
        return Err(GhError::Parse(format!(
            "no pull request in the answer: {}",
            api_messages(response.errors)
        )));
    };
    Ok(pr.viewer_latest_review.map(|review| LatestReview {
        id: review.id,
        body: review.body,
        state: match review.state.as_str() {
            "PENDING" => LatestReviewState::Pending,
            "COMMENTED" => LatestReviewState::Given(ReviewEvent::Comment),
            "APPROVED" => LatestReviewState::Given(ReviewEvent::Approve),
            "CHANGES_REQUESTED" => LatestReviewState::Given(ReviewEvent::RequestChanges),
            _ => LatestReviewState::Other,
        },
    }))
}

#[derive(Deserialize)]
struct Response<T> {
    data: Option<T>,
    errors: Option<Vec<ApiError>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitData {
    add_pull_request_review: Option<SubmitPayload>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitPayload {
    pull_request_review: Option<ReviewNode>,
}

#[derive(Deserialize)]
struct ReviewNode {
    id: String,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Deserialize)]
struct LatestData {
    repository: Option<LatestRepository>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LatestRepository {
    pull_request: Option<LatestPr>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LatestPr {
    viewer_latest_review: Option<LatestNode>,
}

#[derive(Deserialize)]
struct LatestNode {
    id: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    state: String,
}

#[cfg(test)]
mod tests;
