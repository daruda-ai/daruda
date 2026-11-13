//! The pull-request values the rest of daruda reads.

/// The public GitHub host. `gh` trouble on it is worth telling the user
/// about unasked; another host may not be GitHub at all.
pub const GITHUB_HOST: &str = "github.com";

/// A GitHub repository. Every part is lowercased by [`RepoId::new`]: GitHub
/// compares them case-insensitively, and so must every lookup keyed by this.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoId {
    host: String,
    owner: String,
    name: String,
}

impl RepoId {
    pub fn new(host: &str, owner: &str, name: &str) -> Self {
        Self {
            host: host.to_ascii_lowercase(),
            owner: owner.to_ascii_lowercase(),
            name: name.to_ascii_lowercase(),
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn name(&self) -> &str {
        &self.name
    }

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

/// Where a check comes from. A check run and a commit status are separate
/// namespaces: the same name in both is two checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckKind {
    /// A check run (GitHub Actions or another app).
    Run,
    /// A commit status context (external CI).
    Status,
}

/// One check as GitHub reports it, before the crate's rollup rules decide
/// what it counts as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRun {
    pub kind: CheckKind,
    /// The Actions workflow a check run belongs to; two workflows may each
    /// have a job of the same name.
    pub workflow: Option<String>,
    /// The event that ran the workflow (`push`, `pull_request`, ...). A
    /// workflow on both runs each job twice for a same-repository PR — on
    /// the branch head and on the merge with the base — and either can fail.
    pub event: Option<String>,
    pub name: String,
    pub status: CheckStatus,
    pub details_url: Option<String>,
    /// RFC 3339, so later runs sort after earlier ones as strings. `None`
    /// for a run still queued.
    pub started_at: Option<String>,
    /// RFC 3339. `None` for a run not finished yet and for a commit status,
    /// which reports no end.
    pub completed_at: Option<String>,
}

/// What a single check counts as. The mapping from GitHub's values is
/// private to this crate, with the rest of the rollup rules.
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
        /// How many checks fail; `None` when GitHub truncated the list, so
        /// only its own verdict is known, not the count.
        failed: Option<u32>,
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
    /// The Actions workflow, for a check run that belongs to one.
    pub workflow: Option<String>,
    /// The event that ran it, for an Actions check run.
    pub event: Option<String>,
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
    /// The thread has more replies than one request reads.
    pub more_comments: bool,
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
    /// The failing checks in hand behind [`CheckRollup::Failing`]; with a
    /// truncated list, those on its first page.
    pub failed: Vec<FailedCheck>,
    pub review: Option<ReviewDecision>,
    /// RFC 3339.
    pub updated_at: String,
}

/// Which open pull requests a list search asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrFilter {
    /// Authored by the logged-in user.
    Mine,
    /// Waiting on the logged-in user's review.
    ReviewRequested,
    AllOpen,
}

/// One open pull request in a list search, with the repository it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrListItem {
    pub repo: RepoId,
    pub pr: PrSummary,
    /// `None` for a deleted account.
    pub author: Option<String>,
    pub base_branch: String,
    /// Conversation comments, not review comments on the diff.
    pub comments: u32,
}

/// What a list search found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrSearch {
    /// Most recently updated first.
    pub items: Vec<PrListItem>,
    /// GitHub matched more pull requests than one search answers.
    pub truncated: bool,
}

/// A way GitHub can merge a pull request, as its repository allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

/// Whether a pull request can be merged now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mergeability {
    /// GitHub would merge it; failing checks that are not required do not
    /// stop a merge, and show in the check list instead.
    Ready,
    Blocked(BlockReason),
    /// The head conflicts with the base.
    Conflicting,
    /// GitHub has not worked it out yet (it does so lazily, after a push),
    /// or answered a value this crate does not know.
    Unknown,
}

/// Why GitHub refuses to merge a pull request that has no conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// Branch protection: a required review or check is missing.
    Requirements,
    /// The base requires the head to be up to date with it first.
    Behind,
    /// A draft cannot be merged.
    Draft,
}

/// Where one reviewer stands on a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewerState {
    /// Asked to review and has not reviewed since.
    Requested,
    Approved,
    ChangesRequested,
    Commented,
    /// The latest review was dismissed.
    Dismissed,
    /// A review started and not yet submitted.
    Pending,
    /// A review state this crate does not know.
    Other,
}

/// One check in a pull request's list: the latest run of each check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckEntry {
    pub run: CheckRun,
    /// The Actions job behind it, whose log can be fetched.
    pub job: Option<ActionsJob>,
    /// From start to completion; `None` until it completes, for a commit
    /// status, and for times GitHub reports out of order.
    pub duration: Option<std::time::Duration>,
}

/// What a pull request's page shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrDetail {
    pub summary: PrSummary,
    pub body: String,
    /// The head commit, which a merge can be pinned to.
    pub head_sha: String,
    pub base_branch: String,
    /// `None` for a deleted account.
    pub author: Option<String>,
    pub mergeability: Mergeability,
    /// What the repository allows, in the order merge, squash, rebase.
    pub methods: Vec<MergeMethod>,
    /// Reviewers who reviewed, then those only asked to; a team reads
    /// `org/team`. A request GitHub does not name to the caller is left out.
    pub reviewers: Vec<(String, ReviewerState)>,
    /// Failing, then running, then passing, then skipped; GitHub's order
    /// within each. With more than 100 checks, those of the first page.
    pub checks: Vec<CheckEntry>,
    /// Unresolved review threads among the first 100.
    pub open_threads: u32,
}

impl PrSummary {
    /// The PR's page, if `url` is one a browser may be handed: whatever host
    /// answered, a `file:` or script URL must not reach the system opener.
    pub fn web_url(&self) -> Option<&str> {
        let url = self.url.as_str();
        (url.starts_with("https://") || url.starts_with("http://")).then_some(url)
    }
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
