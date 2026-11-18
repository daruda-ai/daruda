//! Reviews: comments, threads, submitted reviews and who is asked.

use super::*;

/// One comment in a review thread.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewComment {
    /// GitHub's node id: what a reply or a reaction names.
    pub id: String,
    /// The comment on GitHub's page, when GitHub gave it.
    pub url: Option<String>,
    /// `None` for a deleted account.
    pub author: Option<String>,
    pub body: String,
}

impl ReviewComment {
    /// The comment's page, if GitHub gave a web page for it.
    pub fn web_url(&self) -> Option<&str> {
        self.url.as_deref().filter(|url| is_web_url(url))
    }
}

/// Whether `name` can be a reviewer GitHub knows: a user login (letters,
/// digits, hyphens and, for managed enterprise accounts, `_`) or an
/// `org/team` slug. Malformed text, such as something starting with `-`,
/// never reaches `gh`'s arguments; GitHub judges the rest.
pub fn reviewer_name_ok(name: &str) -> bool {
    let login = |part: &str| {
        !part.is_empty()
            && !part.starts_with('-')
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    };
    match name.split_once('/') {
        None => login(name),
        Some((org, team)) => {
            login(org)
                && !team.is_empty()
                && !team.starts_with(['-', '.'])
                && team
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        }
    }
}

/// The side of a diff a review comment's lines are on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffSide {
    /// The base: removed or unchanged lines, numbered as before.
    Left,
    /// The head: added or unchanged lines, numbered as now.
    #[default]
    Right,
}

/// An unresolved review thread on a pull request's diff.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewThread {
    /// GitHub's node id: what a reply, resolve or unresolve names.
    pub id: String,
    pub path: String,
    /// The commented lines, first to last; `None` for a file-level comment.
    pub lines: Option<(u32, u32)>,
    /// Which side of the diff `lines` count on.
    pub side: DiffSide,
    /// The diff moved since; `lines` are where it was written.
    pub outdated: bool,
    pub comments: Vec<ReviewComment>,
    /// The thread has more replies than are read for one thread.
    pub more_comments: bool,
    /// What the viewer may do to it.
    pub may: ThreadPermissions,
}

/// What the viewer may do to a review thread, as GitHub says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadPermissions {
    pub reply: bool,
    pub resolve: bool,
    pub unresolve: bool,
}

/// Allowed until GitHub says otherwise: an answer without the fields
/// leaves the actions to GitHub to refuse.
impl Default for ThreadPermissions {
    fn default() -> Self {
        Self {
            reply: true,
            resolve: true,
            unresolve: true,
        }
    }
}

/// A review's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewEvent {
    Comment,
    Approve,
    RequestChanges,
}

/// A comment on lines of the diff, sent with a review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewThreadDraft {
    pub path: String,
    /// The last line, on `side`.
    pub line: u32,
    pub side: DiffSide,
    /// The first line, for a range of more than one.
    pub start_line: Option<u32>,
    pub body: String,
}

/// A review GitHub took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedReview {
    pub id: String,
    pub url: Option<String>,
}

/// The viewer's latest review: what tells whether a submission whose
/// answer was lost went through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestReview {
    pub id: String,
    pub body: String,
    /// What GitHub records: `PENDING` (started, not submitted), or the
    /// verdict given.
    pub state: LatestReviewState,
}

/// Where the viewer's latest review stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatestReviewState {
    Pending,
    Given(ReviewEvent),
    /// Dismissed, or a state this crate does not know.
    Other,
}

/// The comments on a pull request itself, oldest first: its conversation,
/// apart from the review threads on its diff.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Conversation {
    pub comments: Vec<ReviewComment>,
    /// GitHub had more comments than are read for one pull request.
    pub truncated: bool,
}

/// The unresolved threads of a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewThreads {
    pub threads: Vec<ReviewThread>,
    /// GitHub had more threads than are read for one pull request.
    pub truncated: bool,
}

/// Where one reviewer stands on a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewerState {
    /// Asked to review and has not reviewed since.
    Requested,
    Approved,
    ChangesRequested,
    Commented,
    /// The latest review was dismissed.
    Dismissed,
    /// A review started and not yet submitted.
    Pending,
    /// A review state this crate does not know.
    Other,
}
