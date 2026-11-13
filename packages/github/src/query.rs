//! The GraphQL request that finds the pull requests of several branches at
//! once, the search that lists open pull requests across repositories, and
//! the parsers for their answers.
//!
//! One aliased field per branch (`b0`, `b1`, ...) keeps the request count per
//! repository, not per lane. Branch names travel as variables, never inside
//! the query text.

use std::collections::HashMap;

use serde::Deserialize;

use crate::checks::{check_run_status, rollup_from_state, status_context_status, summarize};
use crate::{
    CheckKind, CheckRollup, CheckRun, GhError, PrFilter, PrListItem, PrState, PrSummary, RepoId,
    ReviewComment, ReviewDecision, ReviewThread, ReviewThreads,
};

/// PRs fetched per branch: enough to find an open one among old closed ones.
/// Fork PRs with the same head name count against it; `ref.associatedPullRequests`
/// would exclude them but loses a merged PR once its branch is deleted
/// (the ref is null then — measured on `cli/cli`).
const PRS_PER_BRANCH: u32 = 5;
/// Checks fetched per PR; beyond this GitHub's own rollup state is used.
const CHECKS_PER_PR: u32 = 100;
/// Pull requests one search answers; more are left out of the list.
const SEARCH_RESULTS: u32 = 50;

/// What [`crate::Gh::prs_for_branches`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepoPrs {
    /// `None` for a repository without commits.
    pub default_branch: Option<String>,
    /// Candidates per requested branch, most recently updated first.
    pub by_branch: HashMap<String, Vec<PrSummary>>,
}

/// The query for `count` branches, bound to `$owner`, `$name`, `$b0`...
pub(crate) fn prs_for_branches_query(count: usize) -> String {
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
         defaultBranchRef {{ name }}{fields} }} }} {}",
        pr_fragment()
    )
}

/// The PR fields [`PrNode`] reads, as fragment `pr`.
fn pr_fragment() -> String {
    format!(
        "fragment pr on PullRequest {{ number url title state isDraft headRefName \
         isCrossRepository reviewDecision updatedAt statusCheckRollup {{ state \
         contexts(first: {CHECKS_PER_PR}) {{ pageInfo {{ hasNextPage }} nodes {{ __typename \
         ... on CheckRun {{ name status conclusion detailsUrl startedAt \
         checkSuite {{ workflowRun {{ event workflow {{ name }} }} }} }} \
         ... on StatusContext {{ context state targetUrl createdAt }} }} }} }} }}"
    )
}

/// Parse the answer to [`prs_for_branches_query`] for `branches`, in the
/// order their variables were bound.
pub(crate) fn parse_prs_for_branches(json: &str, branches: &[String]) -> Result<RepoPrs, GhError> {
    let response: Response = serde_json::from_str(json).map_err(parse_error)?;
    let Some(mut repository) = response.data.and_then(|d| d.repository) else {
        return Err(GhError::Parse(format!(
            "no repository in the answer: {}",
            api_messages(response.errors)
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

/// The search text for open pull requests of `repos` matching `filter`.
/// Archived repositories take no new work, so their PRs are left out.
pub(crate) fn search_string(repos: &[RepoId], filter: PrFilter) -> String {
    let mut text = String::from("is:pr is:open archived:false");
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
         nodes {{ ... on PullRequest {{ ...pr repository {{ nameWithOwner }} \
         author {{ login }} baseRefName comments {{ totalCount }} }} }} }} }} {}",
        pr_fragment()
    )
}

/// Parse the answer to [`search_prs_query`] asked of `host`.
pub(crate) fn parse_search_prs(json: &str, host: &str) -> Result<Vec<PrListItem>, GhError> {
    let response: SearchResponse = serde_json::from_str(json).map_err(parse_error)?;
    let Some(search) = response.data.and_then(|d| d.search) else {
        return Err(GhError::Parse(format!(
            "no search in the answer: {}",
            api_messages(response.errors)
        )));
    };
    search
        .nodes
        .into_iter()
        .map(|node| {
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
                author: node.author.map(|a| a.login),
                base_branch: node.base_ref_name,
                comments: node.comments.total_count,
                pr: node.pr.into_summary()?,
            })
        })
        .collect()
}

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

#[derive(Deserialize)]
struct Author {
    login: String,
}

fn parse_error(error: serde_json::Error) -> GhError {
    GhError::Parse(error.to_string())
}

fn api_messages(errors: Option<Vec<ApiError>>) -> String {
    let messages: Vec<String> = errors
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.message)
        .collect();
    messages.join("; ")
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
struct SearchConnection {
    nodes: Vec<SearchNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchNode {
    #[serde(flatten)]
    pr: PrNode,
    repository: NameWithOwner,
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
        #[serde(default)]
        check_suite: Option<CheckSuite>,
    },
    #[serde(rename_all = "camelCase")]
    StatusContext {
        context: String,
        state: String,
        target_url: Option<String>,
        created_at: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckSuite {
    workflow_run: Option<WorkflowRun>,
}

#[derive(Deserialize)]
struct WorkflowRun {
    #[serde(default)]
    event: Option<String>,
    workflow: Workflow,
}

#[derive(Deserialize)]
struct Workflow {
    name: String,
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
                check_suite,
            } => {
                let run = check_suite.and_then(|suite| suite.workflow_run);
                let (workflow, event) = match run {
                    Some(run) => (Some(run.workflow.name), run.event),
                    None => (None, None),
                };
                CheckRun {
                    kind: CheckKind::Run,
                    workflow,
                    event,
                    status: check_run_status(&status, conclusion.as_deref()),
                    name,
                    details_url,
                    started_at,
                }
            }
            Self::StatusContext {
                context,
                state,
                target_url,
                created_at,
            } => CheckRun {
                kind: CheckKind::Status,
                workflow: None,
                event: None,
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
                // without a count, and the failures in hand are still sent.
                let checks = if rollup.contexts.page_info.has_next_page {
                    rollup_from_state(&rollup.state)
                } else {
                    counted
                };
                // `failed` lists what is behind `Failing`, nothing else.
                let failed = match checks {
                    CheckRollup::Failing { .. } => failed,
                    _ => Vec::new(),
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
mod tests;
