//! Checks: each run or status, and what they add up to.

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
