//! How a pull request's head is put in a local worktree. A pull request
//! whose branch lives in its own repository is checked out on that branch,
//! so pushing updates it; one from a fork gets a local review branch
//! `pr-<N>`, since the base repository's remote cannot push to the fork's
//! branch, and that branch carries a [`ReviewMark`] naming its pull request.

use crate::PrListItem;
use crate::link::ReviewMark;

/// The branch a worktree made to review fork pull request `number` is on.
fn review_branch(number: u64) -> String {
    format!("pr-{number}")
}

/// What git is asked for to put a pull request's head in a worktree,
/// decided before any of it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrCheckout {
    /// The remote the base repository is reached through.
    pub remote: String,
    /// What `git fetch <remote>` is given. It names its destination, so the
    /// fetched commit has a name `git worktree add` can start from: a plain
    /// `git fetch <remote> <head>` only moves `<remote>/<head>` when the
    /// remote's configured refspec covers the branch, which a single-branch
    /// clone's does not, and `FETCH_HEAD` is rewritten by any other fetch.
    pub refspec: String,
    /// The local branch the worktree is on.
    pub branch: String,
    /// The fetched ref `branch` is brought to.
    pub start: String,
    /// For a fork's pull request, the encoded mark its review branch carries.
    pub review: Option<String>,
}

impl PrCheckout {
    /// The checkout of `item`, whose base repository `remote` reaches.
    pub fn of(item: &PrListItem, remote: &str) -> Self {
        let number = item.pr.number;
        let (refspec, branch, start, review) = if item.pr.cross_repo {
            // GitHub keeps every pull request's head in the base repository
            // under `refs/pull/<N>/head`; a private namespace keeps it out of
            // the branch lists.
            let start = format!("refs/daruda/pr/{number}");
            let mark = ReviewMark {
                repo: item.repo.clone(),
                number,
            };
            (
                format!("+refs/pull/{number}/head:{start}"),
                review_branch(number),
                start,
                Some(mark.encode()),
            )
        } else {
            let head = &item.pr.head_branch;
            let start = format!("refs/remotes/{remote}/{head}");
            (
                format!("+refs/heads/{head}:{start}"),
                head.clone(),
                start,
                None,
            )
        };
        Self {
            remote: remote.to_owned(),
            refspec,
            branch,
            start,
            review,
        }
    }
}
