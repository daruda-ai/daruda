//! A scripted [`Gh`] for tests: answers come from what the test set, calls
//! are logged as [`GhCall`]s, and no `gh` process is spawned.
//!
//! | Method | Unscripted answer |
//! |---|---|
//! | `auth` | `Ready` |
//! | `prs_for_branches` | default branch `main`, no PRs for any branch |
//! | `search_prs` | none, not truncated |
//! | `create_pr` | adds an open PR numbered after the repo's last, answers its URL |
//! | `failed_log` | exit 1, as for a run that has no failed log |
//! | `review_threads` | none |

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::availability::GhAuth;
use crate::backend::Gh;
use crate::query::{RepoPrs, newest_first};
use crate::{
    ActionsJob, CheckRollup, GhError, NewPr, PrFilter, PrListItem, PrSearch, PrState, PrSummary,
    RepoId, ReviewThreads,
};

/// A request the code under test made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhCall {
    Auth {
        host: String,
    },
    PrsForBranches {
        repo: RepoId,
        branches: Vec<String>,
    },
    SearchPrs {
        host: String,
        repos: Vec<RepoId>,
        filter: PrFilter,
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
}

/// One [`Gh`] method, for aiming failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GhOp {
    Auth,
    PrsForBranches,
    SearchPrs,
    CreatePr,
    FailedLog,
    ReviewThreads,
}

/// A cloneable description of a [`GhError`], turned into a fresh error on
/// every call it fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhFailure {
    NotFound,
    Exit { code: Option<i32>, stderr: String },
    Parse(String),
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
    default_branches: HashMap<RepoId, String>,
    prs: HashMap<RepoId, Vec<PrSummary>>,
    lists: HashMap<PrFilter, Vec<PrListItem>>,
    truncated: HashMap<PrFilter, bool>,
    logs: HashMap<(u64, u64), String>,
    threads: HashMap<u64, ReviewThreads>,
    failures: HashMap<GhOp, GhFailure>,
    calls: Vec<GhCall>,
}

impl FakeGh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_auth(&self, host: &str, auth: GhAuth) {
        self.lock().auth.insert(host.to_owned(), auth);
    }

    pub fn set_default_branch(&self, repo: &RepoId, branch: &str) {
        self.lock()
            .default_branches
            .insert(repo.clone(), branch.to_owned());
    }

    /// The PRs `repo` has; `prs_for_branches` answers each branch with those
    /// whose head is that branch.
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
            Self::CreatePr { .. } => GhOp::CreatePr,
            Self::FailedLog { .. } => GhOp::FailedLog,
            Self::ReviewThreads { .. } => GhOp::ReviewThreads,
        }
    }
}

impl Gh for FakeGh {
    fn auth(&self, host: &str) -> Result<GhAuth, GhError> {
        let mut state = self.lock();
        state.calls.push(GhCall::Auth {
            host: host.to_owned(),
        });
        if let Some(failure) = state.failures.get(&GhOp::Auth) {
            return Err(failure.to_error());
        }
        Ok(state.auth.get(host).copied().unwrap_or(GhAuth::Ready))
    }

    fn prs_for_branches(&self, repo: &RepoId, branches: &[String]) -> Result<RepoPrs, GhError> {
        let mut state = self.lock();
        state.calls.push(GhCall::PrsForBranches {
            repo: repo.clone(),
            branches: branches.to_vec(),
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
        Ok(RepoPrs {
            default_branch: Some(
                state
                    .default_branches
                    .get(repo)
                    .cloned()
                    .unwrap_or_else(|| "main".to_owned()),
            ),
            by_branch,
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
        Ok(state.threads.get(&number).cloned().unwrap_or_default())
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
