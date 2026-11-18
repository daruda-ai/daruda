//! The process-wide store of unsent review text: what every window reads
//! and writes, saved shortly after each change and at quit. Installed by the
//! primary desktop instance; without it (tests) drafts are kept in memory
//! only, so no test touches the real profile.

use std::path::PathBuf;
use std::time::Duration;

use daruda_github::RepoId;
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use daruda_store::review_drafts::{
    CreationDraft, CreationOwner, DescriptionDraft, DraftOwner, LineCommentDraft, PrDrafts,
    ReplyDraft, ReviewDrafts, load_review_drafts_in, review_drafts_dir, save_review_drafts_in,
};
use gpui::{App, Global, Task};

use super::PrPoller;

/// How long typing settles before the drafts are written.
const SAVE_AFTER: Duration = Duration::from_millis(400);

/// Whether what is typed is kept across restarts, as the editor says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DraftKeeping {
    /// Written to the profile's drafts file.
    #[default]
    Saved,
    /// The drafts file could not be read (damaged, or from a newer daruda)
    /// and is left alone: drafts last until quit.
    MemoryOnly,
    /// Writing the drafts file failed; drafts last until quit unless a
    /// later write succeeds.
    SaveFailed,
    /// The account signed in on the host is not known yet: nothing to keep
    /// the drafts under.
    NoAccount,
}

#[derive(Default)]
pub(crate) struct DraftStore {
    drafts: ReviewDrafts,
    /// Where they are saved; `None` keeps them in memory.
    dir: Option<PathBuf>,
    /// Changed since last written.
    dirty: bool,
    /// The last write failed.
    failed: bool,
    /// Pull requests whose review is being submitted, whichever detail or
    /// window asked: one submission at a time each, even across a reopen.
    submitting: std::collections::HashSet<DraftOwner>,
    pending: Option<Task<()>>,
}

impl Global for DraftStore {}

impl DraftStore {
    /// Load the profile's drafts and save there from now on. A damaged or
    /// newer file is left alone: drafts are then kept in memory only, never
    /// written over it.
    pub(crate) fn install(cx: &mut App) {
        let dir = review_drafts_dir();
        let store = match load_review_drafts_in(&dir) {
            Some(drafts) => Self {
                drafts,
                dir: Some(dir),
                ..Self::default()
            },
            None => Self::default(),
        };
        cx.set_global(store);
    }

    fn global_mut(cx: &mut App) -> &mut Self {
        cx.default_global::<Self>()
    }

    /// Whose drafts on `repo#number`: the account signed in on its host.
    /// `None` until that account is known.
    pub(crate) fn owner(repo: &RepoId, number: u64, cx: &App) -> Option<DraftOwner> {
        let account = PrPoller::account(repo.host(), cx)?;
        Some(DraftOwner {
            host: repo.host().to_owned(),
            account: account.login,
            owner: repo.owner().to_owned(),
            name: repo.name().to_owned(),
            number,
        })
    }

    /// Whose pull request being created for `head` of `repo`: the account
    /// signed in on its host. `None` until that account is known.
    pub(crate) fn creation_owner(repo: &RepoId, head: &str, cx: &App) -> Option<CreationOwner> {
        let account = PrPoller::account(repo.host(), cx)?;
        Some(CreationOwner {
            host: repo.host().to_owned(),
            account: account.login,
            owner: repo.owner().to_owned(),
            name: repo.name().to_owned(),
            head: head.to_owned(),
        })
    }

    pub(crate) fn creation(owner: &CreationOwner, cx: &App) -> Option<CreationDraft> {
        cx.try_global::<Self>()?.drafts.creation(owner).cloned()
    }

    /// Keep, or with `None` drop, the Create PR fields for `owner`'s branch.
    pub(crate) fn set_creation(owner: &CreationOwner, draft: Option<CreationDraft>, cx: &mut App) {
        Self::global_mut(cx).drafts.set_creation(owner, draft);
        Self::changed(cx);
    }

    /// Whether drafts under `owner` (`None`: no account known) are kept
    /// across restarts.
    pub(crate) fn keeping<T>(owner: Option<&T>, cx: &App) -> DraftKeeping {
        let Some(store) = cx.try_global::<Self>() else {
            // No store installed: tests, where nothing is promised.
            return if owner.is_some() {
                DraftKeeping::Saved
            } else {
                DraftKeeping::NoAccount
            };
        };
        match (owner, store.dir.is_some(), store.failed) {
            (None, _, _) => DraftKeeping::NoAccount,
            (Some(_), false, _) => DraftKeeping::MemoryOnly,
            (Some(_), true, true) => DraftKeeping::SaveFailed,
            (Some(_), true, false) => DraftKeeping::Saved,
        }
    }

    pub(crate) fn of(owner: &DraftOwner, cx: &App) -> Option<PrDrafts> {
        cx.try_global::<Self>()?.drafts.of(owner).cloned()
    }

    /// Set, or with `None` drop, the description draft.
    pub(crate) fn set_description(
        owner: &DraftOwner,
        draft: Option<DescriptionDraft>,
        cx: &mut App,
    ) {
        let store = Self::global_mut(cx);
        store.drafts.entry(owner).description = draft;
        Self::changed(cx);
    }

    /// Set, or with `None` drop, the reply draft to `thread`.
    pub(crate) fn set_reply(owner: &DraftOwner, thread: &str, text: Option<String>, cx: &mut App) {
        let store = Self::global_mut(cx);
        let replies = &mut store.drafts.entry(owner).replies;
        replies.retain(|held| held.thread != thread);
        if let Some(text) = text {
            replies.push(ReplyDraft {
                thread: thread.to_owned(),
                text,
            });
        }
        Self::changed(cx);
    }

    /// Keep `comment` as comment `id`, or as a new one with `None`.
    /// Answers the id it is kept under.
    pub(crate) fn put_comment(
        owner: &DraftOwner,
        id: Option<u64>,
        mut comment: LineCommentDraft,
        cx: &mut App,
    ) -> u64 {
        let store = Self::global_mut(cx);
        let entry = store.drafts.entry(owner);
        comment.id = id.unwrap_or_else(|| entry.next_comment_id());
        let id = comment.id;
        match entry.comments.iter_mut().find(|held| held.id == id) {
            Some(held) => *held = comment,
            None => entry.comments.push(comment),
        }
        Self::changed(cx);
        id
    }

    /// Mark `owner`'s review as being submitted; `false` if it already is.
    pub(crate) fn begin_submit(owner: &DraftOwner, cx: &mut App) -> bool {
        Self::global_mut(cx).submitting.insert(owner.clone())
    }

    pub(crate) fn end_submit(owner: &DraftOwner, cx: &mut App) {
        Self::global_mut(cx).submitting.remove(owner);
    }

    /// Set, or with `None` drop, the review's text.
    pub(crate) fn set_review_body(owner: &DraftOwner, body: Option<String>, cx: &mut App) {
        Self::global_mut(cx).drafts.entry(owner).review_body = body;
        Self::changed(cx);
    }

    /// Drop the line comment `id`.
    pub(crate) fn remove_comment(owner: &DraftOwner, id: u64, cx: &mut App) {
        Self::global_mut(cx)
            .drafts
            .entry(owner)
            .comments
            .retain(|held| held.id != id);
        Self::changed(cx);
    }

    fn changed(cx: &mut App) {
        let store = Self::global_mut(cx);
        store.drafts.prune();
        store.dirty = true;
        if store.dir.is_none() || store.pending.is_some() {
            return;
        }
        let pending = cx.spawn(async move |cx| {
            cx.background_executor().timer(SAVE_AFTER).await;
            cx.update(Self::flush);
        });
        Self::global_mut(cx).pending = Some(pending);
    }

    /// Write what changed now: when typing settles, on leaving a detail
    /// and at quit.
    pub(crate) fn flush(cx: &mut App) {
        let Some(store) = cx.try_global::<Self>() else {
            return;
        };
        let (Some(dir), true) = (store.dir.clone(), store.dirty) else {
            return;
        };
        let drafts = store.drafts.clone();
        let store = Self::global_mut(cx);
        store.pending = None;
        match save_review_drafts_in(&dir, &drafts) {
            Ok(()) => {
                store.dirty = false;
                store.failed = false;
            }
            Err(e) => {
                store.failed = true;
                LogWriter::log(
                    ErrorReport::new(crate::surface::strings::pull_requests::drafts_save_failed())
                        .severity(ErrorSeverity::Error)
                        .from_error(&e)
                        .at(file!(), line!())
                        .dedup("github.review_drafts.save")
                        .build(),
                );
            }
        }
    }

    /// Point a test's store at `dir`.
    #[cfg(test)]
    pub(crate) fn install_at(dir: PathBuf, cx: &mut App) {
        let drafts = load_review_drafts_in(&dir).unwrap_or_default();
        cx.set_global(Self {
            drafts,
            dir: Some(dir),
            ..Self::default()
        });
    }
}
