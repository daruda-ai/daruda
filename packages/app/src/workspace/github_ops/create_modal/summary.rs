//! What the Create PR form says it will publish, read from git off the UI
//! thread: where it goes (repository, head → base) and what it leaves out
//! (uncommitted files: only the committed HEAD is pushed), read once; and
//! what of the branch it takes over the base typed, read for each base.

use daruda_github::RepoId;
use daruda_github::remote_parse::parse_remote;

use crate::lane::git::GitRepo;
use crate::lane::git::PushTarget;
use crate::lane::git::base::BaseProblem;

/// Where the branch pushes, as the form showed it; checked again before
/// anything is pushed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShownTarget {
    pub remote: String,
    pub url: String,
    pub branch: String,
}

impl ShownTarget {
    pub(crate) fn of(push: &PushTarget) -> Self {
        Self {
            remote: push.remote.clone(),
            url: push.url.clone(),
            branch: push.branch.clone(),
        }
    }
}

/// What the form showed about where the branch pushes, for the push to
/// check under the lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Expected {
    /// Not shown (tests, and callers without a form): not checked.
    Unchecked,
    Target(ShownTarget),
    /// The form said there is nowhere to push.
    NoTarget,
}

impl Expected {
    /// Whether `push`, read again, is what the form said.
    pub(crate) fn holds(&self, push: Option<&PushTarget>) -> bool {
        match (self, push) {
            (Self::Unchecked, _) => true,
            (Self::Target(shown), Some(push)) => *shown == ShownTarget::of(push),
            (Self::NoTarget, None) => true,
            (Self::Target(_), None) | (Self::NoTarget, Some(_)) => false,
        }
    }
}

/// Where the branch pushes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TargetRead {
    /// `repo` is the GitHub repository it names, `None` when the URL is
    /// not one.
    Pushes {
        repo: Option<RepoId>,
        shown: ShownTarget,
    },
    NoTarget,
    /// Reading it failed; the push reads it again.
    Unreadable,
}

impl TargetRead {
    pub(crate) fn expected(&self) -> Expected {
        match self {
            Self::Pushes { shown, .. } => Expected::Target(shown.clone()),
            Self::NoTarget => Expected::NoTarget,
            Self::Unreadable => Expected::Unchecked,
        }
    }

    /// The branch it pushes as, for comparing against the base.
    pub(crate) fn head(&self) -> Option<&str> {
        match self {
            Self::Pushes { shown, .. } => Some(&shown.branch),
            Self::NoTarget | Self::Unreadable => None,
        }
    }
}

/// Read once when the form opens: neither depends on the base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Publication {
    pub target: TargetRead,
    /// Files changed but not committed, untracked ones included: not part
    /// of what is pushed.
    pub uncommitted: usize,
}

/// What the branch takes over its base, as git last fetched the base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Against {
    /// Against `label`, the ref the base resolved to (`origin/main`).
    Counted {
        label: String,
        commits: u32,
        files: usize,
    },
    /// No base typed: GitHub's default branch, not known here.
    DefaultBase,
    /// The base named is not a branch git knows (not fetched, misspelt).
    BaseNotFound(String),
    /// The branch is its own base: GitHub refuses that.
    OnBase,
    /// Counting failed otherwise; the form still creates.
    Unknown,
}

pub(crate) fn read_publication(repo: &dyn GitRepo) -> Publication {
    let target = match repo.push_target() {
        Ok(Some(push)) => TargetRead::Pushes {
            repo: parse_remote(&push.url),
            shown: ShownTarget::of(&push),
        },
        Ok(None) => TargetRead::NoTarget,
        Err(_) => TargetRead::Unreadable,
    };
    let uncommitted = repo.worktree_status().map_or(0, |status| {
        let mut paths: Vec<_> = status
            .staged
            .iter()
            .chain(&status.unstaged)
            .map(|entry| entry.path.clone())
            .collect();
        paths.sort();
        paths.dedup();
        paths.len()
    });
    Publication {
        target,
        uncommitted,
    }
}

/// What `head` takes over `base` (`None`: GitHub's default branch), with
/// no fetch: the base is as git last fetched it.
pub(crate) fn read_against(repo: &dyn GitRepo, base: Option<&str>, head: Option<&str>) -> Against {
    let Some(base) = base else {
        return Against::DefaultBase;
    };
    match repo
        .base_tips(Some(base), head)
        .and_then(|tips| repo.changes_since(tips))
    {
        Ok(changes) => Against::Counted {
            label: changes.tips.base.label.clone(),
            commits: changes.commits,
            files: changes.files.len(),
        },
        Err(BaseProblem::NotFound(name)) => Against::BaseNotFound(name),
        Err(BaseProblem::OnBaseBranch) => Against::OnBase,
        Err(_) => Against::Unknown,
    }
}

#[cfg(test)]
mod tests;
