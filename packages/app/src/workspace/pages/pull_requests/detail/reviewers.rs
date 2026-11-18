//! Asking for reviews from the detail, for a viewer GitHub lets edit the
//! pull request: request reviewers by login or `org/team`, or stop asking
//! one not yet reviewed. One change at a time per detail; a failure keeps
//! what was typed, and asking twice is harmless, so trying again is safe.

use daruda_github::{GhError, RepoId, reviewer_name_ok};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use gpui::{AppContext as _, Context, Entity, Subscription, Window};

use super::{PrDetailId, PrDetailState};
use crate::gh_host::GhHost;
use crate::github::PrPoller;
use crate::surface::strings::pull_requests as s;
use crate::ui::{InputEvent, InputState};
use crate::workspace::Workspace;
use crate::workspace::spawn_helpers::spawn_bg_work_and_mutate;

/// Where the last change to the reviewers stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum ReviewerOp {
    Running,
    /// What was typed is not a login or `org/team`: the first that is not.
    Invalid(String),
    /// The pull request's author, whom GitHub does not let review it.
    Author(String),
    /// What `gh` said.
    Failed(String),
}

/// The reviewer box, open once asked for, and the last change.
#[derive(Default)]
pub(in crate::workspace) struct ReviewerRequests {
    pub input: Option<Entity<InputState>>,
    /// Enter asks; typing clears a line about the text before.
    typing: Option<Subscription>,
    pub op: Option<ReviewerOp>,
}

impl std::fmt::Debug for ReviewerRequests {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewerRequests")
            .field("input", &self.input.as_ref().map(Entity::entity_id))
            .field("op", &self.op)
            .finish()
    }
}

/// The names typed, split on commas and spaces, or the first that cannot be
/// a reviewer.
pub(in crate::workspace) fn reviewer_names(text: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for name in text
        .split([',', ' ', '\t', '\n'])
        .map(|name| name.trim_start_matches('@'))
        .filter(|name| !name.is_empty())
    {
        if !reviewer_name_ok(name) {
            return Err(name.to_owned());
        }
        if !names
            .iter()
            .any(|held: &String| held.eq_ignore_ascii_case(name))
        {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

impl PrDetailState {
    pub(in crate::workspace) fn reviewer_requests(&self) -> &ReviewerRequests {
        &self.reviewer_requests
    }

    /// The viewer may change who is asked, and no change runs.
    fn reviewers_editable(&self) -> bool {
        self.pr.last_known().is_some_and(|pr| pr.can_update)
            && self.reviewer_requests.op != Some(ReviewerOp::Running)
    }
}

impl Workspace {
    /// Open the box a reviewer is typed in, with the keyboard in it.
    pub(in crate::workspace) fn open_pr_reviewer_input(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .pr_detail_mut(id)
            .is_none_or(|detail| detail.reviewer_requests.input.is_some())
        {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(s::reviewer_placeholder()));
        let typing = cx.subscribe(&input, move |ws, _, event: &InputEvent, cx| match event {
            InputEvent::PressEnter { .. } => ws.request_pr_reviewers(id, cx),
            InputEvent::Change => ws.pr_reviewers_typed(id, cx),
            _ => {}
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        if let Some(detail) = self.pr_detail_mut(id) {
            detail.reviewer_requests.input = Some(input);
            detail.reviewer_requests.typing = Some(typing);
        }
        cx.notify();
    }

    /// Close the reviewer box, dropping what was typed; not while a change
    /// runs.
    pub(in crate::workspace) fn close_pr_reviewer_input(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        if let Some(detail) = self.pr_detail_mut(id)
            && detail.reviewer_requests.op != Some(ReviewerOp::Running)
        {
            detail.reviewer_requests = ReviewerRequests::default();
            cx.notify();
        }
    }

    /// A line about the text typed before no longer applies.
    fn pr_reviewers_typed(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        if let Some(requests) = self.pr_detail_mut(id).map(|d| &mut d.reviewer_requests)
            && matches!(
                requests.op,
                Some(ReviewerOp::Invalid(_) | ReviewerOp::Author(_) | ReviewerOp::Failed(_))
            )
        {
            requests.op = None;
            cx.notify();
        }
    }

    /// Ask those typed in the box to review.
    pub(in crate::workspace) fn request_pr_reviewers(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self
            .pr_detail_mut(id)
            .and_then(|detail| detail.reviewer_requests.input.as_ref())
            .map(|input| input.read(cx).value().to_string())
        else {
            return;
        };
        let author = self
            .pr_detail_mut(id)
            .and_then(|detail| detail.pr.last_known())
            .and_then(|pr| pr.author.clone());
        let refused = match reviewer_names(&text) {
            Ok(names) if names.is_empty() => return,
            Ok(names) => match names.iter().find(|name| {
                author
                    .as_deref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(name))
            }) {
                // GitHub refuses the whole request for one author in it.
                Some(name) => ReviewerOp::Author(name.clone()),
                None => return self.change_pr_reviewers(id, names, Vec::new(), cx),
            },
            Err(bad) => ReviewerOp::Invalid(bad),
        };
        if let Some(detail) = self.pr_detail_mut(id) {
            detail.reviewer_requests.op = Some(refused);
        }
        cx.notify();
    }

    /// Stop asking `name`, who has not reviewed yet.
    pub(in crate::workspace) fn remove_pr_reviewer(
        &mut self,
        id: PrDetailId,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        self.change_pr_reviewers(id, Vec::new(), vec![name.to_owned()], cx);
    }

    fn change_pr_reviewers(
        &mut self,
        id: PrDetailId,
        add: Vec<String>,
        remove: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id).filter(|d| d.reviewers_editable()) else {
            return;
        };
        detail.reviewer_requests.op = Some(ReviewerOp::Running);
        let (repo, number) = (detail.item.repo.clone(), detail.item.pr.number);
        let gh = GhHost::global(cx);
        let bg = repo.clone();
        spawn_bg_work_and_mutate(
            cx,
            move || gh.edit_pr_reviewers(&bg, number, &add, &remove),
            move |ws, result, cx| ws.land_pr_reviewers(id, repo, result, cx),
        )
        .detach();
        cx.notify();
    }

    /// Done: the box closes. Failed: the text stays. Either way the pull
    /// request is read again (a failure may have applied part of it), the
    /// list asked again, and a failure reported even with the detail gone.
    fn land_pr_reviewers(
        &mut self,
        id: PrDetailId,
        repo: RepoId,
        result: Result<(), GhError>,
        cx: &mut Context<Self>,
    ) {
        PrPoller::nudge(repo, cx);
        let running = self
            .pr_detail_mut(id)
            .map(|detail| &mut detail.reviewer_requests)
            .filter(|requests| requests.op == Some(ReviewerOp::Running));
        match result {
            Ok(()) => {
                if let Some(requests) = running {
                    *requests = ReviewerRequests::default();
                }
            }
            Err(e) => {
                if let Some(requests) = running {
                    requests.op = Some(ReviewerOp::Failed(e.to_string()));
                }
                let report = ErrorReport::new(s::reviewers_failed())
                    .severity(ErrorSeverity::Error)
                    .from_error(&e)
                    .at(file!(), line!())
                    .dedup("github.pr_reviewers.edit")
                    .build();
                self.report_error(report, cx);
            }
        }
        self.refresh_pr_detail(id, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests;
