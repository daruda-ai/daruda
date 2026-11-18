//! Checking a pull request's head out into a new worktree: fetching it,
//! bringing the local branch to it unless that drops the person's commits,
//! and adding the worktree. GPUI-free; runs off the UI thread.

use daruda_github::remote_parse::parse_remote;
use daruda_github::{PrCheckout, PrListItem};

use crate::surface::strings::pull_requests as s;

use crate::lane::git::{GitError, GitRepo, LaneStart, LocalBranch};

/// Where the worktree's branch was left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CheckedOut {
    /// At the pull request's fetched head: made there, or moved there from a
    /// commit that head, or the one fetched before it, already held.
    AtHead,
    /// Where it was: it has commits of its own the head lacks, which moving
    /// it would drop.
    KeptLocal,
    /// A worktree made for it before and never registered (its window or
    /// project closed while git ran): checked again and taken as it is,
    /// `kept_local` when its branch holds commits the fetched head lacks.
    Existing { kept_local: bool },
}

/// How far opening a worktree for a pull request has got, as its button
/// says while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum CheckoutStage {
    Fetching,
    Creating,
}

#[derive(Debug)]
pub(super) enum CheckoutError {
    /// A branch of the review branch's name exists that was not made to
    /// review this pull request; it is the person's, and is left alone.
    Foreign(String),
    /// A worktree already stands where this one goes, on another branch or
    /// reviewing another pull request: it is left alone.
    Occupied(std::path::PathBuf),
    Git(GitError),
}

impl std::fmt::Display for CheckoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Foreign(branch) => write!(f, "{branch} exists and reviews no such pull request"),
            Self::Occupied(path) => write!(f, "another worktree stands at {}", path.display()),
            Self::Git(e) => e.fmt(f),
        }
    }
}

impl From<GitError> for CheckoutError {
    fn from(e: GitError) -> Self {
        Self::Git(e)
    }
}

/// Fetch the pull request's head, bring the local branch to it unless that
/// would drop commits of the person's, then add the worktree on the branch.
/// Blocking; runs off the UI thread.
pub(super) fn check_out(
    repo: &dyn GitRepo,
    checkout: &PrCheckout,
    push_url: &str,
    new_path: &std::path::Path,
    progress: &dyn Fn(CheckoutStage),
) -> Result<CheckedOut, CheckoutError> {
    let (branch, head) = (checkout.branch.as_str(), checkout.start.as_str());
    // One made before and left unregistered is taken, never made twice,
    // once it is checked to be this pull request's: same branch, and for a
    // review branch, the same mark; then its head is fetched and the branch
    // measured against it. Only a directory that is there counts: git keeps
    // listing one deleted since.
    let left = if new_path.exists() {
        repo.worktrees()?
            .into_iter()
            .find(|worktree| daruda_core::path::same_path(&worktree.path, new_path))
    } else {
        None
    };
    if let Some(left) = left {
        let ours = left.branch.as_deref() == Some(branch)
            && match &checkout.review {
                Some(mark) => repo.review_mark(branch)?.as_ref() == Some(mark),
                None => true,
            };
        if !ours {
            return Err(CheckoutError::Occupied(left.path));
        }
        progress(CheckoutStage::Fetching);
        repo.fetch_ref(
            fetch_source(repo, &checkout.remote, push_url)?,
            &checkout.refspec,
        )?;
        // Checked out there, the branch cannot be moved: it is said when it
        // holds commits of its own.
        let kept_local = repo.local_branch(branch, head)? == LocalBranch::Ahead;
        return Ok(CheckedOut::Existing { kept_local });
    }
    // Measured before the fetch as well: after a force push the new head no
    // longer holds the old one, yet a branch inside the old one has nothing
    // of the person's to lose.
    let before = repo.local_branch(branch, head)?;
    if let Some(mark) = &checkout.review
        && before != LocalBranch::Missing
        && repo.review_mark(branch)?.as_ref() != Some(mark)
    {
        return Err(CheckoutError::Foreign(branch.to_owned()));
    }
    progress(CheckoutStage::Fetching);
    repo.fetch_ref(
        fetch_source(repo, &checkout.remote, push_url)?,
        &checkout.refspec,
    )?;
    let at_head = match before {
        LocalBranch::Missing | LocalBranch::Contained => true,
        LocalBranch::Ahead => repo.local_branch(branch, head)? == LocalBranch::Contained,
    };
    if at_head {
        repo.move_branch(branch, head)?;
    }
    // Only once the branch exists: its config section goes with it when the
    // branch is deleted, so a failed checkout leaves no orphaned key.
    if before == LocalBranch::Missing && checkout.review.is_none() {
        repo.set_push_remote(branch, &checkout.remote)?;
    }
    if let Some(mark) = &checkout.review
        && before == LocalBranch::Missing
    {
        repo.set_review_mark(branch, mark)?;
    }
    progress(CheckoutStage::Creating);
    repo.add_lane(new_path, LaneStart::existing_branch(branch))?;
    Ok(if at_head {
        CheckedOut::AtHead
    } else {
        CheckedOut::KeptLocal
    })
}

/// Where to fetch the pull request's ref from. The page lists the push
/// repository, so the named remote is used only when its fetch URL names that
/// same repository — then the remote's own transport, proxy and credentials
/// apply. A remote that fetches elsewhere falls back to the push URL.
fn fetch_source<'a>(
    repo: &dyn GitRepo,
    remote: &'a str,
    push_url: &'a str,
) -> Result<&'a str, GitError> {
    let fetches_pushed_repo = repo
        .remote_url(remote)?
        .and_then(|url| parse_remote(&url))
        .is_some_and(|fetched| parse_remote(push_url) == Some(fetched));
    Ok(if fetches_pushed_repo {
        remote
    } else {
        push_url
    })
}

/// The lane description of a worktree made for `item`; `None` for one on
/// the pull request's own branch, which says what it is already.
pub(super) fn description(item: &PrListItem) -> Option<String> {
    // GitHub answers no head owner once the fork is deleted; the author,
    // who opened the PR from it, is the closest name left.
    let owner = item
        .head_owner
        .clone()
        .or_else(|| item.author.clone())
        .unwrap_or_else(s::author_deleted);
    item.pr
        .cross_repo
        .then(|| s::review_lane_description(item.pr.number, owner, &item.pr.head_branch))
}

#[cfg(test)]
mod tests;
