//! Handing a pull request's problems to the lane's agent: the prompt goes
//! into the agent's input, never submitted, so the user reads it first.

use daruda_github::prompt::{CheckLog, failed_checks};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use daruda_store::project::LaneRef;
use gpui::{App, Context, Window};

use crate::gh_host::GhHost;
use crate::surface::strings::error as s;
use crate::workspace::Workspace;
use crate::workspace::main_area::pane_tree::PaneId;

/// Why a check has no log in the prompt; read by the agent, not the user.
const NOT_ACTIONS: &str = "not a GitHub Actions check";

impl Workspace {
    /// The pane the prompt goes to: the focused one if it is an agent chat
    /// or a terminal in bracketed-paste mode, else an agent chat in the
    /// active tab, else one in another tab. A terminal without bracketed
    /// paste would run each line of a pasted log.
    pub(in crate::workspace) fn pr_send_target(&self, cx: &App) -> Option<PaneId> {
        let runtime = self.active_runtime();
        let focused = runtime
            .panes
            .iter()
            .find(|pane| pane.id == runtime.focused_pane_id)
            .filter(|pane| {
                pane.agent_chat_view().is_some()
                    || pane
                        .terminal_view()
                        .is_some_and(|view| view.read(cx).session().bracketed_paste_enabled())
            });
        if let Some(pane) = focused {
            return Some(pane.id);
        }
        let active_tab = runtime.active_tab_index;
        runtime
            .panes
            .iter()
            .filter(|pane| pane.agent_chat_view().is_some())
            .filter_map(|pane| Some((self.tab_index_for_pane(pane.id)?, pane.id)))
            .min_by_key(|(tab, _)| (*tab != active_tab, *tab))
            .map(|(_, id)| id)
    }

    /// Read the failing checks' logs and put the fix-it prompt into the
    /// active lane's agent input.
    pub(in crate::workspace) fn send_failed_checks(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lane = self.active;
        let Some(link) = self
            .lane_scoped
            .get(&lane)
            .and_then(|state| state.pr.clone())
        else {
            return;
        };
        if link.pr.failed.is_empty() || self.lane_scoped_mut(lane).pr_sending {
            return;
        }
        if self.pr_send_target(cx).is_none() {
            self.report_pr_send(s::github_no_agent_pane(), ErrorSeverity::Warning, cx);
            return;
        }
        self.lane_scoped_mut(lane).pr_sending = true;
        cx.notify();
        let gh = GhHost::global(cx);
        cx.spawn_in(window, async move |this, cx| {
            let prompt = cx
                .background_executor()
                .spawn(async move {
                    let logs: Vec<CheckLog> = link
                        .pr
                        .failed
                        .iter()
                        .map(|check| CheckLog {
                            check: check.clone(),
                            log: match check.job {
                                Some(job) => {
                                    gh.failed_log(&link.repo, job).map_err(|e| e.to_string())
                                }
                                None => Err(NOT_ACTIONS.to_owned()),
                            },
                        })
                        .collect();
                    failed_checks(&link.pr, &logs)
                })
                .await;
            let delivered = this.update_in(cx, |ws, window, cx| {
                ws.deliver_pr_prompt(lane, prompt, window, cx);
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
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.lane_scoped.get_mut(&lane) {
            state.pr_sending = false;
        }
        cx.notify();
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
