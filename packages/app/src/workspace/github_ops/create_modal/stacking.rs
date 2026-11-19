//! Stacking the new pull request on another: the open pull request its
//! base branch heads, offered as a choice once found; and, when the pull
//! request was created but GitHub did not take it into the stack, stacking
//! it again from the form.
//!
//! Only on github.com, where the stack endpoints are documented. A stack
//! is GitHub's: nothing about it is kept here.

use std::time::Duration;

use daruda_github::RepoId;
use gpui::{Context, Window};

use super::summary::TargetRead;
use super::{CreatePrModal, CreateState};
use crate::gh_host::GhHost;
use crate::workspace::github_ops::create::Unstacked;

/// The only host whose stack endpoints are documented.
const STACKS_HOST: &str = "github.com";

impl CreatePrModal {
    /// The repository to ask and the base to ask about, when a pull
    /// request there could be stacked on: a branch of the push remote,
    /// other than the head.
    fn stack_question(&self, cx: &gpui::App) -> Option<(RepoId, String)> {
        let TargetRead::Pushes {
            repo: Some(repo),
            shown,
        } = &self.publication.as_ref()?.target
        else {
            return None;
        };
        let base = self.base_value(cx)?;
        (repo.host() == STACKS_HOST && base != shown.branch && self.remote_branches.contains(&base))
            .then(|| (repo.clone(), base))
    }

    /// Find the open pull request the base typed now heads, after `delay`
    /// when typing; an older read still running is dropped.
    pub(super) fn read_stack_parent(
        &mut self,
        delay: Option<Duration>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((repo, base)) = self.stack_question(cx) else {
            self.stack_parent_read = None;
            return;
        };
        let gh = GhHost::global(cx);
        self.stack_parent_read = Some(cx.spawn_in(window, async move |this, cx| {
            if let Some(delay) = delay {
                cx.background_executor().timer(delay).await;
            }
            let asked = base.clone();
            let parent = cx
                .background_executor()
                .spawn(async move {
                    // SILENT-OK: an offer only; a failed read offers none
                    // and the pull request is created as without it.
                    let found = gh
                        .prs_for_branches(&repo, std::slice::from_ref(&base), &[])
                        .ok()?;
                    let prs = found.by_branch.get(&base)?;
                    daruda_github::link::open_for(&base, prs).map(|pr| pr.number)
                })
                .await;
            // SILENT-OK: the form may have closed before GitHub answered.
            let _ = this.update(cx, |this, cx| {
                this.stack_parent = parent.map(|number| (asked, number));
                this.stack_parent_read = None;
                cx.notify();
            });
        }));
    }

    /// The pull request the base typed now heads, when one was found.
    pub(super) fn current_stack_parent(&self, cx: &gpui::App) -> Option<u64> {
        let (base, number) = self.stack_parent.as_ref()?;
        (Some(base) == self.base_value(cx).as_ref()).then_some(*number)
    }

    /// What a request carries: the parent, when the user asked to stack
    /// and it is still the one for the base typed.
    pub(super) fn stack_on_value(&self, cx: &gpui::App) -> Option<u64> {
        self.current_stack_parent(cx)
            .filter(|parent| self.stack_on == Some(*parent))
    }

    /// Ask to stack on the parent offered now, or take that back.
    pub(super) fn toggle_stack_on(&mut self, cx: &mut Context<Self>) {
        let offered = self.current_stack_parent(cx);
        self.stack_on = if self.stack_on == offered {
            None
        } else {
            offered
        };
        self.clear_failure(cx);
        cx.notify();
    }

    /// As after creating #483 on #482, GitHub refusing the stacking.
    #[cfg(feature = "screenshot")]
    pub(in crate::workspace::github_ops) fn unstacked_for_shot(mut self) -> Self {
        self.stack_parent = Some(("sidebar-base".to_owned(), 482));
        self.stack_on = Some(482);
        self.state = CreateState::Unstacked {
            left: Unstacked {
                repo: RepoId::new(STACKS_HOST, "daruda", "daruda"),
                parent: 482,
                number: Some(483),
                reason: "gh: Validation Failed (HTTP 422)".to_owned(),
            },
            retrying: false,
        };
        self
    }

    /// The pull request was created but is not in its stack: the form
    /// stays, says so, and offers stacking again.
    pub(in crate::workspace::github_ops) fn left_unstacked(
        &mut self,
        left: Unstacked,
        cx: &mut Context<Self>,
    ) {
        self.state = CreateState::Unstacked {
            left,
            retrying: false,
        };
        cx.notify();
    }

    /// Stack the created pull request again; the workspace does it and
    /// tells the outcome, so closing the form meanwhile loses neither.
    pub(super) fn stack_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let CreateState::Unstacked {
            left,
            retrying: retrying @ false,
        } = &mut self.state
        else {
            return;
        };
        let Some(number) = left.number else {
            return;
        };
        *retrying = true;
        let (repo, parent) = (left.repo.clone(), left.parent);
        cx.notify();
        let (me, handle) = (cx.weak_entity(), window.window_handle());
        workspace.update(cx, |ws, cx| {
            ws.stack_pr_again(repo, parent, number, me, handle, cx);
        });
    }

    /// Stacking again failed: the form says why now, and offers it again.
    pub(in crate::workspace::github_ops) fn stack_failed(
        &mut self,
        reason: String,
        cx: &mut Context<Self>,
    ) {
        if let CreateState::Unstacked { left, retrying } = &mut self.state {
            left.reason = reason;
            *retrying = false;
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests;
