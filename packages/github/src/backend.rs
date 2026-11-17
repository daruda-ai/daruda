//! The seam in front of the `gh` CLI: callers that take [`Gh`] can be handed
//! a fake in tests instead of reaching GitHub with the user's credentials.
//! Methods are whole questions, not a passthrough to `gh api`, so a fake
//! answers each one directly.

use crate::availability::AuthStatus;
use crate::query::RepoPrs;
use crate::{
    ActionsJob, GhError, MergeMethod, NewPr, PrDetail, PrFilter, PrSearch, PrSearchPage,
    PrSearchRequest, RepoId, ReviewThreads,
};

pub trait Gh: Send + Sync {
    /// Whether `gh` is installed and logged in to `host`, and as whom.
    fn auth(&self, host: &str) -> Result<AuthStatus, GhError>;
    /// The pull requests whose head is each of `branches` in `repo`, the
    /// pull requests `numbers` name there, and the repository's default
    /// branch. Neither branches nor numbers sends no request.
    fn prs_for_branches(
        &self,
        repo: &RepoId,
        branches: &[String],
        numbers: &[u64],
    ) -> Result<RepoPrs, GhError>;
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
    /// One bounded provider-search page. Follow its scoped continuation to
    /// read later pages; empty repository scope sends no request.
    fn search_prs_page(&self, request: &PrSearchRequest) -> Result<PrSearchPage, GhError>;
    /// Open a pull request; answers its URL.
    fn create_pr(&self, repo: &RepoId, new: &NewPr) -> Result<String, GhError>;
    /// The log of the failed steps of one Actions job.
    fn failed_log(&self, repo: &RepoId, job: ActionsJob) -> Result<String, GhError>;
    /// The unresolved review threads of pull request `number`.
    fn review_threads(&self, repo: &RepoId, number: u64) -> Result<ReviewThreads, GhError>;
    /// Page `page` (from 1) of the files pull request `number` changes, each
    /// with its patch when GitHub sends one; GitHub lists at most
    /// [`crate::PR_FILES_MAX`] in all.
    fn pr_files_page(
        &self,
        repo: &RepoId,
        number: u64,
        page: u32,
    ) -> Result<crate::PrFilesPage, GhError>;
    /// The commit a pull request's diff of `head` against `base` (a branch
    /// or a commit) is taken from: their merge base.
    fn merge_base(&self, repo: &RepoId, base: &str, head: &str) -> Result<String, GhError>;
    /// What `path` holds at `commit`.
    fn file_at(&self, repo: &RepoId, path: &str, commit: &str) -> Result<crate::FileText, GhError>;
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
    /// Reply `body` to review thread `thread` (its node id); answers the
    /// comment posted.
    fn reply_to_thread(
        &self,
        repo: &RepoId,
        thread: &str,
        body: &str,
    ) -> Result<crate::ReviewComment, GhError>;
    /// Resolve review thread `thread`, or unresolve it.
    fn set_thread_resolved(
        &self,
        repo: &RepoId,
        thread: &str,
        resolved: bool,
    ) -> Result<(), GhError>;
    /// The comments on pull request `number` itself, oldest first.
    fn pr_conversation(&self, repo: &RepoId, number: u64) -> Result<crate::Conversation, GhError>;
    /// Ask `add` to review pull request `number` and stop asking `remove`:
    /// user logins or `org/team` slugs, each checked by
    /// [`crate::reviewer_name_ok`].
    fn edit_pr_reviewers(
        &self,
        repo: &RepoId,
        number: u64,
        add: &[String],
        remove: &[String],
    ) -> Result<(), GhError>;
    /// Submit a review of pull request `pr` (its node id) at `commit`:
    /// its verdict, text and comments on lines.
    fn submit_review(
        &self,
        repo: &RepoId,
        pr: &str,
        commit: &str,
        event: crate::ReviewEvent,
        body: &str,
        threads: &[crate::ReviewThreadDraft],
    ) -> Result<crate::SubmittedReview, GhError>;
    /// The viewer's latest review of pull request `number`, if any.
    fn viewer_latest_review(
        &self,
        repo: &RepoId,
        number: u64,
    ) -> Result<Option<crate::LatestReview>, GhError>;
    /// Replace pull request `number`'s description with `body`.
    fn edit_pr_body(&self, repo: &RepoId, number: u64, body: &str) -> Result<(), GhError>;
    /// Close pull request `number` unmerged. Leaves the branch.
    fn close_pr(&self, repo: &RepoId, number: u64) -> Result<(), GhError>;
    /// Reopen closed pull request `number`.
    fn reopen_pr(&self, repo: &RepoId, number: u64) -> Result<(), GhError>;
    /// Turn auto-merge on for pull request `pr` (its node id) with
    /// `method`, only while its head is `head_sha`: GitHub then merges it,
    /// or queues it, once its requirements pass.
    fn enable_auto_merge(
        &self,
        repo: &RepoId,
        pr: &str,
        method: MergeMethod,
        head_sha: &str,
    ) -> Result<(), GhError>;
    /// Turn auto-merge off for pull request `pr` (its node id).
    fn disable_auto_merge(&self, repo: &RepoId, pr: &str) -> Result<(), GhError>;
    /// Rerun the failed jobs of Actions run `run`.
    fn rerun_failed(&self, repo: &RepoId, run: u64) -> Result<(), GhError>;
    /// When the spent GraphQL rate limit on `host` resets (Unix seconds);
    /// `None` while requests remain. Asking does not draw on the limit.
    fn graphql_rate_limit_reset(&self, host: &str) -> Result<Option<i64>, GhError>;
}
