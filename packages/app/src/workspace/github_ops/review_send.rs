//! Handing a pull request's problems (failing checks, open review threads)
//! to the lane's agent: the prompt goes into the agent's input, never
//! submitted, so the user reads it first.

use daruda_github::Gh;
use daruda_github::prompt::{CheckLog, failed_checks, review_threads};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use daruda_store::project::LaneRef;
use gpui::{App, Context, Window};

use crate::gh_host::GhHost;
use crate::github::PrLink;
use crate::surface::strings::error as s;
use crate::workspace::Workspace;
use crate::workspace::lane_scoped::PrWork;
use crate::workspace::main_area::pane_tree::PaneId;

/// Why a check has no log in the prompt; read by the agent, not the user.
const NOT_ACTIONS: &str = "not a GitHub Actions check";

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

impl Workspace {
    /// The pane the prompt goes to: a writable agent chat, the focused one
    /// first, then one in the active tab, then one in another tab.
    ///
    /// Never a terminal: a shell's line editor turns bracketed paste on too,
    /// so a terminal cannot tell an agent CLI from a prompt where Enter would
    /// run each line of a pasted log.
    pub(in crate::workspace) fn pr_send_target(&self, cx: &App) -> Option<PaneId> {
        let runtime = self.active_runtime();
        let chats = runtime
            .panes
            .iter()
            .filter(|pane| {
                pane.agent_chat_view()
                    .is_some_and(|view| !view.read(cx).is_read_only())
            })
            .filter_map(|pane| Some((pane.id, self.tab_index_for_pane(pane.id)?)));
        best_target(chats, runtime.focused_pane_id, runtime.active_tab_index)
    }

    /// Read the failing checks' logs and put the fix-it prompt into the
    /// active lane's agent input.
    pub(in crate::workspace) fn send_failed_checks(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let has_failures = self
            .lane_scoped
            .get(&self.active)
            .and_then(|state| state.pr.link())
            .is_some_and(|link| !link.pr.failed.is_empty());
        if !has_failures {
            return;
        }
        self.send_pr_prompt(
            |gh, link| {
                let logs: Vec<CheckLog> = link
                    .pr
                    .failed
                    .iter()
                    .map(|check| CheckLog {
                        check: check.clone(),
                        log: match check.job {
                            Some(job) => gh.failed_log(&link.repo, job).map_err(|e| e.to_string()),
                            None => Err(NOT_ACTIONS.to_owned()),
                        },
                    })
                    .collect();
                Ok(Some(failed_checks(&link.pr, &logs)))
            },
            window,
            cx,
        );
    }

    /// Read the unresolved review threads and put the address-them prompt
    /// into the active lane's agent input.
    pub(in crate::workspace) fn send_review_comments(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_pr_prompt(
            |gh, link| {
                let threads = gh
                    .review_threads(&link.repo, link.pr.number)
                    .map_err(|e| e.to_string())?;
                Ok(review_threads(&link.pr, &threads))
            },
            window,
            cx,
        );
    }

    /// The one path both buttons take: find a target first, build the
    /// prompt off the UI thread with `build` (`Ok(None)`: nothing to send),
    /// then deliver it if the lane is still the one shown.
    fn send_pr_prompt(
        &mut self,
        build: impl FnOnce(&dyn Gh, &PrLink) -> Result<Option<String>, String> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lane = self.active;
        let Some(link) = self
            .lane_scoped
            .get(&lane)
            .and_then(|state| state.pr.link().cloned())
        else {
            return;
        };
        if self
            .lane_scoped
            .get(&lane)
            .is_some_and(|state| state.pr_work == PrWork::Sending)
        {
            return;
        }
        if self.pr_send_target(cx).is_none() {
            self.report_pr_send(s::github_no_agent_pane(), ErrorSeverity::Warning, cx);
            return;
        }
        self.lane_scoped_mut(lane).pr_work = PrWork::Sending;
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
        built: Result<Option<String>, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.lane_scoped.get_mut(&lane)
            && state.pr_work == PrWork::Sending
        {
            state.pr_work = PrWork::Idle;
        }
        cx.notify();
        let prompt = match built {
            Ok(Some(prompt)) => prompt,
            Ok(None) => {
                self.report_pr_send(s::github_no_open_comments(), ErrorSeverity::Info, cx);
                return;
            }
            Err(reason) => {
                let report = ErrorReport::new(s::github_read_failed())
                    .severity(ErrorSeverity::Warning)
                    .message(reason)
                    .at(file!(), line!())
                    .dedup("github.send.read")
                    .build();
                self.report_error(report, cx);
                return;
            }
        };
        if self.active != lane {
            self.report_pr_send(s::github_lane_changed(), ErrorSeverity::Warning, cx);
            return;
        }
        let Some(pane) = self.pr_send_target(cx) else {
            self.report_pr_send(s::github_no_agent_pane(), ErrorSeverity::Warning, cx);
            return;
        };
        if !self.send_pane_selection_to(pane, prompt.into(), window, cx) {
            self.report_pr_send(s::github_send_failed(), ErrorSeverity::Warning, cx);
        }
    }

    fn report_pr_send(&mut self, title: String, severity: ErrorSeverity, cx: &mut Context<Self>) {
        let report = ErrorReport::new(title)
            .severity(severity)
            .at(file!(), line!())
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
