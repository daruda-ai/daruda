//! A scripted [`Gh`] for tests: answers come from what the test set, calls
//! are logged as [`GhCall`]s, and no `gh` process is spawned.
//!
//! | Method | Unscripted answer |
//! |---|---|
//! | `auth` | `Ready`, no account |
//! | `prs_for_branches` | default branch `main`, no PRs for any branch or number |
//! | `search_prs` | none, not truncated |
//! | `search_prs_page` | the `set_pr_list` list of its audience, narrowed to its repositories, state and title words (not facets), paged |
//! | `create_pr` | adds an open PR numbered after the repo's last, answers its URL |
//! | `failed_log` | exit 1, as for a run that has no failed log |
//! | `review_threads` | none |
//! | `pr_files_page` | the `set_pr_files` list, paged as GitHub pages it |
//! | `merge_base` | `merge-base:{base}...{head}` |
//! | `file_at` | `Absent` |
//! | `pr_detail` | exit 1, as for a pull request that does not exist |
//! | `merge_pr` | `HeadMoved` if a scripted detail's head differs, else merged |
//! | `set_ready` | the PR, if scripted, becomes open |
//! | `close_pr` | the PR, if scripted, becomes closed |
//! | `reopen_pr` | the PR, if scripted, becomes open |
//! | `rerun_failed` | done |
//! | `reply_to_thread` | adds the reply to the scripted thread, answers it |
//! | `pr_conversation` | the `set_conversation` comments, else none |
//! | `edit_pr_reviewers` | the scripted detail asks those added, not those removed |
//! | `submit_review` | recorded; becomes the viewer's latest review |
//! | `viewer_latest_review` | the last review submitted, or `set_latest_review`'s |
//! | `edit_pr_body` | the scripted detail's body becomes `body` |
//! | `set_thread_resolved` | a resolved thread leaves `review_threads` until unresolved |
//! | `graphql_rate_limit_reset` | `None`: requests remain |
//!
//! A mutation changes the state of the scripted PR in both `set_prs` and
//! `set_pr_detail`, so the next lookup sees it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::availability::{AuthStatus, GhAccount, GhAuth};
use crate::backend::Gh;
use crate::query::{RepoPrs, newest_first};
use crate::system::check_head_sha;
use crate::{
    ActionsJob, CheckRollup, GhError, MergeMethod, NewPr, PrDetail, PrFilter, PrListItem, PrSearch,
    PrSearchPage, PrSearchRequest, PrState, PrSummary, RepoId, ReviewComment, ReviewThreads,
};

mod search;

/// A request the code under test made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhCall {
    Auth {
        host: String,
    },
    PrsForBranches {
        repo: RepoId,
        branches: Vec<String>,
        numbers: Vec<u64>,
    },
    SearchPrs {
        host: String,
        repos: Vec<RepoId>,
        filter: PrFilter,
    },
    SearchPrsPage {
        request: Box<PrSearchRequest>,
    },
    CreatePr {
        repo: RepoId,
        new: NewPr,
    },
    FailedLog {
        repo: RepoId,
        job: ActionsJob,
    },
    ReviewThreads {
        repo: RepoId,
        number: u64,
    },
    PrFiles {
        repo: RepoId,
        number: u64,
        page: u32,
    },
    MergeBase {
        repo: RepoId,
        base: String,
        head: String,
    },
    FileAt {
        repo: RepoId,
        path: String,
        commit: String,
    },
    PrDetail {
        repo: RepoId,
        number: u64,
    },
    MergePr {
        repo: RepoId,
        number: u64,
        method: MergeMethod,
        head_sha: String,
    },
    SetReady {
        repo: RepoId,
        number: u64,
    },
    ClosePr {
        repo: RepoId,
        number: u64,
    },
    ReopenPr {
        repo: RepoId,
        number: u64,
    },
    ReplyToThread {
        repo: RepoId,
        thread: String,
        body: String,
    },
    SetThreadResolved {
        repo: RepoId,
        thread: String,
        resolved: bool,
    },
    Conversation {
        repo: RepoId,
        number: u64,
    },
    EditPrBody {
        repo: RepoId,
        number: u64,
        body: String,
    },
    SubmitReview {
        repo: RepoId,
        pr: String,
        commit: String,
        event: crate::ReviewEvent,
        body: String,
        threads: Vec<crate::ReviewThreadDraft>,
    },
    LatestReview {
        repo: RepoId,
        number: u64,
    },
    EditPrReviewers {
        repo: RepoId,
        number: u64,
        add: Vec<String>,
        remove: Vec<String>,
    },
    RerunFailed {
        repo: RepoId,
        run: u64,
    },
    RateLimitReset {
        host: String,
    },
}

/// One [`Gh`] method, for aiming failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GhOp {
    Auth,
    PrsForBranches,
    SearchPrs,
    SearchPrsPage,
    CreatePr,
    FailedLog,
    ReviewThreads,
    PrFiles,
    MergeBase,
    FileAt,
    PrDetail,
    MergePr,
    SetReady,
    ClosePr,
    ReopenPr,
    ReplyToThread,
    SetThreadResolved,
    Conversation,
    EditPrBody,
    SubmitReview,
    LatestReview,
    EditPrReviewers,
    RerunFailed,
    RateLimitReset,
}

/// A cloneable description of a [`GhError`], turned into a fresh error on
/// every call it fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhFailure {
    NotFound,
    Exit { code: Option<i32>, stderr: String },
    Parse(String),
    HeadMoved,
}

impl GhFailure {
    fn to_error(&self) -> GhError {
        match self {
            Self::NotFound => GhError::NotFound,
            Self::Exit { code, stderr } => GhError::Exit {
                code: *code,
                stderr: stderr.clone(),
            },
            Self::Parse(message) => GhError::Parse(message.clone()),
            Self::HeadMoved => GhError::HeadMoved,
        }
    }
}

/// Clones share one state, so a test keeps a clone to script and inspect
/// while the code under test holds `Arc<dyn Gh>`.
#[derive(Debug, Clone, Default)]
pub struct FakeGh {
    state: Arc<Mutex<FakeState>>,
}

#[derive(Debug, Default)]
struct FakeState {
    auth: HashMap<String, GhAuth>,
    accounts: HashMap<String, GhAccount>,
    default_branches: HashMap<RepoId, String>,
    prs: HashMap<RepoId, Vec<PrSummary>>,
    lists: HashMap<PrFilter, Vec<PrListItem>>,
    truncated: HashMap<PrFilter, bool>,
    search_pages: HashMap<PrSearchRequest, String>,
    logs: HashMap<(u64, u64), String>,
    threads: HashMap<u64, ReviewThreads>,
    resolved_threads: std::collections::HashSet<String>,
    conversations: HashMap<(RepoId, u64), crate::Conversation>,
    latest_review: Option<crate::LatestReview>,
    /// A scripted `submit_review` failure still lands, as when `gh` times
    /// out after GitHub took the review.
    review_lands_anyway: bool,
    details: HashMap<(RepoId, u64), PrDetail>,
    files: HashMap<(RepoId, u64), crate::PrFiles>,
    file_texts: HashMap<(RepoId, String, String), crate::FileText>,
    failures: HashMap<GhOp, GhFailure>,
    resets: HashMap<String, i64>,
    calls: Vec<GhCall>,
}

impl FakeGh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_auth(&self, host: &str, auth: GhAuth) {
        self.lock().auth.insert(host.to_owned(), auth);
    }

    /// The account a ready `host` answers it is logged in as.
    pub fn set_account(&self, host: &str, account: GhAccount) {
        self.lock().accounts.insert(host.to_owned(), account);
    }

    pub fn set_default_branch(&self, repo: &RepoId, branch: &str) {
        self.lock()
            .default_branches
            .insert(repo.clone(), branch.to_owned());
    }

    /// The PRs `repo` has; `prs_for_branches` answers each branch with those
    /// whose head is that branch, and each number with the PR it names.
    pub fn set_prs(&self, repo: &RepoId, prs: Vec<PrSummary>) {
        self.lock().prs.insert(repo.clone(), prs);
    }

    /// What `search_prs` with `filter` finds; each search answers those of
    /// its repositories.
    pub fn set_pr_list(&self, filter: PrFilter, items: Vec<PrListItem>) {
        self.lock().lists.insert(filter, items);
    }

    /// Whether `search_prs` with `filter` reports more than it answered.
    pub fn set_search_truncated(&self, filter: PrFilter, truncated: bool) {
        self.lock().truncated.insert(filter, truncated);
    }

    /// What `failed_log` prints for `job`.
    pub fn set_failed_log(&self, job: ActionsJob, log: &str) {
        self.lock().logs.insert((job.run, job.job), log.to_owned());
    }

    /// The unresolved threads of PR `number`, whatever its repository.
    pub fn set_review_threads(&self, number: u64, threads: ReviewThreads) {
        self.lock().threads.insert(number, threads);
    }

    /// What `pr_files` answers for PR `number` of `repo`.
    pub fn set_pr_files(&self, repo: &RepoId, number: u64, files: crate::PrFiles) {
        self.lock().files.insert((repo.clone(), number), files);
    }

    /// What `file_at` answers for `path` at `commit` of `repo`.
    pub fn set_file_at(&self, repo: &RepoId, path: &str, commit: &str, text: crate::FileText) {
        self.lock()
            .file_texts
            .insert((repo.clone(), path.to_owned(), commit.to_owned()), text);
    }

    /// What `pr_conversation` answers for PR `number` of `repo`.
    pub fn set_conversation(&self, repo: &RepoId, number: u64, conversation: crate::Conversation) {
        self.lock()
            .conversations
            .insert((repo.clone(), number), conversation);
    }

    /// What `viewer_latest_review` answers, as if a review were submitted
    /// elsewhere or a lost submission went through.
    pub fn set_latest_review(&self, review: Option<crate::LatestReview>) {
        self.lock().latest_review = review;
    }

    /// Whether a scripted `submit_review` failure lands the review anyway.
    pub fn set_review_lands_anyway(&self, lands: bool) {
        self.lock().review_lands_anyway = lands;
    }

    /// What `pr_detail` answers for PR `number` of `repo`.
    pub fn set_pr_detail(&self, repo: &RepoId, number: u64, detail: PrDetail) {
        self.lock().details.insert((repo.clone(), number), detail);
    }

    /// That the GraphQL limit on `host` is spent and resets at `reset`
    /// (Unix seconds).
    pub fn set_rate_limit_reset(&self, host: &str, reset: i64) {
        self.lock().resets.insert(host.to_owned(), reset);
    }

    /// Fail every later call of `op` until [`FakeGh::clear_failure`].
    pub fn fail(&self, op: GhOp, failure: GhFailure) {
        self.lock().failures.insert(op, failure);
    }

    pub fn clear_failure(&self, op: GhOp) {
        self.lock().failures.remove(&op);
    }

    pub fn calls(&self) -> Vec<GhCall> {
        self.lock().calls.clone()
    }

    /// How many calls of `op` were made.
    pub fn count(&self, op: GhOp) -> usize {
        self.lock()
            .calls
            .iter()
            .filter(|call| call.op() == op)
            .count()
    }

    fn lock(&self) -> MutexGuard<'_, FakeState> {
        // A test that panicked mid-call leaves plain data behind; keep going.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl GhCall {
    fn op(&self) -> GhOp {
        match self {
            Self::Auth { .. } => GhOp::Auth,
            Self::PrsForBranches { .. } => GhOp::PrsForBranches,
            Self::SearchPrs { .. } => GhOp::SearchPrs,
            Self::SearchPrsPage { .. } => GhOp::SearchPrsPage,
            Self::CreatePr { .. } => GhOp::CreatePr,
            Self::FailedLog { .. } => GhOp::FailedLog,
            Self::ReviewThreads { .. } => GhOp::ReviewThreads,
            Self::PrFiles { .. } => GhOp::PrFiles,
            Self::MergeBase { .. } => GhOp::MergeBase,
            Self::FileAt { .. } => GhOp::FileAt,
            Self::PrDetail { .. } => GhOp::PrDetail,
            Self::MergePr { .. } => GhOp::MergePr,
            Self::SetReady { .. } => GhOp::SetReady,
            Self::ClosePr { .. } => GhOp::ClosePr,
            Self::ReopenPr { .. } => GhOp::ReopenPr,
            Self::ReplyToThread { .. } => GhOp::ReplyToThread,
            Self::SetThreadResolved { .. } => GhOp::SetThreadResolved,
            Self::Conversation { .. } => GhOp::Conversation,
            Self::EditPrBody { .. } => GhOp::EditPrBody,
            Self::SubmitReview { .. } => GhOp::SubmitReview,
            Self::LatestReview { .. } => GhOp::LatestReview,
            Self::EditPrReviewers { .. } => GhOp::EditPrReviewers,
            Self::RerunFailed { .. } => GhOp::RerunFailed,
            Self::RateLimitReset { .. } => GhOp::RateLimitReset,
        }
    }
}

impl FakeState {
    /// Log `call`, then answer the failure scripted for it, if any.
    fn record(&mut self, call: GhCall) -> Result<(), GhError> {
        let op = call.op();
        self.calls.push(call);
        match self.failures.get(&op) {
            Some(failure) => Err(failure.to_error()),
            None => Ok(()),
        }
    }

    /// Move PR `number` of `repo` to `state` wherever it is scripted.
    fn set_state(&mut self, repo: &RepoId, number: u64, state: PrState) {
        let listed = self.prs.get_mut(repo).into_iter().flatten();
        let detailed = self.details.get_mut(&(repo.clone(), number));
        for pr in listed
            .filter(|pr| pr.number == number)
            .chain(detailed.map(|detail| &mut detail.summary))
        {
            pr.state = state;
        }
    }
}

impl Gh for FakeGh {
    fn auth(&self, host: &str) -> Result<AuthStatus, GhError> {
        let mut state = self.lock();
        state.calls.push(GhCall::Auth {
            host: host.to_owned(),
        });
        if let Some(failure) = state.failures.get(&GhOp::Auth) {
            return Err(failure.to_error());
        }
        let auth = state.auth.get(host).copied().unwrap_or(GhAuth::Ready);
        Ok(AuthStatus {
            auth,
            account: (auth == GhAuth::Ready)
                .then(|| state.accounts.get(host).cloned())
                .flatten(),
        })
    }

    fn prs_for_branches(
        &self,
        repo: &RepoId,
        branches: &[String],
        numbers: &[u64],
    ) -> Result<RepoPrs, GhError> {
        let mut state = self.lock();
        state.calls.push(GhCall::PrsForBranches {
            repo: repo.clone(),
            branches: branches.to_vec(),
            numbers: numbers.to_vec(),
        });
        if let Some(failure) = state.failures.get(&GhOp::PrsForBranches) {
            return Err(failure.to_error());
        }
        let prs = state.prs.get(repo).cloned().unwrap_or_default();
        let by_branch = branches
            .iter()
            .map(|branch| {
                let mut matching: Vec<PrSummary> = prs
                    .iter()
                    .filter(|pr| &pr.head_branch == branch)
                    .cloned()
                    .collect();
                matching.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                (branch.clone(), matching)
            })
            .collect();
        let by_number = numbers
            .iter()
            .map(|number| {
                let pr = prs.iter().find(|pr| pr.number == *number).cloned();
                (*number, pr)
            })
            .collect();
        Ok(RepoPrs {
            default_branch: Some(
                state
                    .default_branches
                    .get(repo)
                    .cloned()
                    .unwrap_or_else(|| "main".to_owned()),
            ),
            by_branch,
            by_number,
        })
    }

    fn search_prs(
        &self,
        host: &str,
        repos: &[RepoId],
        filter: PrFilter,
    ) -> Result<PrSearch, GhError> {
        let mut state = self.lock();
        state.calls.push(GhCall::SearchPrs {
            host: host.to_owned(),
            repos: repos.to_vec(),
            filter,
        });
        if let Some(failure) = state.failures.get(&GhOp::SearchPrs) {
            return Err(failure.to_error());
        }
        let mut items: Vec<PrListItem> = state
            .lists
            .get(&filter)
            .into_iter()
            .flatten()
            .filter(|item| {
                item.repo.host().eq_ignore_ascii_case(host) && repos.contains(&item.repo)
            })
            .cloned()
            .collect();
        newest_first(&mut items);
        Ok(PrSearch {
            items,
            truncated: state.truncated.get(&filter).copied().unwrap_or(false),
        })
    }

    fn search_prs_page(&self, request: &PrSearchRequest) -> Result<PrSearchPage, GhError> {
        self.search_page(request)
    }

    fn failed_log(&self, repo: &RepoId, job: ActionsJob) -> Result<String, GhError> {
        let mut state = self.lock();
        state.calls.push(GhCall::FailedLog {
            repo: repo.clone(),
            job,
        });
        if let Some(failure) = state.failures.get(&GhOp::FailedLog) {
            return Err(failure.to_error());
        }
        state
            .logs
            .get(&(job.run, job.job))
            .cloned()
            .ok_or_else(|| GhError::Exit {
                code: Some(1),
                stderr: "log not found".to_owned(),
            })
    }

    fn review_threads(&self, repo: &RepoId, number: u64) -> Result<ReviewThreads, GhError> {
        let mut state = self.lock();
        state.calls.push(GhCall::ReviewThreads {
            repo: repo.clone(),
            number,
        });
        if let Some(failure) = state.failures.get(&GhOp::ReviewThreads) {
            return Err(failure.to_error());
        }
        let mut threads = state.threads.get(&number).cloned().unwrap_or_default();
        threads
            .threads
            .retain(|thread| !state.resolved_threads.contains(&thread.id));
        Ok(threads)
    }

    fn pr_files_page(
        &self,
        repo: &RepoId,
        number: u64,
        page: u32,
    ) -> Result<crate::PrFilesPage, GhError> {
        let mut state = self.lock();
        state.record(GhCall::PrFiles {
            repo: repo.clone(),
            number,
            page,
        })?;
        let files = state
            .files
            .get(&(repo.clone(), number))
            .map(|listed| listed.files.as_slice())
            .unwrap_or_default();
        let per = crate::PR_FILES_PER_PAGE;
        let start = (page.max(1) as usize - 1) * per;
        let slice = files
            .get(start..(start + per).min(files.len()))
            .unwrap_or_default();
        Ok(crate::PrFilesPage::of(slice.to_vec(), page))
    }

    fn merge_base(&self, repo: &RepoId, base: &str, head: &str) -> Result<String, GhError> {
        self.lock().record(GhCall::MergeBase {
            repo: repo.clone(),
            base: base.to_owned(),
            head: head.to_owned(),
        })?;
        Ok(format!("merge-base:{base}...{head}"))
    }

    fn file_at(&self, repo: &RepoId, path: &str, commit: &str) -> Result<crate::FileText, GhError> {
        let mut state = self.lock();
        state.record(GhCall::FileAt {
            repo: repo.clone(),
            path: path.to_owned(),
            commit: commit.to_owned(),
        })?;
        Ok(state
            .file_texts
            .get(&(repo.clone(), path.to_owned(), commit.to_owned()))
            .cloned()
            .unwrap_or(crate::FileText::Absent))
    }

    fn pr_detail(&self, repo: &RepoId, number: u64) -> Result<PrDetail, GhError> {
        let mut state = self.lock();
        state.calls.push(GhCall::PrDetail {
            repo: repo.clone(),
            number,
        });
        if let Some(failure) = state.failures.get(&GhOp::PrDetail) {
            return Err(failure.to_error());
        }
        state
            .details
            .get(&(repo.clone(), number))
            .cloned()
            .ok_or_else(|| GhError::Exit {
                code: Some(1),
                stderr: format!("Could not resolve to a PullRequest with the number of {number}."),
            })
    }

    fn merge_pr(
        &self,
        repo: &RepoId,
        number: u64,
        method: MergeMethod,
        head_sha: &str,
    ) -> Result<(), GhError> {
        let mut state = self.lock();
        state.record(GhCall::MergePr {
            repo: repo.clone(),
            number,
            method,
            head_sha: head_sha.to_owned(),
        })?;
        check_head_sha(head_sha)?;
        let scripted_head = state
            .details
            .get(&(repo.clone(), number))
            .map(|detail| detail.head_sha.as_str());
        if scripted_head.is_some_and(|head| head != head_sha) {
            return Err(GhError::HeadMoved);
        }
        state.set_state(repo, number, PrState::Merged);
        Ok(())
    }

    fn set_ready(&self, repo: &RepoId, number: u64) -> Result<(), GhError> {
        let mut state = self.lock();
        state.record(GhCall::SetReady {
            repo: repo.clone(),
            number,
        })?;
        state.set_state(repo, number, PrState::Open);
        Ok(())
    }

    fn reply_to_thread(
        &self,
        repo: &RepoId,
        thread: &str,
        body: &str,
    ) -> Result<ReviewComment, GhError> {
        let mut state = self.lock();
        state.record(GhCall::ReplyToThread {
            repo: repo.clone(),
            thread: thread.to_owned(),
            body: body.to_owned(),
        })?;
        let posted = state.calls.len();
        let comment = ReviewComment {
            id: format!("fake-reply-{posted}"),
            author: Some("fake-viewer".into()),
            body: body.to_owned(),
            ..ReviewComment::default()
        };
        let held = state
            .threads
            .values_mut()
            .flat_map(|threads| threads.threads.iter_mut())
            .find(|held| held.id == thread);
        match held {
            Some(held) => {
                held.comments.push(comment.clone());
                Ok(comment)
            }
            None => Err(GhError::Parse(format!("no thread {thread}"))),
        }
    }

    fn set_thread_resolved(
        &self,
        repo: &RepoId,
        thread: &str,
        resolved: bool,
    ) -> Result<(), GhError> {
        let mut state = self.lock();
        state.record(GhCall::SetThreadResolved {
            repo: repo.clone(),
            thread: thread.to_owned(),
            resolved,
        })?;
        if resolved {
            state.resolved_threads.insert(thread.to_owned());
        } else {
            state.resolved_threads.remove(thread);
        }
        Ok(())
    }

    fn pr_conversation(&self, repo: &RepoId, number: u64) -> Result<crate::Conversation, GhError> {
        let mut state = self.lock();
        state.record(GhCall::Conversation {
            repo: repo.clone(),
            number,
        })?;
        Ok(state
            .conversations
            .get(&(repo.clone(), number))
            .cloned()
            .unwrap_or_default())
    }

    fn edit_pr_reviewers(
        &self,
        repo: &RepoId,
        number: u64,
        add: &[String],
        remove: &[String],
    ) -> Result<(), GhError> {
        let mut state = self.lock();
        state.record(GhCall::EditPrReviewers {
            repo: repo.clone(),
            number,
            add: add.to_vec(),
            remove: remove.to_vec(),
        })?;
        if let Some(detail) = state.details.get_mut(&(repo.clone(), number)) {
            detail.reviewers.retain(|(name, state)| {
                !(remove.contains(name) && *state == crate::ReviewerState::Requested)
            });
            // Asked again after a review, a reviewer is requested again, as
            // GitHub has them.
            for name in add {
                match detail.reviewers.iter_mut().find(|(held, _)| held == name) {
                    Some((_, state)) => *state = crate::ReviewerState::Requested,
                    None => detail
                        .reviewers
                        .push((name.clone(), crate::ReviewerState::Requested)),
                }
            }
        }
        Ok(())
    }

    fn submit_review(
        &self,
        repo: &RepoId,
        pr: &str,
        commit: &str,
        event: crate::ReviewEvent,
        body: &str,
        threads: &[crate::ReviewThreadDraft],
    ) -> Result<crate::SubmittedReview, GhError> {
        let mut state = self.lock();
        let answer = state.record(GhCall::SubmitReview {
            repo: repo.clone(),
            pr: pr.to_owned(),
            commit: commit.to_owned(),
            event,
            body: body.to_owned(),
            threads: threads.to_vec(),
        });
        let lands = answer.is_ok() || state.review_lands_anyway;
        let id = format!("fake-review-{}", state.calls.len());
        if lands {
            state.latest_review = Some(crate::LatestReview {
                id: id.clone(),
                body: body.to_owned(),
                state: crate::LatestReviewState::Given(event),
            });
        }
        answer.map(|()| crate::SubmittedReview { id, url: None })
    }

    fn viewer_latest_review(
        &self,
        repo: &RepoId,
        number: u64,
    ) -> Result<Option<crate::LatestReview>, GhError> {
        let mut state = self.lock();
        state.record(GhCall::LatestReview {
            repo: repo.clone(),
            number,
        })?;
        Ok(state.latest_review.clone())
    }

    fn edit_pr_body(&self, repo: &RepoId, number: u64, body: &str) -> Result<(), GhError> {
        let mut state = self.lock();
        state.record(GhCall::EditPrBody {
            repo: repo.clone(),
            number,
            body: body.to_owned(),
        })?;
        if let Some(detail) = state.details.get_mut(&(repo.clone(), number)) {
            detail.body = body.to_owned();
        }
        Ok(())
    }

    fn close_pr(&self, repo: &RepoId, number: u64) -> Result<(), GhError> {
        let mut state = self.lock();
        state.record(GhCall::ClosePr {
            repo: repo.clone(),
            number,
        })?;
        state.set_state(repo, number, PrState::Closed);
        Ok(())
    }

    fn reopen_pr(&self, repo: &RepoId, number: u64) -> Result<(), GhError> {
        let mut state = self.lock();
        state.record(GhCall::ReopenPr {
            repo: repo.clone(),
            number,
        })?;
        state.set_state(repo, number, PrState::Open);
        Ok(())
    }

    fn rerun_failed(&self, repo: &RepoId, run: u64) -> Result<(), GhError> {
        self.lock().record(GhCall::RerunFailed {
            repo: repo.clone(),
            run,
        })
    }

    fn graphql_rate_limit_reset(&self, host: &str) -> Result<Option<i64>, GhError> {
        let mut state = self.lock();
        state.record(GhCall::RateLimitReset {
            host: host.to_owned(),
        })?;
        Ok(state.resets.get(host).copied())
    }

    fn create_pr(&self, repo: &RepoId, new: &NewPr) -> Result<String, GhError> {
        let mut state = self.lock();
        state.calls.push(GhCall::CreatePr {
            repo: repo.clone(),
            new: new.clone(),
        });
        if let Some(failure) = state.failures.get(&GhOp::CreatePr) {
            return Err(failure.to_error());
        }
        let prs = state.prs.entry(repo.clone()).or_default();
        let number = prs.iter().map(|pr| pr.number).max().unwrap_or(0) + 1;
        let url = format!(
            "https://{}/{}/{}/pull/{number}",
            repo.host(),
            repo.owner(),
            repo.name()
        );
        prs.push(PrSummary {
            number,
            url: url.clone(),
            title: new.title.clone(),
            state: if new.draft {
                PrState::Draft
            } else {
                PrState::Open
            },
            head_branch: new.head.clone(),
            cross_repo: false,
            checks: CheckRollup::None,
            failed: Vec::new(),
            review: None,
            updated_at: String::new(),
        });
        Ok(url)
    }
}

#[cfg(test)]
mod tests;
