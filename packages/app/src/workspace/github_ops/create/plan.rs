//! What Create PR does for a branch, decided before anything is pushed:
//! the repository, the base as `gh` takes it, and whether the push sets an
//! upstream.

use std::path::PathBuf;

use daruda_github::remote_parse::parse_remote;
use daruda_github::{NewPr, RepoId};

use super::{CreatePrError, CreatePrRequest};
use crate::lane::git::PushTarget;

/// A base as `gh --base` takes it: the bare branch, without the push
/// remote's prefix (`origin/main` names `main` on that remote).
fn strip_remote<'a>(base: &'a str, remote: &str) -> &'a str {
    base.strip_prefix(remote)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(base)
}

/// A lane's recorded base as a default for the form: a branch, never a
/// commit, which no PR can target.
pub(super) fn base_branch(base: &str, remote: &str) -> Option<String> {
    let base = strip_remote(base, remote);
    let commit_like = (7..=40).contains(&base.len()) && base.chars().all(|c| c.is_ascii_hexdigit());
    (!base.is_empty() && !commit_like).then(|| base.to_owned())
}

/// What Create PR does, decided before anything is pushed.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct PrPlan {
    pub repo: RepoId,
    /// `-u` only for a branch with no upstream yet: an existing one (in a
    /// fork workflow, the repository it came from) stays.
    pub set_upstream: bool,
    pub new: NewPr,
}

/// Plan the PR for a branch pushing to `push`. `default_base` is the
/// repository's default branch when an answer has named it: with no base
/// typed, GitHub targets that one, so the head must not be it either.
pub(super) fn plan_pr(
    push: PushTarget,
    request: CreatePrRequest,
    default_base: Option<&str>,
    worktree: PathBuf,
) -> Result<PrPlan, CreatePrError> {
    // Pushing a review branch would copy a fork's head into this
    // repository and open a second pull request for it.
    if push.review.is_some() {
        return Err(CreatePrError::ReviewBranch);
    }
    let repo = parse_remote(&push.url).ok_or(CreatePrError::NoTarget)?;
    let base = request
        .base
        .map(|base| strip_remote(&base, &push.remote).to_owned());
    if base.as_deref().or(default_base) == Some(push.branch.as_str()) {
        return Err(CreatePrError::HeadIsBase(push.branch));
    }
    Ok(PrPlan {
        repo,
        set_upstream: !push.published && !push.has_upstream,
        new: NewPr {
            head: push.branch,
            base,
            title: request.title,
            body: request.body,
            draft: request.draft,
            worktree,
        },
    })
}

#[cfg(test)]
mod tests;
