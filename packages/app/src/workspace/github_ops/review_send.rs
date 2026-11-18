//! Handing a pull request's problems (failing checks, open review threads)
//! to a lane's agent: the prompt goes into the agent's input, never
//! submitted, so the user reads it first. The Git panel sends about the
//! active lane's pull request; the Pull requests page about the lane linked
//! to the one it shows.

use daruda_github::prompt::{CheckLog, NoLog, failed_checks, review_threads};
use daruda_github::{Gh, GhError};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use daruda_store::project::LaneRef;
use gpui::{App, Context, Window};

use crate::gh_host::GhHost;
use crate::github::PrLink;
use crate::surface::strings::error as s;
use crate::workspace::Workspace;
use crate::workspace::main_area::pane_tree::PaneId;

/// A writable agent chat a prompt can go into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct WritableChat {
    pub pane: PaneId,
    pub title: gpui::SharedString,
    /// Its tab's number, from 1: chats often share a title.
    pub tab: usize,
}

/// Among writable chats as `(pane, tab)`: the focused one, then one in the
/// active tab, then the first by tab order.
fn best_target(
    chats: impl IntoIterator<Item = (PaneId, usize)>,
    focused: PaneId,
    active_tab: usize,
) -> Option<PaneId> {
    chats
        .into_iter()
        .min_by_key(|&(pane, tab)| (pane != focused, tab != active_tab, tab))
        .map(|(pane, _)| pane)
}

/// Where a pull request's problems are sent: the lane whose agent works on
/// it, and the pull request as last read.
#[derive(Debug, Clone)]
pub(in crate::workspace) struct PrSendTarget {
    pub lane: LaneRef,
    pub link: PrLink,
}

/// The fix-it prompt for a pull request's failing checks, their logs read
/// through `gh`.
pub(in crate::workspace) fn checks_prompt(
    gh: &dyn Gh,
    link: &PrLink,
) -> Result<Option<String>, GhError> {
    let logs: Vec<CheckLog> = link
        .pr
        .failed
        .iter()
        .map(|check| CheckLog {
            check: check.clone(),
            log: match check.job {
                Some(job) => gh
                    .failed_log(&link.repo, job)
                    .map_err(|e| NoLog::Unreadable(e.to_string())),
                None => Err(NoLog::NotActions),
            },
        })
        .collect();
    Ok((!logs.is_empty()).then(|| failed_checks(&link.pr, &logs)))
}

/// The address-them prompt for a pull request's unresolved threads; `None`
/// when there are none.
pub(in crate::workspace) fn threads_prompt(
    gh: &dyn Gh,
    link: &PrLink,
) -> Result<Option<String>, GhError> {
    let threads = gh.review_threads(&link.repo, link.pr.number)?;
    Ok(review_threads(&link.pr, &threads))
}

impl Workspace {
    /// The pane in `lane` the prompt goes to: a writable agent chat, the
    /// focused one first, then one in the active tab, then one in another tab.
    ///
    /// Never a terminal: a shell's line editor turns bracketed paste on too,
    /// so a terminal cannot tell an agent CLI from a prompt where Enter would
    /// run each line of a pasted log.
    fn pr_send_target(&self, lane: LaneRef, cx: &App) -> Option<PaneId> {
        let runtime = self.main_area.runtimes.get(&lane)?;
        best_target(
            self.lane_chats(lane, cx),
            runtime.focused_pane_id,
            runtime.active_tab_index,
        )
    }

    /// `lane`'s writable agent chats as `(pane, tab)`; never a terminal.
    fn lane_chats(&self, lane: LaneRef, cx: &App) -> Vec<(PaneId, usize)> {
        let Some(runtime) = self.main_area.runtimes.get(&lane) else {
            return Vec::new();
        };
        runtime
            .panes
            .iter()
            .filter(|pane| {
                pane.agent_chat_view()
                    .is_some_and(|view| !view.read(cx).is_read_only())
            })
            .filter_map(|pane| {
                let tab = runtime
                    .tabs
                    .iter()
                    .position(|tab| tab.layout.pane_ids().contains(&pane.id))?;
                Some((pane.id, tab))
            })
            .collect()
    }

    /// `lane`'s writable agent chats with their titles and tab numbers
    /// (from 1), the one a send would pick first.
    pub(in crate::workspace) fn writable_chats(
        &self,
        lane: LaneRef,
        cx: &App,
    ) -> Vec<WritableChat> {
        let Some(runtime) = self.main_area.runtimes.get(&lane) else {
            return Vec::new();
        };
        let best = self.pr_send_target(lane, cx);
        let mut chats: Vec<WritableChat> = self
            .lane_chats(lane, cx)
            .into_iter()
            .filter_map(|(pane, tab)| {
                let title = runtime.panes.iter().find(|held| held.id == pane)?.title(cx);
                Some(WritableChat {
                    pane,
                    title,
                    tab: tab + 1,
                })
            })
            .collect();
        chats.sort_by_key(|chat| (Some(chat.pane) != best, chat.tab));
        chats
    }

    /// Put `prompt` into the input of chat `pane` in `lane`, unsent,
    /// showing the lane first; checked again here, as the chat may have
    /// closed or become read-only while the prompt was prepared.
    pub(in crate::workspace) fn deliver_to_chat(
        &mut self,
        lane: LaneRef,
        pane: PaneId,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .lane_chats(lane, cx)
            .iter()
            .any(|(held, _)| *held == pane)
        {
            self.report_pr_send(s::github_no_agent_pane(), ErrorSeverity::Warning, cx);
            return false;
        }
        if !self.shows_lane(lane) {
            self.activate_lane(lane, window, cx);
        }
        let sent = self.send_pane_selection_to(pane, prompt.into(), window, cx);
        if !sent {
            self.report_pr_send(s::github_send_failed(), ErrorSeverity::Warning, cx);
        }
        sent
    }

    /// Whether `lane` is what the main area shows: the active lane, with no
    /// workspace page over it.
    fn shows_lane(&self, lane: LaneRef) -> bool {
        self.active == lane && self.active_page().is_none()
    }

    /// The active lane and its linked pull request, which the Git panel's
    /// buttons send about.
    fn active_pr_target(&self) -> Option<PrSendTarget> {
        let link = self.lane_scoped.get(&self.active)?.pr.link()?.clone();
        Some(PrSendTarget {
            lane: self.active,
            link,
        })
    }

    /// Whether a send for `lane` is under way, so its buttons stay inert.
    pub(in crate::workspace) fn is_sending_pr(&self, lane: LaneRef) -> bool {
        self.lane_scoped
            .get(&lane)
            .is_some_and(|state| state.pr.is_sending())
    }

    /// The Git panel's Fix checks: the active lane's pull request.
    pub(in crate::workspace) fn send_failed_checks(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(target) = self.active_pr_target() {
            self.send_failed_checks_to(target, window, cx);
        }
    }

    /// The Git panel's Review comments: the active lane's pull request.
    pub(in crate::workspace) fn send_review_comments(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(target) = self.active_pr_target() {
            self.send_review_comments_to(target, window, cx);
        }
    }

    /// Read the failing checks' logs and put the fix-it prompt into the
    /// agent input of `target`'s lane.
    pub(in crate::workspace) fn send_failed_checks_to(
        &mut self,
        target: PrSendTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if target.link.pr.failed.is_empty() {
            return;
        }
        self.send_pr_prompt(target, checks_prompt, window, cx);
    }

    /// Read the unresolved review threads and put the address-them prompt
    /// into the agent input of `target`'s lane.
    pub(in crate::workspace) fn send_review_comments_to(
        &mut self,
        target: PrSendTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_pr_prompt(target, threads_prompt, window, cx);
    }

    /// The one path every button takes: show the lane, find a target in it,
    /// build the prompt off the UI thread with `build` (`Ok(None)`: nothing
    /// to send), then deliver it if the lane is still the one shown.
    fn send_pr_prompt(
        &mut self,
        target: PrSendTarget,
        build: impl FnOnce(&dyn Gh, &PrLink) -> Result<Option<String>, GhError> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let PrSendTarget { lane, link } = target;
        // A send already under way answers the click; no target warning.
        if self.is_sending_pr(lane) {
            return;
        }
        // The prompt lands in the lane's chat input unsent, for the user to
        // read first, so that lane is shown before anything is read: over a
        // page, the Git panel's as much as the Pull requests page's. It comes
        // before looking for a chat, so a lane without one is where the user
        // is told to open it, not the page they pressed the button on.
        if !self.shows_lane(lane) {
            self.activate_lane(lane, window, cx);
        }
        if self.pr_send_target(lane, cx).is_none() {
            self.report_pr_send(s::github_no_agent_pane(), ErrorSeverity::Warning, cx);
            return;
        }
        self.lane_scoped_mut(lane).pr.begin_send();
        cx.notify();
        let gh = GhHost::global(cx);
        cx.spawn_in(window, async move |this, cx| {
            let built = cx
                .background_executor()
                .spawn(async move { build(gh.as_ref(), &link) })
                .await;
            let delivered = this.update_in(cx, |ws, window, cx| {
                ws.deliver_pr_prompt(lane, built, window, cx);
            });
            if let Err(e) = delivered {
                LogWriter::log(
                    ErrorReport::new("PR prompt could not reach its workspace")
                        .severity(ErrorSeverity::Info)
                        .with_context("error", e.to_string())
                        .at(file!(), line!())
                        .dedup("github.send.closed")
                        .build(),
                );
            }
        })
        .detach();
    }

    /// Put `prompt` into the agent input of `lane`, if it is still the one
    /// shown; the user may have moved on while the logs were read.
    fn deliver_pr_prompt(
        &mut self,
        lane: LaneRef,
        built: Result<Option<String>, GhError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.lane_scoped.get_mut(&lane) {
            state.pr.end_send();
        }
        cx.notify();
        let prompt = match built {
            Ok(Some(prompt)) => prompt,
            Ok(None) => {
                self.report_pr_send(s::github_no_open_comments(), ErrorSeverity::Info, cx);
                return;
            }
            Err(e) => {
                let report = ErrorReport::new(s::github_read_failed())
                    .severity(ErrorSeverity::Warning)
                    .from_error(&e)
                    .at(file!(), line!())
                    .dedup("github.send.read")
                    .build();
                self.report_error(report, cx);
                return;
            }
        };
        // Another lane, or a page over this one: its input is out of sight.
        if !self.shows_lane(lane) {
            self.report_pr_send(s::github_lane_changed(), ErrorSeverity::Warning, cx);
            return;
        }
        let Some(pane) = self.pr_send_target(lane, cx) else {
            self.report_pr_send(s::github_no_agent_pane(), ErrorSeverity::Warning, cx);
            return;
        };
        if !self.send_pane_selection_to(pane, prompt.into(), window, cx) {
            self.report_pr_send(s::github_send_failed(), ErrorSeverity::Warning, cx);
        }
    }

    #[track_caller]
    fn report_pr_send(&mut self, title: String, severity: ErrorSeverity, cx: &mut Context<Self>) {
        let at = std::panic::Location::caller();
        let report = ErrorReport::new(title)
            .severity(severity)
            .at(at.file(), at.line())
            .dedup("github.send")
            .build();
        self.report_error(report, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_focused_chat_wins_then_the_active_tab_then_tab_order() {
        /// (label, (chat, tab) pairs, focused pane, active tab, expected)
        type Row = (
            &'static str,
            Vec<(PaneId, usize)>,
            PaneId,
            usize,
            Option<PaneId>,
        );
        let cases: Vec<Row> = vec![
            ("no chat", vec![], 1, 0, None),
            ("focused chat", vec![(1, 2), (2, 0)], 1, 0, Some(1)),
            (
                "active tab over earlier tab",
                vec![(1, 0), (2, 3)],
                9,
                3,
                Some(2),
            ),
            ("first tab otherwise", vec![(5, 4), (6, 2)], 9, 0, Some(6)),
        ];
        for (label, chats, focused, active, expected) in cases {
            assert_eq!(best_target(chats, focused, active), expected, "{label}");
        }
    }
}
