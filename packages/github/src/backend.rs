//! The seam in front of the `gh` CLI: callers that take [`Gh`] can be handed
//! a fake in tests instead of reaching GitHub with the user's credentials.
//! Methods are whole questions, not a passthrough to `gh api`, so a fake
//! answers each one directly.

use crate::availability::GhAuth;
use crate::query::RepoPrs;
use crate::{
    ActionsJob, GhError, MergeMethod, NewPr, PrDetail, PrFilter, PrSearch, RepoId, ReviewThreads,
};

pub trait Gh: Send + Sync {
    /// Whether `gh` is installed and logged in to `host`.
    fn auth(&self, host: &str) -> Result<GhAuth, GhError>;
    /// The pull requests whose head is each of `branches` in `repo`, plus the
    /// repository's default branch.
    fn prs_for_branches(&self, repo: &RepoId, branches: &[String]) -> Result<RepoPrs, GhError>;
    /// The open pull requests of `repos`, all on `host`, that match
    /// `filter`, most recently updated first; at most 50 per 20
    /// repositories, the rest left out and marked `truncated`. No
    /// repositories sends no request.
    fn search_prs(
        &self,
        host: &str,
        repos: &[RepoId],
        filter: PrFilter,
    ) -> Result<PrSearch, GhError>;
    /// Open a pull request; answers its URL.
    fn create_pr(&self, repo: &RepoId, new: &NewPr) -> Result<String, GhError>;
    /// The log of the failed steps of one Actions job.
    fn failed_log(&self, repo: &RepoId, job: ActionsJob) -> Result<String, GhError>;
    /// The unresolved review threads of pull request `number`.
    fn review_threads(&self, repo: &RepoId, number: u64) -> Result<ReviewThreads, GhError>;
    /// What pull request `number`'s page shows, in one request.
    fn pr_detail(&self, repo: &RepoId, number: u64) -> Result<PrDetail, GhError>;
    /// Merge pull request `number` with `method`, only if its head is still
    /// `head_sha`; otherwise [`GhError::HeadMoved`]. Leaves the branch.
    fn merge_pr(
        &self,
        repo: &RepoId,
        number: u64,
        method: MergeMethod,
        head_sha: &str,
    ) -> Result<(), GhError>;
    /// Mark draft pull request `number` ready for review.
    fn set_ready(&self, repo: &RepoId, number: u64) -> Result<(), GhError>;
    /// Close pull request `number` unmerged. Leaves the branch.
    fn close_pr(&self, repo: &RepoId, number: u64) -> Result<(), GhError>;
    /// Rerun the failed jobs of Actions run `run`.
    fn rerun_failed(&self, repo: &RepoId, run: u64) -> Result<(), GhError>;
}
