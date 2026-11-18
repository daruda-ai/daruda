//! A pull request's place in a native GitHub stack: the stack's number,
//! the branch it targets and its pull requests in order. Asked apart from
//! the detail, so a server without stacks still shows the detail.

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

/// What asking for a pull request's stack answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StackRead {
    NotStacked,
    Stacked(PrStack),
    /// The server has no stacks: the ordinary pull request flow only.
    Unsupported,
}

impl StackRead {
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
