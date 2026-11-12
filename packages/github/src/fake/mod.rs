//! A scripted [`Gh`] for tests: answers come from what the test set, calls
//! are logged as [`GhCall`]s, and no `gh` process is spawned.
//!
//! | Method | Unscripted answer |
//! |---|---|
//! | `auth` | `Ready` |
//! | `prs_for_branches` | default branch `main`, no PRs for any branch |

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::availability::GhAuth;
use crate::backend::Gh;
use crate::query::RepoPrs;
use crate::{GhError, PrSummary, RepoId};

/// A request the code under test made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhCall {
    Auth { host: String },
    PrsForBranches { repo: RepoId, branches: Vec<String> },
}

/// One [`Gh`] method, for aiming failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GhOp {
    Auth,
    PrsForBranches,
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
}

#[cfg(test)]
mod tests;
