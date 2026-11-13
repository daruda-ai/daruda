//! The request for one pull request's detail and the parser for its answer.

use serde::Deserialize;

use super::{ApiError, Author, PrNode, api_messages, parse_error, pr_fragment};
use crate::checks::entries;
use crate::{BlockReason, GhError, MergeMethod, Mergeability, PrDetail, ReviewerState};

/// Reviews, review requests and review threads one detail request reads.
const DETAIL_PAGE: u32 = 100;

/// One pull request's detail, bound to `$owner`, `$name` and `$number`.
/// A user's latest review stands for them; a request made after it means
/// they are asked again.
pub(crate) fn pr_detail_query() -> String {
    format!(
        "query($owner: String!, $name: String!, $number: Int!) {{ \
         repository(owner: $owner, name: $name) {{ mergeCommitAllowed squashMergeAllowed \
         rebaseMergeAllowed viewerDefaultMergeMethod pullRequest(number: $number) {{ ...pr body headRefOid baseRefName \
         author {{ login }} mergeable mergeStateStatus \
         latestReviews(first: {DETAIL_PAGE}) {{ nodes {{ author {{ login }} state }} }} \
         reviewRequests(first: {DETAIL_PAGE}) {{ nodes {{ requestedReviewer {{ __typename \
         ... on User {{ login }} ... on Bot {{ login }} ... on Mannequin {{ login }} \
         ... on Team {{ combinedSlug }} }} }} }} \
         reviewThreads(first: {DETAIL_PAGE}) {{ nodes {{ isResolved }} }} }} }} }} {}",
        pr_fragment()
    )
}

/// Parse the answer to [`pr_detail_query`].
pub(crate) fn parse_pr_detail(json: &str) -> Result<PrDetail, GhError> {
    let response: DetailResponse = serde_json::from_str(json).map_err(parse_error)?;
    let Some((repository, pr)) = response
        .data
        .and_then(|d| d.repository)
        .and_then(|mut r| r.pull_request.take().map(|pr| (r, pr)))
    else {
        return Err(GhError::Parse(format!(
            "no pull request in the answer: {}",
            api_messages(response.errors)
        )));
    };
    let methods = [
        (repository.merge_commit_allowed, MergeMethod::Merge),
        (repository.squash_merge_allowed, MergeMethod::Squash),
        (repository.rebase_merge_allowed, MergeMethod::Rebase),
    ]
    .into_iter()
    .filter_map(|(allowed, method)| allowed.then_some(method))
    .collect();
    let default_method = repository
        .viewer_default_merge_method
        .as_deref()
        .and_then(merge_method);
    let mergeability = mergeability(&pr.mergeable, &pr.merge_state_status);
    let reviewers = reviewers(pr.latest_reviews.nodes, pr.review_requests.nodes);
    let open_threads = pr
        .review_threads
        .nodes
        .iter()
        .filter(|thread| !thread.is_resolved)
        .count() as u32;
    let (summary, runs) = pr.pr.into_parts()?;
    Ok(PrDetail {
        summary,
        body: pr.body,
        head_sha: pr.head_ref_oid,
        base_branch: pr.base_ref_name,
        author: pr.author.map(|a| a.login),
        mergeability,
        methods,
        default_method,
        reviewers,
        checks: entries(&runs),
        open_threads,
    })
}

/// GitHub's `mergeable` and `mergeStateStatus`. `mergeable` alone knows a
/// conflict for certain; the state says why a conflict-free PR waits.
/// UNSTABLE (a check that is not required fails) and HAS_HOOKS still merge.
fn mergeability(mergeable: &str, state: &str) -> Mergeability {
    if mergeable == "CONFLICTING" {
        return Mergeability::Conflicting;
    }
    match state {
        "CLEAN" | "UNSTABLE" | "HAS_HOOKS" => Mergeability::Ready,
        "BLOCKED" => Mergeability::Blocked(BlockReason::Requirements),
        "BEHIND" => Mergeability::Blocked(BlockReason::Behind),
        "DRAFT" => Mergeability::Blocked(BlockReason::Draft),
        "DIRTY" => Mergeability::Conflicting,
        _ => Mergeability::Unknown,
    }
}

/// GitHub's `PullRequestMergeMethod`; `None` for a value this crate does
/// not know.
fn merge_method(value: &str) -> Option<MergeMethod> {
    match value {
        "MERGE" => Some(MergeMethod::Merge),
        "SQUASH" => Some(MergeMethod::Squash),
        "REBASE" => Some(MergeMethod::Rebase),
        _ => None,
    }
}

/// GitHub's review `state`.
fn reviewer_state(state: &str) -> ReviewerState {
    match state {
        "APPROVED" => ReviewerState::Approved,
        "CHANGES_REQUESTED" => ReviewerState::ChangesRequested,
        "COMMENTED" => ReviewerState::Commented,
        "DISMISSED" => ReviewerState::Dismissed,
        "PENDING" => ReviewerState::Pending,
        _ => ReviewerState::Other,
    }
}

/// Reviewers in GitHub's review order, then those only asked to. A request
/// outstanding for someone who reviewed means they were asked again since.
fn reviewers(reviews: Vec<ReviewNode>, requests: Vec<RequestNode>) -> Vec<(String, ReviewerState)> {
    let requested: Vec<String> = requests
        .into_iter()
        .filter_map(|request| request.requested_reviewer.and_then(Reviewer::into_name))
        .collect();
    let mut reviewers: Vec<(String, ReviewerState)> = reviews
        .into_iter()
        .filter_map(|review| {
            let login = review.author?.login;
            let state = if requested.contains(&login) {
                ReviewerState::Requested
            } else {
                reviewer_state(&review.state)
            };
            Some((login, state))
        })
        .collect();
    for name in requested {
        if !reviewers.iter().any(|(login, _)| *login == name) {
            reviewers.push((name, ReviewerState::Requested));
        }
    }
    reviewers
}

#[derive(Deserialize)]
struct DetailResponse {
    data: Option<DetailData>,
    errors: Option<Vec<ApiError>>,
}

#[derive(Deserialize)]
struct DetailData {
    repository: Option<DetailRepository>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailRepository {
    merge_commit_allowed: bool,
    squash_merge_allowed: bool,
    rebase_merge_allowed: bool,
    viewer_default_merge_method: Option<String>,
    pull_request: Option<DetailPr>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailPr {
    #[serde(flatten)]
    pr: PrNode,
    body: String,
    head_ref_oid: String,
    base_ref_name: String,
    author: Option<Author>,
    mergeable: String,
    merge_state_status: String,
    latest_reviews: Nodes<ReviewNode>,
    review_requests: Nodes<RequestNode>,
    review_threads: Nodes<ResolvedNode>,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
struct ReviewNode {
    author: Option<Author>,
    state: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestNode {
    requested_reviewer: Option<Reviewer>,
}

/// A requested reviewer. A type this crate does not know is left out
/// rather than failing the whole detail.
#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum Reviewer {
    User {
        login: String,
    },
    Bot {
        login: String,
    },
    Mannequin {
        login: String,
    },
    #[serde(rename_all = "camelCase")]
    Team {
        combined_slug: String,
    },
    #[serde(other)]
    Other,
}

impl Reviewer {
    fn into_name(self) -> Option<String> {
        match self {
            Self::User { login } | Self::Bot { login } | Self::Mannequin { login } => Some(login),
            Self::Team { combined_slug } => Some(combined_slug),
            Self::Other => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedNode {
    is_resolved: bool,
}

#[cfg(test)]
mod tests;
