//! The GraphQL request that finds the pull requests of several branches at
//! once and the parser for its answer, plus the pull request fragment and
//! node the other requests share: `search` holds the search query's shape and
//! node parser that `paged_search` pages with cursors, `detail` reads one,
//! `threads` reads its review threads.
//!
//! One aliased field per branch (`b0`, `b1`, ...) and per pull request number
//! (`n0`, ...) keeps the request count per repository, not per lane. Branch
//! names and numbers travel as variables, never inside the query text.

use std::collections::HashMap;

use serde::Deserialize;

use crate::checks::{check_run_status, rollup_from_state, status_context_status, summarize};
use crate::{CheckKind, CheckRollup, CheckRun, GhError, PrState, PrSummary, ReviewDecision};

mod auto_merge;
mod conversation;
mod detail;
mod paged_search;
mod rate_limit;
mod review;
mod search;
mod stack;
mod thread_actions;
mod threads;

pub(crate) use auto_merge::{
    DISABLE_AUTO_MERGE_MUTATION, ENABLE_AUTO_MERGE_MUTATION, method_name, parse_auto_merge,
};
pub(crate) use conversation::{CONVERSATION_QUERY, read_conversation};
pub(crate) use detail::{parse_pr_detail, pr_detail_query};
pub(crate) use paged_search::{paged_search_args, paged_search_string, parse_search_page};
pub(crate) use rate_limit::parse_graphql_reset;
pub(crate) use review::{
    LATEST_REVIEW_QUERY, parse_latest_review, parse_submitted, submit_request,
};
pub use stack::{PrStack, StackEntry, StackRead};
pub(crate) use stack::{lacks_stacks, parse_stack, stack_query};
pub(crate) use thread_actions::{
    REPLY_MUTATION, RESOLVE_MUTATION, UNRESOLVE_MUTATION, parse_reply, parse_resolved,
};
pub(crate) use threads::{read_review_threads, review_replies_query, review_threads_query};

/// PRs fetched per branch: enough to find an open one among old closed ones.
/// Fork PRs with the same head name count against it; `ref.associatedPullRequests`
/// would exclude them but loses a merged PR once its branch is deleted
/// (the ref is null then — measured on `cli/cli`).
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
    /// Each requested pull request number, with `None` for one the
    /// repository has no pull request under.
    pub by_number: HashMap<u64, Option<PrSummary>>,
}

/// The query for `count` branches and `numbers` pull request numbers, bound
/// to `$owner`, `$name`, `$b0`... and `$n0`...
pub(crate) fn prs_for_branches_query(count: usize, numbers: usize) -> String {
    let mut declarations = String::from("$owner: String!, $name: String!");
    let mut fields = String::new();
    for i in 0..numbers {
        declarations.push_str(&format!(", $n{i}: Int!"));
        fields.push_str(&format!(" n{i}: pullRequest(number: $n{i}) {{ ...pr }}"));
    }
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
         ... on CheckRun {{ name status conclusion detailsUrl startedAt completedAt \
         checkSuite {{ workflowRun {{ event workflow {{ name }} }} }} }} \
         ... on StatusContext {{ context state targetUrl createdAt }} }} }} }} }}"
    )
}

/// Parse the answer to [`prs_for_branches_query`] for `branches` and
/// `numbers`, in the order their variables were bound.
pub(crate) fn parse_prs_for_branches(
    json: &str,
    branches: &[String],
    numbers: &[u64],
) -> Result<RepoPrs, GhError> {
    let response: Response = serde_json::from_str(json).map_err(parse_error)?;
    let errors = response.errors.unwrap_or_default();
    if !errors.iter().all(ApiError::is_number_not_found) {
        return Err(GhError::Parse(format!(
            "GitHub answered with errors: {}",
            api_messages(Some(errors))
        )));
    }
    let Some(mut repository) = response.data.and_then(|d| d.repository) else {
        return Err(GhError::Parse(format!(
            "no repository in the answer: {}",
            api_messages(Some(errors))
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
    let mut by_number = HashMap::new();
    for (i, number) in numbers.iter().enumerate() {
        let field = repository
            .remove(&format!("n{i}"))
            .ok_or_else(|| GhError::Parse(format!("missing field n{i}")))?;
        let node: Option<PrNode> = serde_json::from_value(field).map_err(parse_error)?;
        by_number.insert(*number, node.map(PrNode::into_summary).transpose()?);
    }
    Ok(RepoPrs {
        default_branch,
        by_branch,
        by_number,
    })
}

/// Whether `json` is an answer to [`prs_for_branches_query`] whose every
/// error is a number the repository has no pull request under. GitHub sends
/// such an answer with the rest of its data, and `gh` prints it but exits
/// non-zero, as for any answer carrying errors.
pub(crate) fn only_numbers_not_found(json: &str) -> bool {
    serde_json::from_str::<Response>(json).is_ok_and(|response| {
        response.errors.is_some_and(|errors| {
            !errors.is_empty() && errors.iter().all(ApiError::is_number_not_found)
        })
    })
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
struct Response {
    data: Option<Data>,
    errors: Option<Vec<ApiError>>,
}

/// A GraphQL answer: what it found, and any errors GitHub reported.
#[derive(Deserialize)]
struct Answer<T> {
    data: Option<T>,
    errors: Option<Vec<ApiError>>,
}

/// `repository { pullRequest { .. } }`: the shape a question about one
/// pull request is answered in.
#[derive(Deserialize)]
struct RepoPr<T> {
    repository: Option<PrOf<T>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrOf<T> {
    pull_request: Option<T>,
}

/// The pull request `json` answers about, or GitHub's errors saying why
/// there is none.
fn pull_request<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, GhError> {
    let answer: Answer<RepoPr<T>> = serde_json::from_str(json).map_err(parse_error)?;
    answer
        .data
        .and_then(|data| data.repository)
        .and_then(|repository| repository.pull_request)
        .ok_or_else(|| {
            GhError::Parse(format!(
                "no pull request in the answer: {}",
                api_messages(answer.errors)
            ))
        })
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    /// Where in the answer the error lies: field names, or list indexes.
    #[serde(default)]
    path: Vec<serde_json::Value>,
}

impl ApiError {
    /// `pullRequest(number:)` under alias `n<i>` found nothing; that field
    /// is null and the rest of the answer stands.
    fn is_number_not_found(&self) -> bool {
        let alias = match self.path.as_slice() {
            [repository, alias] if repository == "repository" => alias.as_str(),
            _ => None,
        };
        self.kind.as_deref() == Some("NOT_FOUND")
            && alias
                .and_then(|alias| alias.strip_prefix('n'))
                .is_some_and(|index| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()))
    }
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
        completed_at: Option<String>,
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
                completed_at,
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
                    completed_at,
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
                completed_at: None,
            },
        }
    }
}

/// GitHub's pull request `state` and draft flag as one state; `None` for a
/// state this crate does not know.
fn pr_state(state: &str, draft: bool) -> Option<PrState> {
    match (state, draft) {
        ("OPEN", true) => Some(PrState::Draft),
        ("OPEN", false) => Some(PrState::Open),
        ("MERGED", _) => Some(PrState::Merged),
        ("CLOSED", _) => Some(PrState::Closed),
        _ => None,
    }
}

impl PrNode {
    fn into_summary(self) -> Result<PrSummary, GhError> {
        self.into_parts().map(|(summary, _)| summary)
    }

    /// The summary and the check runs it was added up from.
    fn into_parts(self) -> Result<(PrSummary, Vec<CheckRun>), GhError> {
        let state = pr_state(&self.state, self.is_draft)
            .ok_or_else(|| GhError::Parse(format!("PR state {}", self.state)))?;
        let review = match self.review_decision.as_deref() {
            Some("APPROVED") => Some(ReviewDecision::Approved),
            Some("CHANGES_REQUESTED") => Some(ReviewDecision::ChangesRequested),
            Some("REVIEW_REQUIRED") => Some(ReviewDecision::ReviewRequired),
            _ => None,
        };
        let (checks, failed, runs) = match self.status_check_rollup {
            None => (CheckRollup::None, Vec::new(), Vec::new()),
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
                (checks, failed, runs)
            }
        };
        let summary = PrSummary {
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
        };
        Ok((summary, runs))
    }
}

#[cfg(test)]
mod tests;
