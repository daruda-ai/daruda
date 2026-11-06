//! Per-pane account switching. The invariant: **a switch never destroys
//! work.** A live turn keeps its pane, and the switch opens a fresh pane under
//! the new account instead.
//!
//! **Agent chat** panes let [`switch_kind`] decide. An empty pane reconnects
//! in place through [`Workspace::reset_agent_chat_session`] (the `/clear`
//! path). A conversation continues in the same pane: the new account is a
//! different `CLAUDE_CONFIG_DIR`, so it needs its own ACP session (neither
//! `session/load` nor a resume crosses accounts), and
//! [`Workspace::continue_agent_chat_in_place`] keeps the transcript on screen
//! and briefs that session with a recap. A conversation bound to its exact
//! session — a mirror, a task's chat — still opens a new pane, and the account
//! menu offers a new pane on purpose too.
//!
//! **Terminal** panes always open a new pane, without consulting
//! [`switch_kind`]. There is no existing safe in-place PTY-respawn primitive
//! in this codebase (tearing down and rebuilding a live shell's PTY resources
//! — master, stdout poll task, stdin channel — mid-pane-lifetime is exactly
//! the kind of invented session-teardown mechanism the project avoids shipping
//! without a proven, reused path).
//!
//! This file also owns the per-window side of an account list change
//! ([`Workspace::reconcile_account_pins`] — reverting panes pinned to an
//! account the list no longer names and pruning its usage cache and sticky
//! focus). The shared [`daruda_store::accounts::AccountsState`] itself is
//! propagated app-wide through [`super::accounts_global`] (a GPUI Global +
//! `observe_global`), not a per-window broadcast. The headless add-account
//! / reauthenticate
//! *login* flow that creates and refreshes accounts in the first place
//! lives in the sibling `account_login_ops/` — split out because it is
//! a distinct, self-contained domain (spawn/poll/finish machinery) large
//! enough to warrant its own file; both mirror
//! `settings/sections/accounts.rs` owning the Settings-side half
//! of the same overall account domain (default/delete).

use gpui::{Context, Window};

use daruda_store::accounts::{AccountId, AccountSelection};

use super::SwitchPaneAccount;
use crate::surface::strings as s;
use crate::ui::ButtonVariant;
use crate::ui::dialog::open_confirm_dialog;
use crate::workspace::Workspace;
use crate::workspace::main_area::agent_chat_host::agent_chat_handoff_ops::{
    ChatHold, HandoffTarget,
};
use crate::workspace::main_area::pane;
use crate::workspace::main_area::pane::TabEntry;
use crate::workspace::main_area::pane_tree::{PaneId, PaneLayout};

/// Why a switch leaves the pane alone and opens a new one — the sentence the
/// confirm dialog shows. Its [`NewPaneKind`] follows from it, so a Terminal
/// cause can never build an Agent chat pane, or vice versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum NewPaneCause {
    /// Terminal pane: no safe in-place PTY respawn exists — see the module doc.
    Terminal,
    /// Agent chat mid-turn: a teardown would interrupt live work.
    AgentChatBusy,
    /// Agent chat whose conversation is bound to its exact session — a
    /// read-only mirror, or a task's chat whose delivery a teardown would fail.
    AgentChatPinned,
}

impl NewPaneCause {
    fn kind(self) -> NewPaneKind {
        match self {
            Self::Terminal => NewPaneKind::Terminal,
            Self::AgentChatBusy | Self::AgentChatPinned => NewPaneKind::AgentChat,
        }
    }
}

/// Which constructor a new pane under another account goes through: the kind
/// of the pane it is opened beside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NewPaneKind {
    Terminal,
    AgentChat,
}

/// Whether a pane-account switch reconnects the same pane or opens a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum SwitchKind {
    /// Nothing to lose: tear down and reconnect the same pane under the new
    /// account.
    InPlace,
    /// A conversation a fresh session can pick up: keep it on screen and
    /// continue it under the new account in the same pane.
    Continue,
    /// Something must survive: leave the pane alone and open a new one under
    /// the new account instead.
    NewPane(NewPaneCause),
}

/// Pure decision for an Agent chat pane. Live work keeps its pane; so does a
/// conversation bound to its exact session. Any other conversation continues
/// in place, and an empty pane — choosing an account before the first prompt,
/// the common case — simply reconnects.
pub(in crate::workspace) fn switch_kind(hold: ChatHold) -> SwitchKind {
    match hold {
        ChatHold::Busy => SwitchKind::NewPane(NewPaneCause::AgentChatBusy),
        ChatHold::Pinned => SwitchKind::NewPane(NewPaneCause::AgentChatPinned),
        ChatHold::Conversation => SwitchKind::Continue,
        ChatHold::Empty => SwitchKind::InPlace,
    }
}

impl Workspace {
    /// Action handler: switch the *focused* pane's account. Thin shim over
    /// [`Self::switch_pane_account`] — the concrete dispatch entry point is
    /// this action for a keyboard/programmatic trigger, or a direct call
    /// from the status-bar dropdown's menu-item click (which already knows
    /// the exact pane it's targeting).
    pub(in crate::workspace) fn on_switch_pane_account(
        &mut self,
        action: &SwitchPaneAccount,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_id = self.active_runtime().focused_pane_id;
        self.switch_pane_account(pane_id, action.0, window, cx);
    }

    /// Switch `pane_id`'s account to `selection`
    /// ([`AccountSelection::SystemDefault`] = `~/.claude`). Dispatches on pane
    /// kind: an Agent chat pane consults [`switch_kind`]; a Terminal pane
    /// always opens a new pane (see the module doc); File and flow panes don't
    /// track an account and are a no-op.
    pub(in crate::workspace) fn switch_pane_account(
        &mut self,
        pane_id: PaneId,
        selection: AccountSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = self
            .active_runtime()
            .panes
            .iter()
            .find(|p| p.id == pane_id)
            .and_then(|p| p.account_selection())
        else {
            return; // pane gone, or a kind that tracks no account
        };
        // The dropdown lists the pane's current account as a clickable entry,
        // and every branch below either tears a session down or opens an extra
        // pane — so a re-pick of the account already in use stops here.
        if current == selection {
            return;
        }

        let kind = self
            .agent_chat_hold(pane_id, cx)
            .map_or(SwitchKind::NewPane(NewPaneCause::Terminal), switch_kind);
        match kind {
            SwitchKind::InPlace => {
                self.switch_agent_chat_account_in_place(pane_id, selection, cx);
            }
            SwitchKind::Continue => {
                self.continue_agent_chat_in_place(pane_id, HandoffTarget::Account(selection), cx);
            }
            SwitchKind::NewPane(cause) => {
                self.confirm_open_pane_with_account(pane_id, cause, selection, window, cx);
            }
        }
    }

    /// The account menu's explicit "open in a new pane": the person asked for
    /// the extra pane, so no confirm, and `pane_id` keeps its own session.
    pub(in crate::workspace) fn open_pane_with_account(
        &mut self,
        pane_id: PaneId,
        selection: AccountSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let kind = if self.agent_chat_view(pane_id).is_some() {
            NewPaneKind::AgentChat
        } else {
            NewPaneKind::Terminal
        };
        self.open_new_pane_with_account(pane_id, kind, selection, window, cx);
    }

    /// Record `selection` as `pane_id`'s account in both wrapper caches; the
    /// next connect resolves its `CLAUDE_CONFIG_DIR` from it
    /// (`agent_chat_account_selection`).
    pub(in crate::workspace) fn set_agent_chat_account(
        &mut self,
        pane_id: PaneId,
        selection: AccountSelection,
    ) {
        if let Some(content) = self
            .active_runtime_mut()
            .panes
            .iter_mut()
            .find(|p| p.id == pane_id)
            .and_then(|pane| pane.agent_chat_content_mut())
        {
            content.account = selection;
        }
        if let Some(chat) = self
            .orchestrator
            .chat
            .as_mut()
            .filter(|chat| chat.pane_id == pane_id)
        {
            chat.account = selection;
        }
    }

    /// `SwitchKind::InPlace` for an Agent chat pane: set the pane's account
    /// selection, then reuse [`Self::reset_agent_chat_session`] — the same
    /// path the `/clear` slash command runs — to tear down and reconnect the
    /// session. `connect_agent_chat` resolves the new `CLAUDE_CONFIG_DIR`
    /// from the selection this just wrote (`agent_chat_account_selection`),
    /// so the fresh session runs under the newly selected account (or
    /// `~/.claude` for [`AccountSelection::SystemDefault`]).
    fn switch_agent_chat_account_in_place(
        &mut self,
        pane_id: PaneId,
        selection: AccountSelection,
        cx: &mut Context<Self>,
    ) {
        self.set_agent_chat_account(pane_id, selection);
        self.reset_agent_chat_session(pane_id, cx);
    }

    /// `SwitchKind::NewPane`: confirm with the user first (G9 — this opens
    /// an extra pane they didn't explicitly ask to open), then build it via
    /// [`Self::open_new_pane_with_account`]. The body names the actual reason
    /// this pane can't be reused, since the three read very differently to a
    /// user deciding whether to go ahead.
    fn confirm_open_pane_with_account(
        &mut self,
        source_pane_id: PaneId,
        cause: NewPaneCause,
        selection: AccountSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let body = match cause {
            NewPaneCause::Terminal => s::modal::switch_account_new_pane_body_terminal(),
            NewPaneCause::AgentChatBusy => s::modal::switch_account_new_pane_body_busy(),
            NewPaneCause::AgentChatPinned => s::modal::switch_account_new_pane_body_pinned(),
        };
        let weak = cx.weak_entity();
        open_confirm_dialog(
            s::modal::switch_account_new_pane_title(),
            body,
            s::modal::switch_account_new_pane_confirm(),
            ButtonVariant::Primary,
            move |_, window, app_cx| {
                if let Some(ws) = weak.upgrade() {
                    ws.update(app_cx, |ws, cx| {
                        ws.open_new_pane_with_account(
                            source_pane_id,
                            cause.kind(),
                            selection,
                            window,
                            cx,
                        );
                    });
                }
            },
            window,
            cx,
        );
    }

    /// Build a fresh pane under `account_id` in the active lane's current
    /// tab set and focus it. Terminal reuses [`Self::create_pane_with_cwd`]
    /// (the same constructor `Self::create_pane` wraps); Agent chat reuses
    /// [`Self::create_new_agent_chat_pane`], inheriting `source_pane_id`'s
    /// agent (mirrors `split_focused_pane_kind`'s inheritance) and patching
    /// the account override onto the result the same way session restore
    /// does. Mirrors `add_tab` / `open_agent_chat_pane_with_agent`'s
    /// tab-append + focus flow — no new pane-opening machinery.
    fn open_new_pane_with_account(
        &mut self,
        source_pane_id: PaneId,
        kind: NewPaneKind,
        selection: AccountSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_lane_is_inaccessible() {
            return;
        }
        let new_pane = match kind {
            NewPaneKind::Terminal => {
                let cwd = self.default_cwd_for_new_pane();
                // Terminal pane: no agent, so no required auth domain —
                // the account's own recipe decides the env.
                let prepared = self.resolve_account(selection, pane::AccountDomain::Any);
                match self.create_pane_with_cwd(cwd, selection, prepared.as_ref(), window, cx) {
                    Ok(p) => p,
                    Err(e) => {
                        self.report_pane_error(
                            &crate::surface::strings::error::pane_context_switch_account(),
                            e,
                            cx,
                        );
                        return;
                    }
                }
            }
            NewPaneKind::AgentChat => {
                let (local_cwd, remote_cwd, session_host) = self.active_lane_cwds();
                let agent_id = self
                    .agent_chat_view(source_pane_id)
                    .map(|v| v.read(cx).agent_id().to_owned())
                    .unwrap_or_else(|| self.open_agent_id(None));
                let mut new_pane = self.create_new_agent_chat_pane(
                    agent_id,
                    local_cwd,
                    remote_cwd,
                    session_host,
                    window,
                    cx,
                );
                if let Some(content) = new_pane.agent_chat_content_mut() {
                    content.account = selection;
                }
                new_pane
            }
        };

        let pane_id = new_pane.id;
        let tab_id = self.alloc_id();
        self.active_runtime_mut().panes.push(new_pane);
        self.active_runtime_mut().tabs.push(TabEntry {
            id: tab_id,
            layout: PaneLayout::Pane(pane_id),
            last_focused_pane: pane_id,
            user_label: None,
        });
        let cur_tab = self.active_runtime().active_tab_index;
        self.active_runtime_mut().tab_history.push(cur_tab);
        let last_tab = self.active_runtime().tabs.len() - 1;
        self.active_runtime_mut().active_tab_index = last_tab;

        if kind == NewPaneKind::AgentChat && !self.docks.bottom.read(cx).is_open {
            self.docks.bottom.update(cx, |d, cx| {
                d.toggle();
                cx.notify();
            });
            self.main_area.pending_resize = true;
        }
        self.set_focused_pane(pane_id, window, cx);
        self.bump_activity(pane_id);
        self.focus_pane(pane_id, window, cx);
        self.resize_all_tabs(window, cx);
        self.mutate_durable(cx, |_, _| {});
    }

    /// Count of this workspace's currently-*loaded* panes (every lane
    /// runtime visited this session, not just the active one) whose
    /// account override is `account_id`. A lane never opened this session
    /// has no entry in `main_area.runtimes` yet, so this can undercount
    /// — an accepted simplification (see `settings/sections/accounts.rs`).
    ///
    /// `pub(crate)` as the one read-only exception to G7: the Settings
    /// account-delete confirm sums this across every open window to say how
    /// many panes the delete reverts. It changes nothing; the revert itself
    /// is [`Self::reconcile_account_pins`], which the delete reaches through
    /// `AccountsGlobal`.
    pub(crate) fn panes_referencing_account(&self, account_id: AccountId) -> usize {
        self.main_area
            .runtimes
            .values()
            .flat_map(|rt| rt.panes.iter())
            .filter(|p| {
                !self.is_orchestrator_pane(p.id)
                    && p.account_selection() == Some(AccountSelection::Managed(account_id))
            })
            .count()
            + usize::from(
                self.orchestrator
                    .chat
                    .as_ref()
                    .is_some_and(|chat| chat.account == AccountSelection::Managed(account_id)),
            )
    }

    /// Bring every account pin in step with [`Self::accounts`]: a pane (or
    /// the hidden orchestrator slot) pinned to an account the list no longer
    /// names reverts to [`AccountSelection::SystemDefault`], and the usage
    /// caches and sticky usage focus drop what they held for it. A Terminal
    /// pane's shell keeps running under the env it spawned with; only the
    /// selection used for persistence and display is reset.
    ///
    /// Runs from the `AccountsGlobal` observer, so a delete in any window
    /// lands here the same way — as does an account an outside edit dropped,
    /// once the next in-app write reloads the file. Idempotent: that observer
    /// also fires for the login slot, and a pass with nothing to change
    /// persists nothing.
    pub(in crate::workspace) fn reconcile_account_pins(&mut self, cx: &mut Context<Self>) {
        let accounts = &self.accounts;
        let dangling = |selection: AccountSelection| match selection {
            AccountSelection::Managed(id) => accounts.find(id).is_none(),
            AccountSelection::SystemDefault => false,
        };
        let mut pane_changed = false;
        if let Some(chat) = self
            .orchestrator
            .chat
            .as_mut()
            .filter(|chat| dangling(chat.account))
        {
            chat.account = AccountSelection::SystemDefault;
            pane_changed = true;
        }
        for p in self
            .main_area
            .runtimes
            .values_mut()
            .flat_map(|rt| rt.panes.iter_mut())
        {
            let account = match &mut p.content {
                pane::PaneContent::Terminal(t) => &mut t.account,
                pane::PaneContent::AgentChat(ac) if !ac.view.read(cx).is_read_only() => {
                    &mut ac.account
                }
                _ => continue,
            };
            if dangling(*account) {
                *account = AccountSelection::SystemDefault;
                pane_changed = true;
            }
        }
        self.agent_usage.usage_by_account.retain_known(accounts);
        let sticky_focus_len = self.agent_usage.sticky_focus_by_recipe.len();
        self.agent_usage
            .sticky_focus_by_recipe
            .retain(|_, focused| match focused {
                pane::FocusedAccount::Managed { id, .. } => accounts.find(*id).is_some(),
                pane::FocusedAccount::SystemDefault => true,
            });
        let sticky_focus_changed =
            self.agent_usage.sticky_focus_by_recipe.len() != sticky_focus_len;
        if pane_changed {
            self.mutate_durable(cx, |_, _| {});
        }
        if pane_changed || sticky_focus_changed {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_kind_reconnects_in_place_only_with_nothing_to_lose() {
        assert_eq!(switch_kind(ChatHold::Empty), SwitchKind::InPlace);
    }

    #[test]
    fn switch_kind_continues_a_conversation_on_an_idle_pane() {
        assert_eq!(switch_kind(ChatHold::Conversation), SwitchKind::Continue);
    }

    #[test]
    fn switch_kind_keeps_a_pinned_conversation_on_its_session() {
        assert_eq!(
            switch_kind(ChatHold::Pinned),
            SwitchKind::NewPane(NewPaneCause::AgentChatPinned)
        );
    }

    #[test]
    fn switch_kind_protects_live_work() {
        assert_eq!(
            switch_kind(ChatHold::Busy),
            SwitchKind::NewPane(NewPaneCause::AgentChatBusy),
            "a busy pane is never torn down mid-work"
        );
    }

    /// The confirm's reason decides the pane it builds: only a terminal's
    /// reason builds a terminal.
    #[test]
    fn a_new_pane_is_the_kind_of_the_pane_it_is_opened_beside() {
        assert_eq!(NewPaneCause::Terminal.kind(), NewPaneKind::Terminal);
        for cause in [NewPaneCause::AgentChatBusy, NewPaneCause::AgentChatPinned] {
            assert_eq!(cause.kind(), NewPaneKind::AgentChat);
        }
    }
}
