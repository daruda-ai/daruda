//! Bring the panes a successful login unblocked up to date.

use crate::workspace::{Workspace, main_area::pane_tree::PaneId};
use daruda_chat::view::AgentSessionStatus;
use gpui::Context;

impl Workspace {
    /// Bring every pane on the credentials a successful login into `target`
    /// wrote up to date, in every window — the tail of both reauthenticate
    /// flows. Only the window that ran the login hears its outcome, but the
    /// credentials are shared, so its panes are not the only ones refused.
    ///
    /// Deferred: this runs inside that window's own update, and the fan-out
    /// updates it again (AGENTS.md Pitfall #5).
    pub(in crate::workspace) fn settle_panes_after_login_everywhere(
        &mut self,
        target: daruda_agent::accounts::LoginTarget,
        cx: &mut Context<Self>,
    ) {
        cx.defer(move |cx| {
            crate::window_registry::WindowRegistry::for_each_workspace(cx, |ws, _window, cx| {
                ws.settle_panes_after_login(target, cx)
            });
        });
    }

    /// This window's half of [`Self::settle_panes_after_login_everywhere`].
    ///
    /// Without it a user who signs in from a failure is left looking at the
    /// same error, with no sign the login took and a button offering it again.
    /// Scoped to the panes that run on those credentials: a login fixes
    /// nothing for the others.
    ///
    /// An expired login usually surfaces as a **turn** failure: the agent
    /// creates the session without checking credentials and refuses at the
    /// first real request, so the pane stays `Connected`. Such a pane only
    /// loses the refusal ending its conversation; its session is left running,
    /// because whether a running adapter picks up freshly written credentials
    /// without a reconnect is not established, and tearing down a live session
    /// to find out would cost the user their conversation on a guess. A pane
    /// whose *connection* failed is reconnected as well ([`login_clears`]).
    pub(in crate::workspace) fn settle_panes_after_login(
        &mut self,
        target: daruda_agent::accounts::LoginTarget,
        cx: &mut Context<Self>,
    ) {
        // Collected before any view is updated: `retry_agent_chat_connect`
        // leases each view, and holding a read across that would re-enter the
        // same entity (AGENTS.md Pitfall #5).
        let signed_in: Vec<(PaneId, Option<daruda_acp::Remedy>)> = self
            .every_agent_chat()
            .map(|(id, _)| (id, self.agent_chat_account_selection(id)))
            .collect::<Vec<_>>()
            .into_iter()
            .filter(|&(pane_id, selection)| {
                let domain = crate::workspace::main_area::pane::AccountDomain::for_pane(
                    &self.account_pane_for(pane_id),
                );
                crate::workspace::account_login_ops::pane_login_target(
                    selection,
                    domain,
                    &self.accounts,
                ) == Some(target)
            })
            .map(|(pane_id, _)| {
                let remedy =
                    self.agent_chat_view(pane_id)
                        .and_then(|view| match &view.read(cx).status() {
                            AgentSessionStatus::Error { remedy, .. } => Some(*remedy),
                            _ => None,
                        });
                (pane_id, remedy)
            })
            .collect();

        for (pane_id, remedy) in signed_in {
            if let Some(view) = self.agent_chat_view(pane_id) {
                view.update(cx, |view, cx| view.signed_in_again(cx));
            }
            if login_clears(remedy) {
                self.retry_agent_chat_connect(pane_id, cx);
            }
        }
    }
}

/// Whether a fresh sign-in clears a pane's connection failure, `remedy`
/// (`None` when its connection did not fail), so reconnecting it is progress.
///
/// Only `Reauthenticate` is: an organization-blocked account reconnects into
/// the identical refusal, so retrying it automatically would spend the user's
/// attention on a loop dressed up as progress. And a pane that never failed is
/// mid-conversation; a login must not tear its live session down.
pub(super) fn login_clears(remedy: Option<daruda_acp::Remedy>) -> bool {
    remedy == Some(daruda_acp::Remedy::Reauthenticate)
}

#[cfg(test)]
mod tests;
