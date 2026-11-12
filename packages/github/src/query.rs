//! The GraphQL request that finds the pull requests of several branches at
//! once, and the parser for its answer.
//!
//! One aliased field per branch (`b0`, `b1`, ...) keeps the request count per
//! repository, not per lane. Branch names travel as variables, never inside
//! the query text.

use std::collections::HashMap;

use serde::Deserialize;

use crate::checks::{check_run_status, rollup_from_state, status_context_status, summarize};
use crate::{
    CheckRollup, CheckRun, GhError, PrState, PrSummary, ReviewComment, ReviewDecision,
    ReviewThread, ReviewThreads,
};

/// PRs fetched per branch: enough to find an open one among old closed ones.
const PRS_PER_BRANCH: u32 = 5;
/// Checks fetched per PR; beyond this GitHub's own rollup state is used.
const CHECKS_PER_PR: u32 = 100;

/// What [`crate::Gh::prs_for_branches`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepoPrs {
    /// `None` for a repository without commits.
    pub default_branch: Option<String>,
    /// Candidates per requested branch, most recently updated first.
    pub by_branch: HashMap<String, Vec<PrSummary>>,
}

/// The query for `count` branches, bound to `$owner`, `$name`, `$b0`...
pub fn prs_for_branches_query(count: usize) -> String {
    let mut declarations = String::from("$owner: String!, $name: String!");
    let mut fields = String::new();
    for i in 0..count {
        declarations.push_str(&format!(", $b{i}: String!"));
        fields.push_str(&format!(
            " b{i}: pullRequests(headRefName: $b{i}, first: {PRS_PER_BRANCH}, \
             orderBy: {{field: UPDATED_AT, direction: DESC}}) {{ nodes {{ ...pr }} }}"
        ));
    }
    format!(
        "query({declarations}) {{ repository(owner: $owner, name: $name) {{ \
         defaultBranchRef {{ name }}{fields} }} }} \
         fragment pr on PullRequest {{ number url title state isDraft headRefName \
         isCrossRepository reviewDecision updatedAt statusCheckRollup {{ state \
         contexts(first: {CHECKS_PER_PR}) {{ pageInfo {{ hasNextPage }} nodes {{ __typename \
         ... on CheckRun {{ name status conclusion detailsUrl startedAt }} \
         ... on StatusContext {{ context state targetUrl createdAt }} }} }} }} }}"
    )
}

/// Parse the answer to [`prs_for_branches_query`] for `branches`, in the
/// order their variables were bound.
pub fn parse_prs_for_branches(json: &str, branches: &[String]) -> Result<RepoPrs, GhError> {
    let response: Response = serde_json::from_str(json).map_err(parse_error)?;
    let Some(mut repository) = response.data.and_then(|d| d.repository) else {
        let messages: Vec<String> = response
            .errors
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.message)
            .collect();
        return Err(GhError::Parse(format!(
            "no repository in the answer: {}",
            messages.join("; ")
        )));
    };
    let default_branch = repository
        .remove("defaultBranchRef")
        .and_then(|value| serde_json::from_value::<Option<BranchRef>>(value).ok())
        .flatten()
        .map(|branch| branch.name);
    let mut by_branch = HashMap::new();
    for (i, branch) in branches.iter().enumerate() {
        let field = repository
            .remove(&format!("b{i}"))
            .ok_or_else(|| GhError::Parse(format!("missing field b{i}")))?;
        let connection: Connection = serde_json::from_value(field).map_err(parse_error)?;
        let prs = connection
            .nodes
            .into_iter()
            .map(PrNode::into_summary)
            .collect::<Result<Vec<_>, _>>()?;
        by_branch.insert(branch.clone(), prs);
    }
    Ok(RepoPrs {
        default_branch,
        by_branch,
    })
}

/// Unresolved review threads of one pull request, bound to `$owner`,
/// `$name` and `$number`.
pub const REVIEW_THREADS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) { \
    repository(owner: $owner, name: $name) { pullRequest(number: $number) { \
    reviewThreads(first: 100) { pageInfo { hasNextPage } nodes { isResolved isOutdated path \
    line startLine originalLine originalStartLine \
    comments(first: 20) { nodes { author { login } body } } } } } } }";

/// Parse the answer to [`REVIEW_THREADS_QUERY`], keeping the unresolved
/// threads. An outdated thread has no current line; where it was written is
/// used instead.
pub fn parse_review_threads(json: &str) -> Result<ReviewThreads, GhError> {
    let response: ThreadsResponse = serde_json::from_str(json).map_err(parse_error)?;
    let Some(connection) = response
        .data
        .and_then(|d| d.repository)
        .and_then(|r| r.pull_request)
        .map(|pr| pr.review_threads)
    else {
        let messages: Vec<String> = response
            .errors
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.message)
            .collect();
        return Err(GhError::Parse(format!(
            "no pull request in the answer: {}",
            messages.join("; ")
        )));
    };
    let threads = connection
        .nodes
        .into_iter()
        .filter(|thread| !thread.is_resolved)
        .map(|thread| {
            let end = thread.line.or(thread.original_line);
            let start = thread.start_line.or(thread.original_start_line).or(end);
            ReviewThread {
                path: thread.path,
                lines: end.zip(start).map(|(end, start)| (start.min(end), end)),
                outdated: thread.is_outdated,
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
struct CommentConnection {
    nodes: Vec<CommentNode>,
}

#[derive(Deserialize)]
struct CommentNode {
    author: Option<Author>,
    body: String,
}

#[derive(Deserialize)]
struct Author {
    login: String,
}

fn parse_error(error: serde_json::Error) -> GhError {
    GhError::Parse(error.to_string())
}

#[derive(Deserialize)]
struct Response {
    data: Option<Data>,
    errors: Option<Vec<ApiError>>,
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
}

#[derive(Deserialize)]
struct Data {
    /// Aliased fields are dynamic, so the object stays a map.
    repository: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Deserialize)]
struct BranchRef {
    name: String,
}

#[derive(Deserialize)]
struct Connection {
    nodes: Vec<PrNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrNode {
    number: u64,
    url: String,
    title: String,
    state: String,
    is_draft: bool,
    head_ref_name: String,
    is_cross_repository: bool,
    review_decision: Option<String>,
    updated_at: String,
    status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
struct Rollup {
    state: String,
    contexts: Contexts,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Contexts {
    page_info: PageInfo,
    nodes: Vec<Context>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
}

#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum Context {
    #[serde(rename_all = "camelCase")]
    CheckRun {
        name: String,
        status: String,
        conclusion: Option<String>,
        details_url: Option<String>,
        started_at: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    StatusContext {
        context: String,
        state: String,
        target_url: Option<String>,
        created_at: Option<String>,
    },
}

impl Context {
    fn into_run(self) -> CheckRun {
        match self {
            Self::CheckRun {
                name,
                status,
                conclusion,
                details_url,
                started_at,
            } => CheckRun {
                status: check_run_status(&status, conclusion.as_deref()),
                name,
                details_url,
                started_at,
            },
            Self::StatusContext {
                context,
                state,
                target_url,
                created_at,
            } => CheckRun {
                status: status_context_status(&state),
                name: context,
                details_url: target_url,
                started_at: created_at,
            },
        }
    }
}

impl PrNode {
    fn into_summary(self) -> Result<PrSummary, GhError> {
        let state = match (self.state.as_str(), self.is_draft) {
            ("OPEN", true) => PrState::Draft,
            ("OPEN", false) => PrState::Open,
            ("MERGED", _) => PrState::Merged,
            ("CLOSED", _) => PrState::Closed,
            (other, _) => return Err(GhError::Parse(format!("PR state {other}"))),
        };
        let review = match self.review_decision.as_deref() {
            Some("APPROVED") => Some(ReviewDecision::Approved),
            Some("CHANGES_REQUESTED") => Some(ReviewDecision::ChangesRequested),
            Some("REVIEW_REQUIRED") => Some(ReviewDecision::ReviewRequired),
            _ => None,
        };
        let (checks, failed) = match self.status_check_rollup {
            None => (CheckRollup::None, Vec::new()),
            Some(rollup) => {
                let runs: Vec<CheckRun> = rollup
                    .contexts
                    .nodes
                    .into_iter()
                    .map(Context::into_run)
                    .collect();
                let (counted, failed) = summarize(&runs);
                // A truncated list cannot be added up; GitHub's verdict stands,
                // with the failures in hand as a lower bound on the count.
                let checks = if rollup.contexts.page_info.has_next_page {
                    match rollup_from_state(&rollup.state) {
                        CheckRollup::Failing { .. } => CheckRollup::Failing {
                            failed: u32::try_from(failed.len()).unwrap_or(u32::MAX),
                        },
                        other => other,
                    }
                } else {
                    counted
                };
                (checks, failed)
            }
        };
        Ok(PrSummary {
            number: self.number,
            url: self.url,
            title: self.title,
            state,
            head_branch: self.head_ref_name,
            cross_repo: self.is_cross_repository,
            checks,
            failed,
            review,
            updated_at: self.updated_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_binds_one_variable_per_branch() {
        let query = prs_for_branches_query(2);
        assert!(query.contains("$b0: String!, $b1: String!"));
        assert!(query.contains("b1: pullRequests(headRefName: $b1"));
        assert!(!query.contains("$b2"));
    }
}
