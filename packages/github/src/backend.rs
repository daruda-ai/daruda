//! The seam in front of the `gh` CLI: callers that take [`Gh`] can be handed
//! a fake in tests instead of reaching GitHub with the user's credentials.
//! Methods are whole questions, not a passthrough to `gh api`, so a fake
//! answers each one directly.

use crate::availability::GhAuth;
use crate::query::RepoPrs;
use crate::{ActionsJob, GhError, NewPr, RepoId};

pub trait Gh: Send + Sync {
    /// Whether `gh` is installed and logged in to `host`.
    fn auth(&self, host: &str) -> Result<GhAuth, GhError>;
    /// The pull requests whose head is each of `branches` in `repo`, plus the
    /// repository's default branch.
    fn prs_for_branches(&self, repo: &RepoId, branches: &[String]) -> Result<RepoPrs, GhError>;
    /// Open a pull request; answers its URL.
    fn create_pr(&self, repo: &RepoId, new: &NewPr) -> Result<String, GhError>;
    /// The log of the failed steps of one Actions job.
    fn failed_log(&self, repo: &RepoId, job: ActionsJob) -> Result<String, GhError>;
}
