//! Unsent pull request text, kept across restarts: a description being
//! edited, replies being written and comments on lines of the diff for a
//! review not yet submitted, each under the account that wrote it on its
//! host, the repository and the pull request; and, apart from those, the
//! fields of a pull request being created, under the branch it is for.
//! Profile-scoped
//! through the data directory; daruda is the sole writer.
//!
//! ```text
//! <data dir>/review-drafts.json
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::observability::error_report::{ErrorReport, ErrorSeverity};
use crate::observability::log_writer::LogWriter;
use crate::observability::system_info::redact_home;
use crate::persistence::{LoadOutcome, load_json_file, save_json_atomic};

/// The file's layout version: a newer one is not loaded, so fields this
/// build does not know are never written away. 2 added line comments, the
/// review's text and creation drafts; 3, a reply sent but not confirmed.
pub const SCHEMA_VERSION: u32 = 3;

/// Whose drafts, on which pull request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DraftOwner {
    pub host: String,
    /// The login that wrote them: another account on the host sees none.
    pub account: String,
    pub owner: String,
    pub name: String,
    pub number: u64,
}

/// A description being edited, with the description it began from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescriptionDraft {
    pub base: String,
    pub text: String,
}

/// A reply being written to a review thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyDraft {
    /// GitHub's id of the thread.
    pub thread: String,
    pub text: String,
    /// Sent, failed, and not yet known not to have posted: it is not sent
    /// again, even from another detail or after a restart, until a read of
    /// the thread or the person says it did not post.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent: Option<SentReply>,
}

/// A reply sent whose failure does not prove it did not post.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentReply {
    pub body: String,
    /// The thread's last comment as it was sent: only a comment after it
    /// can be this reply. `None` when the thread's end was not read.
    pub after: Option<String>,
    /// The account it went out as; `None` when not known.
    pub by: Option<String>,
    /// What `gh` said.
    pub reason: String,
    /// The request was cut off waiting: GitHub may post it yet, so a read
    /// not finding it does not prove it did not post.
    #[serde(default)]
    pub cut_off: bool,
}

/// Which version of a file a line comment's lines count in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DraftSide {
    /// The base: removed or unchanged lines, numbered as before.
    Left,
    /// The head: added or unchanged lines, numbered as now.
    Right,
}

/// A comment on lines of the diff, for a review not yet submitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineCommentDraft {
    /// Names it among this pull request's comments. The highest plus one
    /// is given to a new one, so an id frees up once its comment is gone.
    pub id: u64,
    pub path: String,
    pub side: DraftSide,
    /// First and last line, inclusive.
    pub start: u32,
    pub end: u32,
    /// The head commit the lines were read at: on another head they may
    /// point at different code.
    pub revision: String,
    pub body: String,
    /// Replacement text for the lines, sent as a suggestion.
    #[serde(default)]
    pub suggestion: Option<String>,
}

/// One pull request's drafts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrDrafts {
    pub of: DraftOwner,
    #[serde(default)]
    pub description: Option<DescriptionDraft>,
    #[serde(default)]
    pub replies: Vec<ReplyDraft>,
    #[serde(default)]
    pub comments: Vec<LineCommentDraft>,
    /// The text of the review the comments go out with.
    #[serde(default)]
    pub review_body: Option<String>,
}

impl PrDrafts {
    pub fn is_empty(&self) -> bool {
        self.description.is_none()
            && self.replies.is_empty()
            && self.comments.is_empty()
            && self.review_body.is_none()
    }

    /// An id no comment here holds.
    pub fn next_comment_id(&self) -> u64 {
        self.comments
            .iter()
            .map(|held| held.id)
            .max()
            .map_or(1, |id| id + 1)
    }
}

/// Whose pull request being created, for which branch.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CreationOwner {
    pub host: String,
    pub account: String,
    pub owner: String,
    pub name: String,
    /// The branch the pull request is for, as it is pushed.
    pub head: String,
}

/// The Create PR form's fields as the user left them. A field the user did
/// not touch is `None`; one they cleared is `Some("")`, kept as such.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreationDraft {
    pub of: CreationOwner,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub draft: bool,
    /// The template the description was filled from, if it was.
    #[serde(default)]
    pub template: Option<String>,
    /// When it was last changed, Unix seconds.
    #[serde(default)]
    pub saved_at: i64,
}

impl CreationDraft {
    /// Nothing of the user's to keep.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.body.is_none()
            && self.base.is_none()
            && !self.draft
            && self.template.is_none()
    }
}

/// Every pull request's drafts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewDrafts {
    pub schema_version: u32,
    #[serde(default)]
    pub prs: Vec<PrDrafts>,
    /// Pull requests being created, one per branch.
    #[serde(default)]
    pub creations: Vec<CreationDraft>,
}

impl Default for ReviewDrafts {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            prs: Vec::new(),
            creations: Vec::new(),
        }
    }
}

impl ReviewDrafts {
    pub fn of(&self, owner: &DraftOwner) -> Option<&PrDrafts> {
        self.prs.iter().find(|held| held.of == *owner)
    }

    /// `owner`'s drafts to change; dropped again by [`Self::prune`] when
    /// left empty.
    pub fn entry(&mut self, owner: &DraftOwner) -> &mut PrDrafts {
        let at = match self.prs.iter().position(|held| held.of == *owner) {
            Some(at) => at,
            None => {
                self.prs.push(PrDrafts {
                    of: owner.clone(),
                    description: None,
                    replies: Vec::new(),
                    comments: Vec::new(),
                    review_body: None,
                });
                self.prs.len() - 1
            }
        };
        &mut self.prs[at]
    }

    /// Drop pull requests with nothing left in draft.
    pub fn prune(&mut self) {
        self.prs.retain(|held| !held.is_empty());
        self.creations.retain(|held| !held.is_empty());
    }

    pub fn creation(&self, owner: &CreationOwner) -> Option<&CreationDraft> {
        self.creations.iter().find(|held| held.of == *owner)
    }

    /// Keep `draft` for its branch, or with `None` drop `owner`'s.
    pub fn set_creation(&mut self, owner: &CreationOwner, draft: Option<CreationDraft>) {
        self.creations.retain(|held| held.of != *owner);
        self.creations.extend(draft);
    }
}

/// `review-drafts.json` under `data_dir`.
pub fn review_drafts_path_in(data_dir: &Path) -> PathBuf {
    data_dir.join("review-drafts.json")
}

/// The drafts under `data_dir`: none when the file is missing. `None` for
/// a damaged file or one from a newer daruda, which must not be replaced
/// by an empty set.
pub fn load_review_drafts_in(data_dir: &Path) -> Option<ReviewDrafts> {
    let path = review_drafts_path_in(data_dir);
    let drafts = match load_json_file::<ReviewDrafts>("review-drafts", &path) {
        LoadOutcome::Parsed(drafts) => drafts,
        LoadOutcome::Missing => return Some(ReviewDrafts::default()),
        LoadOutcome::Corrupt => return None,
    };
    if drafts.schema_version > SCHEMA_VERSION {
        LogWriter::log(
            ErrorReport::new("review-drafts.json from a newer daruda — refusing to load")
                .severity(ErrorSeverity::Warning)
                .at(file!(), line!())
                .with_context("path", redact_home(&path))
                .with_context("found", drafts.schema_version.to_string())
                .with_context("supported", SCHEMA_VERSION.to_string())
                .dedup("review_drafts.schema.too_new")
                .build(),
        );
        return None;
    }
    // An older layout reads as this one; it is written back as this one.
    let mut drafts = drafts;
    drafts.schema_version = SCHEMA_VERSION;
    Some(drafts)
}

/// Save atomically: a tempfile in the same directory, then renamed in.
pub fn save_review_drafts_in(data_dir: &Path, drafts: &ReviewDrafts) -> std::io::Result<()> {
    save_json_atomic(data_dir, &review_drafts_path_in(data_dir), drafts)
}

/// The profile's data directory, where the drafts live.
pub fn review_drafts_dir() -> PathBuf {
    crate::persistence::default_data_dir()
}

#[cfg(test)]
mod tests;
