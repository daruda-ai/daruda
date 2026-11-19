//! A pull request's place in a native GitHub stack: the stack's number,
//! the branch it targets and its pull requests in order. Asked apart from
//! the detail, so a server without stacks still shows the detail.
//!
//! A new pull request joins a stack only at its top: GitHub's REST API
//! creates a stack from pull requests in order and appends to one, and has
//! nothing that inserts, reorders or takes one pull request out.

use serde::Deserialize;

use crate::{GhError, PrState};

/// Pull requests of a stack read at once; more are counted, not listed.
pub(crate) const STACK_PAGE: u32 = 20;

/// The stack of pull request `$number`, if it is in one.
pub(crate) fn stack_query() -> String {
    format!(
        "query($owner: String!, $name: String!, $number: Int!) {{ \
         repository(owner: $owner, name: $name) {{ pullRequest(number: $number) {{ \
         stack {{ number size baseRefName entries(first: {STACK_PAGE}) {{ totalCount \
         nodes {{ position pullRequest {{ number title state isDraft }} }} }} }} }} }} }}"
    )
}

/// One pull request of a stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackEntry {
    /// 1 is closest to the base branch.
    pub position: u32,
    pub number: u64,
    pub title: String,
    pub state: PrState,
}

/// A native stack of pull requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrStack {
    pub number: u64,
    /// The branch the stack's pull requests target.
    pub base: String,
    pub size: u32,
    /// In stack order, bottom first.
    pub entries: Vec<StackEntry>,
}

impl PrStack {
    /// The open (not draft) pull requests from the bottom of the stack up to
    /// `number`, in order: what auto-merge with the stack below would take.
    /// `None` when `number` is not listed, or nothing below it is open.
    pub fn range_below(&self, number: u64) -> Option<Vec<u64>> {
        let at = self
            .entries
            .iter()
            .find(|entry| entry.number == number)?
            .position;
        let range: Vec<u64> = self
            .entries
            .iter()
            // ASSUMPTION: not measured — GitHub does not auto-merge a draft,
            // so drafts are left out of what the operation takes.
            .filter(|entry| entry.position <= at && entry.state == PrState::Open)
            .map(|entry| entry.number)
            .collect();
        (range.len() > 1).then_some(range)
    }

    /// Pull requests of the stack not listed here: past the page read, or
    /// ones the viewer cannot see.
    pub fn more(&self) -> usize {
        (self.size as usize).saturating_sub(self.entries.len())
    }
}

/// Where a new pull request goes to sit on top of another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackPlace {
    /// `parent` is in no stack: the two make a new one.
    New { parent: u64 },
    /// `parent` is the top of stack `stack`: the new one is appended.
    Top { stack: u64 },
}

/// Why a pull request cannot be stacked on another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoStacking {
    /// The server has no stacks.
    Unsupported,
    /// The other is in a stack but not at its top, or the top is past the
    /// entries read: only the top takes another.
    NotTop,
}

impl std::fmt::Display for NoStacking {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => write!(f, "the server has no stacks"),
            Self::NotTop => write!(f, "it is not the top of its stack"),
        }
    }
}

/// The API version the stack endpoints are documented under.
const STACKS_API_VERSION: &str = "X-GitHub-Api-Version: 2026-03-10";

/// The `gh api` arguments and the request body that put pull request
/// `number` at `place` in `repo`.
// ASSUMPTION: not measured — written from GitHub's REST reference
// (`POST /repos/{owner}/{repo}/stacks` with the pull requests bottom to
// top, `POST .../stacks/{stack_number}/add` with those to append), and
// that the `number` GraphQL reads for a stack is that `stack_number`.
pub(crate) fn stack_request(
    repo: &crate::RepoId,
    place: StackPlace,
    number: u64,
) -> (Vec<String>, String) {
    let stacks = format!("repos/{}/{}/stacks", repo.owner(), repo.name());
    let (path, pull_requests) = match place {
        StackPlace::New { parent } => (stacks, vec![parent, number]),
        StackPlace::Top { stack } => (format!("{stacks}/{stack}/add"), vec![number]),
    };
    let args = [
        "api",
        "--hostname",
        repo.host(),
        "--method",
        "POST",
        "-H",
        STACKS_API_VERSION,
        &path,
        "--input",
        "-",
    ]
    .map(str::to_owned)
    .to_vec();
    let body = serde_json::json!({ "pull_requests": pull_requests }).to_string();
    (args, body)
}

/// What asking for a pull request's stack answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StackRead {
    NotStacked,
    Stacked(PrStack),
    /// The server has no stacks: the ordinary pull request flow only.
    Unsupported,
}

impl StackRead {
    /// Where a pull request stacked on `parent`, whose stack this is,
    /// would go.
    pub fn place_on(&self, parent: u64) -> Result<StackPlace, NoStacking> {
        match self {
            Self::NotStacked => Ok(StackPlace::New { parent }),
            Self::Unsupported => Err(NoStacking::Unsupported),
            Self::Stacked(stack) => {
                let top = stack.entries.last().filter(|_| stack.more() == 0);
                match top {
                    Some(top) if top.number == parent => Ok(StackPlace::Top {
                        stack: stack.number,
                    }),
                    _ => Err(NoStacking::NotTop),
                }
            }
        }
    }

    /// [`PrStack::range_below`] of a stacked pull request; `None` otherwise.
    pub fn range_below(&self, number: u64) -> Option<Vec<u64>> {
        match self {
            Self::Stacked(stack) => stack.range_below(number),
            Self::NotStacked | Self::Unsupported => None,
        }
    }
}

/// Parse the answer to [`stack_query`].
pub(crate) fn parse_stack(json: &str) -> Result<StackRead, GhError> {
    let pr: PrNode = super::pull_request(json)?;
    let Some(stack) = pr.stack else {
        return Ok(StackRead::NotStacked);
    };
    let mut entries: Vec<StackEntry> = stack
        .entries
        .nodes
        .into_iter()
        .filter_map(|node| {
            let pr = node.pull_request?;
            Some(StackEntry {
                position: node.position,
                number: pr.number,
                state: super::pr_state(&pr.state, pr.is_draft)?,
                title: pr.title,
            })
        })
        .collect();
    entries.sort_by_key(|entry| entry.position);
    Ok(StackRead::Stacked(PrStack {
        number: stack.number,
        base: stack.base_ref_name,
        size: stack.size,
        entries,
    }))
}

/// Whether `gh`'s refusal says the server has no stack fields at all.
// ASSUMPTION: not measured — a server without stacks rejects the query
// with GraphQL's usual "Field 'stack' doesn't exist on type 'PullRequest'".
pub(crate) fn lacks_stacks(stderr: &str) -> bool {
    stderr.contains("Field 'stack' doesn't exist on type 'PullRequest'")
}

#[derive(Deserialize)]
struct PrNode {
    stack: Option<StackNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StackNode {
    number: u64,
    size: u32,
    base_ref_name: String,
    entries: Entries,
}

#[derive(Deserialize)]
struct Entries {
    nodes: Vec<EntryNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntryNode {
    position: u32,
    pull_request: Option<EntryPr>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntryPr {
    number: u64,
    title: String,
    state: String,
    #[serde(default)]
    is_draft: bool,
}

#[cfg(test)]
mod tests;
