//! A pull request as a list or a lane sees it, and one to create.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Draft,
    Merged,
    Closed,
}

impl PrState {
    /// Open or draft: the PR still takes pushes and runs checks.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Open | Self::Draft)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

/// A pull request to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPr {
    /// The branch on the repository's side, already pushed.
    pub head: String,
    /// `None` lets GitHub pick the repository's default branch.
    pub base: Option<String>,
    pub title: String,
    pub body: String,
    pub draft: bool,
    /// The lane's worktree; `gh` runs there so it reads the same remotes.
    pub worktree: std::path::PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrSummary {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub state: PrState,
    pub head_branch: String,
    /// The head lives in another repository (a fork).
    pub cross_repo: bool,
    pub checks: CheckRollup,
    /// The failing checks in hand behind [`CheckRollup::Failing`]; with a
    /// truncated list, those on its first page.
    pub failed: Vec<FailedCheck>,
    pub review: Option<ReviewDecision>,
    /// RFC 3339.
    pub updated_at: String,
}

/// One pull request in a list search, with the repository it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrListItem {
    pub repo: RepoId,
    pub pr: PrSummary,
    /// `None` for a deleted account.
    pub author: Option<String>,
    /// The owner of the repository the head branch lives in: the fork's
    /// owner for a cross-repository PR. `None` once the fork is deleted.
    pub head_owner: Option<String>,
    pub base_branch: String,
    /// Conversation comments, not review comments on the diff.
    pub comments: u32,
}

/// A list gathered from search pages.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrSearch {
    /// Most recently updated first.
    pub items: Vec<PrListItem>,
    /// More pull requests match than the list holds: pages not read yet, or
    /// past what GitHub's search answers.
    pub truncated: bool,
}

impl PrSummary {
    /// The PR's page, if `url` is one a browser may be handed: whatever host
    /// answered, a `file:` or script URL must not reach the system opener.
    pub fn web_url(&self) -> Option<&str> {
        let url = self.url.as_str();
        is_web_url(url).then_some(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_open_and_draft_prs_are_active() {
        let active: Vec<_> = [
            PrState::Open,
            PrState::Draft,
            PrState::Merged,
            PrState::Closed,
        ]
        .into_iter()
        .map(PrState::is_active)
        .collect();
        assert_eq!(active, [true, true, false, false]);
    }
}
