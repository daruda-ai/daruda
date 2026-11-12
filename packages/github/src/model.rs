//! The pull-request values the rest of daruda reads.

/// A GitHub repository. `owner` and `name` are lowercased: GitHub compares
/// them case-insensitively, and so must every lookup keyed by this.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoId {
    pub host: String,
    pub owner: String,
    pub name: String,
}

impl RepoId {
    /// `host/owner/name`, the form `gh --repo` accepts for any host.
    pub fn qualified(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Draft,
    Merged,
    Closed,
}

impl PrState {
    /// Open or draft: the PR still takes pushes and runs checks.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Open | Self::Draft)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

/// One check as GitHub reports it, before [`crate::checks::summarize`]
/// decides what it counts as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRun {
    pub name: String,
    pub status: CheckStatus,
    pub details_url: Option<String>,
    /// RFC 3339, so later runs sort after earlier ones as strings.
    pub started_at: Option<String>,
}

/// What a single check counts as. The mapping from GitHub's values lives in
/// [`crate::checks`], with the rest of the rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pending,
    Success,
    Failure,
    /// Skipped, neutral or stale: left out of the rollup.
    Ignored,
}

/// The checks of a pull request, added up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckRollup {
    /// No check that counts.
    None,
    Pending,
    Passing,
    Failing {
        failed: u32,
    },
}

/// A GitHub Actions job, the unit whose failed log `gh run view` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionsJob {
    pub run: u64,
    pub job: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedCheck {
    pub name: String,
    pub details_url: Option<String>,
    /// `None` for a check outside GitHub Actions, which has no log to fetch.
    pub job: Option<ActionsJob>,
}

/// One comment in a review thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewComment {
    /// `None` for a deleted account.
    pub author: Option<String>,
    pub body: String,
}

/// An unresolved review thread on a pull request's diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewThread {
    pub path: String,
    /// The commented lines, first to last; `None` for a file-level comment.
    pub lines: Option<(u32, u32)>,
    /// The diff moved since; `lines` are where it was written.
    pub outdated: bool,
    pub comments: Vec<ReviewComment>,
}

/// The unresolved threads of a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewThreads {
    pub threads: Vec<ReviewThread>,
    /// GitHub had more threads than one request reads.
    pub truncated: bool,
}

/// A pull request to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPr {
    /// The branch on the repository's side, already pushed.
    pub head: String,
    /// `None` lets GitHub pick the repository's default branch.
    pub base: Option<String>,
    pub title: String,
    pub body: String,
    pub draft: bool,
    /// The lane's worktree; `gh` runs there so it reads the same remotes.
    pub worktree: std::path::PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrSummary {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub state: PrState,
    pub head_branch: String,
    /// The head lives in another repository (a fork).
    pub cross_repo: bool,
    pub checks: CheckRollup,
    /// The failing checks behind [`CheckRollup::Failing`]; may hold fewer
    /// than its count when GitHub truncated the list.
    pub failed: Vec<FailedCheck>,
    pub review: Option<ReviewDecision>,
    /// RFC 3339.
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_open_and_draft_prs_are_active() {
        let active: Vec<_> = [
            PrState::Open,
            PrState::Draft,
            PrState::Merged,
            PrState::Closed,
        ]
        .into_iter()
        .map(PrState::is_active)
        .collect();
        assert_eq!(active, [true, true, false, false]);
    }
}
