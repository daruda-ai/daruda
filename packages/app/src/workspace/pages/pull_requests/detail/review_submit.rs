//! Submitting a review: its verdict, its text and the pending comments on
//! lines, at the head commit the detail read. Comments written on another
//! head, or on lines not within one hunk, are not sent unless left out
//! deliberately. A submission whose answer is lost is checked against the
//! viewer's latest review before anything is offered again, and only what
//! GitHub took is cleared from the drafts.

use daruda_github::{
    DiffSide, GhError, LatestReview, LatestReviewState, RepoId, ReviewEvent, ReviewThreadDraft,
};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::review_drafts::{DraftOwner, DraftSide, LineCommentDraft};
use gpui::{Context, Entity, Subscription, Window};

use super::diff_layout::LineRange;
use super::line_comments::view_side;
use super::{PrDetailId, PrDetailState};
use crate::gh_host::GhHost;
use crate::github::{DraftStore, PrPoller};
use crate::surface::strings::pull_requests as s;
use crate::ui::{InputEvent, TextareaState};
use crate::workspace::Workspace;
use crate::workspace::spawn_helpers::spawn_bg_work_and_mutate;

/// Where submitting stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum SubmitState {
    Writing,
    /// Not sent: what stands in the way.
    Blocked(SubmitBlock),
    Submitting,
    /// What `gh` said; GitHub was asked and has no such review. The drafts
    /// stay to submit again.
    Failed(String),
    /// The submission failed and whether GitHub took it could not be
    /// read: only checking again, against what was `sent`, is offered,
    /// never another submit.
    Unknown {
        reason: String,
        sent: Sent,
    },
    /// Reading GitHub again for an unknown outcome.
    Checking,
}

/// Why a review is not sent as asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum SubmitBlock {
    /// Comments written on another head or on lines not within one hunk:
    /// sent only if left out deliberately, which keeps them as drafts.
    Unanchored(usize),
    /// A review with no verdict to give, no text and no comments.
    Empty,
    /// The author cannot approve or request changes on their own pull
    /// request.
    OwnPullRequest,
    /// The pull request is not read yet.
    NotRead,
}

/// The review being written.
pub(in crate::workspace) struct ReviewSubmit {
    pub body: Entity<TextareaState>,
    pub event: ReviewEvent,
    pub state: SubmitState,
    owner: Option<DraftOwner>,
    _typing: Subscription,
}

impl std::fmt::Debug for ReviewSubmit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewSubmit")
            .field("event", &self.event)
            .field("state", &self.state)
            .finish()
    }
}

/// What was sent, to tell it among the viewer's reviews later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct Sent {
    repo: RepoId,
    number: u64,
    /// The viewer's latest review before it was sent; `None` when there
    /// was none.
    before: Option<String>,
    body: String,
    event: ReviewEvent,
    published: Vec<u64>,
}

impl Sent {
    /// Whether `latest` is this review: new since it was sent, with its
    /// text and verdict, and submitted rather than pending.
    fn is(&self, latest: &LatestReview) -> bool {
        Some(&latest.id) != self.before.as_ref()
            && super::description::same_text(latest.body.trim_end(), self.body.trim_end())
            && latest.state == LatestReviewState::Given(self.event)
    }
}

/// Submissions whose outcome is unknown, by pull request, for the whole
/// app: a detail reopened later checks them instead of sending again.
#[derive(Default)]
struct Unsettled(std::collections::HashMap<DraftOwner, Sent>);

impl gpui::Global for Unsettled {}

/// Pull requests whose review is being submitted, whichever detail or
/// window asked: one submission at a time each, even across a reopen.
#[derive(Default)]
struct Submitting(std::collections::HashSet<DraftOwner>);

impl gpui::Global for Submitting {}

/// What a submission sends, and the pending comments it publishes.
struct Submission {
    repo: RepoId,
    number: u64,
    pr: String,
    commit: String,
    event: ReviewEvent,
    body: String,
    threads: Vec<ReviewThreadDraft>,
    published: Vec<u64>,
}

/// How a submission ended.
pub(in crate::workspace) enum Submitted {
    /// GitHub took it, or its latest review shows it did though the answer
    /// was lost.
    Took,
    /// GitHub has no such review.
    Failed(GhError),
    /// It failed and GitHub could not be read to tell whether it took it.
    Unknown(GhError),
}

/// What a pending comment is sent as: its text, with any replacement text
/// as a suggestion GitHub can apply.
pub(in crate::workspace) fn thread_of(comment: &LineCommentDraft) -> ReviewThreadDraft {
    let body = match &comment.suggestion {
        Some(text) => {
            // Longer than any run of backticks inside, so the text cannot
            // close it.
            let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
            let fence = "`".repeat(longest.max(2) + 1);
            format!(
                "{}\n\n{fence}suggestion\n{text}\n{fence}",
                comment.body.trim_end()
            )
        }
        None => comment.body.clone(),
    };
    ReviewThreadDraft {
        path: comment.path.clone(),
        line: comment.end,
        side: match comment.side {
            DraftSide::Left => DiffSide::Left,
            DraftSide::Right => DiffSide::Right,
        },
        start_line: (comment.start < comment.end).then_some(comment.start),
        body,
    }
}

/// Whether `comment` can be sent at `head`: written on it, and on lines
/// the listing (when read) finds within one hunk.
fn sendable(state: &PrDetailState, comment: &LineCommentDraft, head: &str) -> bool {
    if comment.revision != head {
        return false;
    }
    let range = LineRange {
        path: comment.path.clone(),
        side: view_side(comment.side),
        start: comment.start,
        end: comment.end,
        revision: comment.revision.clone(),
    };
    state.files().and_then(|files| files.anchors(&range)) != Some(false)
}

impl Workspace {
    /// Begin writing a review, continuing its kept text.
    pub(in crate::workspace) fn open_pr_review(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .pr_detail_mut(id)
            .is_none_or(|detail| detail.review.is_some())
        {
            return;
        }
        let owner = self.pr_draft_owner(id, cx);
        let kept = owner
            .as_ref()
            .and_then(|owner| DraftStore::of(owner, cx))
            .and_then(|drafts| drafts.review_body)
            .unwrap_or_default();
        let body = crate::ui::make_markdown_prose_state(
            &kept,
            s::review_placeholder(),
            crate::ui::theme::PR_REPLY_ROWS,
            window,
            cx,
        );
        let typing = cx.subscribe(&body, move |ws, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                ws.pr_review_typed(id, cx);
            }
        });
        body.update(cx, |body, cx| body.focus(window, cx));
        if let Some(detail) = self.pr_detail_mut(id) {
            detail.review = Some(ReviewSubmit {
                body,
                event: ReviewEvent::Comment,
                state: SubmitState::Writing,
                owner,
                _typing: typing,
            });
        }
        cx.notify();
    }

    fn pr_review_typed(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        let Some(review) = self.pr_detail_mut(id).and_then(|d| d.review.as_mut()) else {
            return;
        };
        if matches!(
            review.state,
            SubmitState::Blocked(_) | SubmitState::Failed(_)
        ) {
            review.state = SubmitState::Writing;
        }
        let text = review.body.read(cx).value().to_string();
        if let Some(owner) = review.owner.clone() {
            DraftStore::set_review_body(&owner, Some(text).filter(|t| !t.trim().is_empty()), cx);
        }
        cx.notify();
    }

    pub(in crate::workspace) fn set_pr_review_event(
        &mut self,
        id: PrDetailId,
        event: ReviewEvent,
        cx: &mut Context<Self>,
    ) {
        if let Some(review) = self.pr_detail_mut(id).and_then(|d| d.review.as_mut())
            && review.state.editable()
        {
            review.event = event;
            if matches!(review.state, SubmitState::Blocked(_)) {
                review.state = SubmitState::Writing;
            }
            cx.notify();
        }
    }

    /// The verdict at `index` of the button group, if any.
    pub(in crate::workspace) fn pick_pr_review_event(
        &mut self,
        id: PrDetailId,
        index: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let event = match index {
            Some(0) => ReviewEvent::Comment,
            Some(1) => ReviewEvent::Approve,
            Some(2) => ReviewEvent::RequestChanges,
            _ => return,
        };
        self.set_pr_review_event(id, event, cx);
    }

    /// Close the review; its text stays in draft.
    pub(in crate::workspace) fn close_pr_review(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        if let Some(detail) = self.pr_detail_mut(id)
            && detail
                .review
                .as_ref()
                .is_some_and(|review| review.state.editable())
        {
            detail.review = None;
            cx.notify();
        }
    }

    /// Submit the review with every pending comment; refused while any
    /// cannot be sent.
    pub(in crate::workspace) fn submit_pr_review(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        self.submit_pr_review_leaving(id, false, cx);
    }

    /// Submit it with the comments that can be sent; the others stay in
    /// draft.
    pub(in crate::workspace) fn submit_pr_review_without_unanchored(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        self.submit_pr_review_leaving(id, true, cx);
    }

    fn submit_pr_review_leaving(
        &mut self,
        id: PrDetailId,
        leave_out: bool,
        cx: &mut Context<Self>,
    ) {
        let drafts = self
            .pr_detail_mut(id)
            .and_then(|detail| detail.review.as_ref()?.owner.clone())
            .and_then(|owner| DraftStore::of(&owner, cx));
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let Some(review) = detail.review.as_ref() else {
            return;
        };
        if !review.state.editable() {
            return;
        }
        let body = review.body.read(cx).value().to_string();
        let event = review.event;
        let viewer = review.owner.as_ref().map(|owner| owner.account.clone());
        // At the head the person reviewed: the one shown, even while a
        // refresh is out.
        let read = detail
            .pr
            .last_known()
            .map(|pr| (pr.node_id.clone(), pr.head_sha.clone(), pr.author.clone()));
        let outcome = match read {
            None => Err(SubmitBlock::NotRead),
            Some((pr, head, author)) => {
                let comments = drafts.map(|drafts| drafts.comments).unwrap_or_default();
                let (send, held): (Vec<_>, Vec<_>) = comments
                    .into_iter()
                    .partition(|comment| sendable(detail, comment, &head));
                let own = author
                    .zip(viewer)
                    .is_some_and(|(author, viewer)| author.eq_ignore_ascii_case(&viewer));
                if !held.is_empty() && !leave_out {
                    Err(SubmitBlock::Unanchored(held.len()))
                } else if own && event != ReviewEvent::Comment {
                    Err(SubmitBlock::OwnPullRequest)
                } else if event != ReviewEvent::Approve && body.trim().is_empty() && send.is_empty()
                {
                    // A comment, or a request for changes, says something.
                    Err(SubmitBlock::Empty)
                } else {
                    Ok(Submission {
                        repo: detail.item.repo.clone(),
                        number: detail.item.pr.number,
                        pr,
                        commit: head,
                        event,
                        body,
                        threads: send.iter().map(thread_of).collect(),
                        published: send.iter().map(|comment| comment.id).collect(),
                    })
                }
            }
        };
        let Some(review) = detail.review.as_mut() else {
            return;
        };
        let submission = match outcome {
            Ok(submission) => submission,
            Err(block) => {
                review.state = SubmitState::Blocked(block);
                cx.notify();
                return;
            }
        };
        let owner = review.owner.clone();
        // An earlier submission of this pull request's review, from any
        // detail, whose outcome is unknown: checked, not sent again.
        let unsettled = owner
            .as_ref()
            .and_then(|owner| cx.try_global::<Unsettled>()?.0.get(owner).cloned());
        if let Some(sent) = unsettled {
            if let Some(review) = self.pr_detail_mut(id).and_then(|d| d.review.as_mut()) {
                review.state = SubmitState::Unknown {
                    reason: s::review_unsettled(),
                    sent,
                };
            }
            cx.notify();
            return;
        }
        // One submission per pull request in the whole app: a detail
        // reopened meanwhile must not send the same review again.
        if let Some(owner) = &owner
            && !cx.default_global::<Submitting>().0.insert(owner.clone())
        {
            return;
        }
        if let Some(review) = self.pr_detail_mut(id).and_then(|d| d.review.as_mut()) {
            review.state = SubmitState::Submitting;
        }
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || submit(gh.as_ref(), submission),
            move |ws, (sent, result), cx| ws.land_pr_review(id, owner, sent, result, cx),
        )
        .detach();
        cx.notify();
    }

    /// Read GitHub again for a submission whose outcome is unknown.
    pub(in crate::workspace) fn check_pr_review(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        let Some(review) = self.pr_detail_mut(id).and_then(|d| d.review.as_mut()) else {
            return;
        };
        let SubmitState::Unknown { sent, .. } = &review.state else {
            return;
        };
        let sent = sent.clone();
        review.state = SubmitState::Checking;
        let owner = review.owner.clone();
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || {
                let result = match gh.viewer_latest_review(&sent.repo, sent.number) {
                    Ok(latest) if latest.as_ref().is_some_and(|latest| sent.is(latest)) => {
                        Submitted::Took
                    }
                    Ok(_) => Submitted::Failed(GhError::Parse(
                        "GitHub has no review with this text".into(),
                    )),
                    Err(e) => Submitted::Unknown(e),
                };
                (sent, result)
            },
            move |ws, (sent, result), cx| ws.land_pr_review(id, owner, sent, result, cx),
        )
        .detach();
        cx.notify();
    }

    /// Took (or found taken): the published comments and the text leave
    /// the drafts, and the pull request is read again. Failed: everything
    /// stays. Reported and refreshed even with the detail gone.
    fn land_pr_review(
        &mut self,
        id: PrDetailId,
        owner: Option<DraftOwner>,
        sent: Sent,
        result: Submitted,
        cx: &mut Context<Self>,
    ) {
        PrPoller::nudge(sent.repo.clone(), cx);
        let unknown = matches!(result, Submitted::Unknown(_));
        // An unknown outcome is remembered app-wide: nothing may be sent
        // again until a check settles it.
        if let Some(owner) = &owner {
            cx.default_global::<Submitting>().0.remove(owner);
            let unsettled = &mut cx.default_global::<Unsettled>().0;
            if unknown {
                unsettled.insert(owner.clone(), sent.clone());
            } else {
                unsettled.remove(owner);
            }
        }
        let failure = match result {
            Submitted::Took => {
                if let Some(owner) = &owner {
                    for comment in &sent.published {
                        DraftStore::remove_comment(owner, *comment, cx);
                    }
                    DraftStore::set_review_body(owner, None, cx);
                }
                None
            }
            Submitted::Failed(e) | Submitted::Unknown(e) => Some(e),
        };
        if let Some(detail) = self.pr_detail_mut(id)
            && let Some(review) = detail.review.as_mut()
            && matches!(
                review.state,
                SubmitState::Submitting | SubmitState::Checking
            )
        {
            match &failure {
                None => detail.review = None,
                Some(e) if unknown => {
                    review.state = SubmitState::Unknown {
                        reason: e.to_string(),
                        sent,
                    };
                }
                Some(e) => review.state = SubmitState::Failed(e.to_string()),
            }
        }
        match failure {
            None => self.refresh_pr_detail(id, cx),
            Some(e) => {
                let report = ErrorReport::new(s::review_submit_failed())
                    .severity(ErrorSeverity::Error)
                    .from_error(&e)
                    .at(file!(), line!())
                    .dedup("github.pr_review.submit")
                    .build();
                self.report_error(report, cx);
            }
        }
        cx.notify();
    }
}

/// Submit; on failure, ask whether the viewer's latest review is now this
/// one, which means GitHub took it though the answer was lost. Without the
/// latest review from before, or from after, that cannot be told: unknown.
fn submit(gh: &dyn daruda_github::Gh, submission: Submission) -> (Sent, Submitted) {
    let before = gh.viewer_latest_review(&submission.repo, submission.number);
    let mut sent = Sent {
        repo: submission.repo.clone(),
        number: submission.number,
        before: None,
        body: submission.body.clone(),
        event: submission.event,
        published: submission.published.clone(),
    };
    let answer = gh.submit_review(
        &submission.repo,
        &submission.pr,
        &submission.commit,
        submission.event,
        &submission.body,
        &submission.threads,
    );
    let error = match answer {
        Ok(_) => return (sent, Submitted::Took),
        Err(error) => error,
    };
    sent.before = match before {
        Ok(before) => before.map(|review| review.id),
        Err(_) => return (sent, Submitted::Unknown(error)),
    };
    let result = match gh.viewer_latest_review(&submission.repo, submission.number) {
        Ok(Some(latest)) if sent.is(&latest) => Submitted::Took,
        Ok(_) => Submitted::Failed(error),
        Err(_) => Submitted::Unknown(error),
    };
    (sent, result)
}

impl SubmitState {
    /// Whether the review can be changed or sent: nothing in flight, and no
    /// outcome left unknown.
    pub(in crate::workspace) fn editable(&self) -> bool {
        !matches!(
            self,
            Self::Submitting | Self::Checking | Self::Unknown { .. }
        )
    }
}

#[cfg(test)]
mod tests;
