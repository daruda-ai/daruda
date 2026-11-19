//! What a pane is like, whatever its kind: the questions the workspace
//! asks of every pane, answered once per kind.
//!
//! A kind implements [`PaneBehavior`] and says only what is true of it;
//! what does not apply keeps the default. `PaneContent::behavior` is the
//! one place the kinds are told apart for these questions, so a new kind
//! is one `impl` here rather than an arm in every method. Where kinds must
//! differ in kind-specific data (what a pane draws, what it saves), the
//! callers still match on `PaneContent`, and the compiler still asks.

use std::path::Path;

use daruda_store::project::PaneCwd;
use gpui::{App, FocusHandle, Focusable as _, SharedString};

use super::{AgentChatContent, FileContent, PaneContent, PrPaneContent, TerminalContent};
use crate::ui::icons;

/// Which of a worktree's tab groups a pane of a kind may sit in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum PaneHosting {
    /// Among the tabs only: what is worked in, a shell or a chat.
    Tabs,
    /// In either group: what is read beside the work, and may be sent
    /// between them.
    Either,
    /// In the side tabs only.
    Side,
}

impl PaneHosting {
    /// Whether the side tabs take it.
    pub(in crate::workspace) fn aside(self) -> bool {
        matches!(self, Self::Either | Self::Side)
    }

    /// Whether it may sit among the tabs.
    pub(in crate::workspace) fn among_tabs(self) -> bool {
        matches!(self, Self::Tabs | Self::Either)
    }
}

pub(in crate::workspace) trait PaneBehavior {
    /// Title shown in the tab strip, pane header and status bar.
    fn title(&self, cx: &App) -> SharedString;

    /// Focus handle the pane gives to the window when activated.
    fn focus_handle(&self, cx: &App) -> FocusHandle;

    /// Focus handle for the outer pane-wrapper div's `track_focus` call:
    /// `Some` for content drawn as a plain div tree, `None` (the default)
    /// for content whose own view tracks its focus in `Render`, which the
    /// wrapper must not track twice.
    fn wrapper_focus_handle(&self) -> Option<&FocusHandle> {
        None
    }

    /// The local directory the content is in, when it tracks one. Never a
    /// path on another machine: every consumer expects one that exists here.
    fn cwd(&self) -> Option<&Path> {
        None
    }

    /// Whether the pane holds unsaved edits.
    fn is_dirty(&self, _cx: &App) -> bool {
        false
    }

    /// Whether closing the pane would stop work in progress.
    fn runs_work(&self, _cx: &App) -> bool {
        false
    }

    /// Whether the pane's save path is meaningful for the user.
    fn can_save(&self) -> bool {
        false
    }

    /// The account the pane runs under, for a kind that tracks one.
    /// `None` hides the status bar's account slot rather than show a
    /// misleading "System".
    fn account_selection(&self) -> Option<daruda_store::accounts::AccountSelection> {
        None
    }

    /// The icon a pane of this kind has in the rail while it has no status.
    fn rail_icon(&self) -> &'static str;

    /// Which tab group the kind may sit in.
    fn hosting(&self) -> PaneHosting {
        PaneHosting::Tabs
    }
}

impl PaneBehavior for TerminalContent {
    fn title(&self, _cx: &App) -> SharedString {
        self.cached_title.clone()
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.view.read(cx).focus_handle().clone()
    }

    fn cwd(&self) -> Option<&Path> {
        self.cached_cwd.as_deref()
    }

    /// A job in the shell's foreground.
    fn runs_work(&self, cx: &App) -> bool {
        !self.exited
            && (self.view.read(cx).session().command_is_running()
                || self.master.as_deref().is_some_and(|master| {
                    daruda_terminal::pty::runs_foreground_job(master, self.shell_pid)
                }))
    }

    fn account_selection(&self) -> Option<daruda_store::accounts::AccountSelection> {
        Some(self.account)
    }

    fn rail_icon(&self) -> &'static str {
        icons::TERMINAL
    }
}

impl PaneBehavior for FileContent {
    fn title(&self, _cx: &App) -> SharedString {
        self.cached_title.clone()
    }

    /// The pane-level handle; the find panel's input has its own, which
    /// takes priority while the panel is open.
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn wrapper_focus_handle(&self) -> Option<&FocusHandle> {
        Some(&self.focus_handle)
    }

    /// The file's parent directory.
    fn cwd(&self) -> Option<&Path> {
        self.view.path.parent()
    }

    /// The editor differs from the text it loaded.
    fn is_dirty(&self, cx: &App) -> bool {
        self.view.holds_editable_buffer() && *self.editor_state.read(cx).text() != self.saved_text
    }

    fn can_save(&self) -> bool {
        self.view.holds_editable_buffer() && self.view.path.is_absolute()
    }

    fn rail_icon(&self) -> &'static str {
        icons::FILE_VIEWER
    }

    fn hosting(&self) -> PaneHosting {
        PaneHosting::Either
    }
}

impl PaneBehavior for AgentChatContent {
    /// Nothing is cached to go stale: the live session title, else the
    /// first prompt, else "New {agent name}" — the precedence the in-pane
    /// activity bar uses.
    fn title(&self, cx: &App) -> SharedString {
        let view = self.view.read(cx);
        daruda_chat::agent_chat_helpers::activity_bar_title(view.session_title(), view.items())
            .map(SharedString::from)
            .unwrap_or_else(|| {
                crate::surface::strings::common::new_agent_chat_named(view.agent_name()).into()
            })
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.view.read(cx).focus_handle(cx)
    }

    /// `None` for a session on another machine: its cwd string leaking
    /// through would reach `spawn_pty`'s existence check, fall back to
    /// `$HOME` and skip the lane-local tier.
    fn cwd(&self) -> Option<&Path> {
        self.cwd.as_ref().and_then(PaneCwd::as_local)
    }

    /// An agent turn under way.
    fn runs_work(&self, cx: &App) -> bool {
        self.view.read(cx).is_busy()
    }

    fn account_selection(&self) -> Option<daruda_store::accounts::AccountSelection> {
        Some(self.account)
    }

    fn rail_icon(&self) -> &'static str {
        icons::SESSION
    }
}

impl PaneBehavior for PrPaneContent {
    fn title(&self, _cx: &App) -> SharedString {
        self.label.clone()
    }

    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn wrapper_focus_handle(&self) -> Option<&FocusHandle> {
        Some(&self.focus_handle)
    }

    fn rail_icon(&self) -> &'static str {
        icons::PULL_REQUESTS
    }

    fn hosting(&self) -> PaneHosting {
        PaneHosting::Side
    }
}

impl PaneContent {
    /// The content as the questions asked of every kind. The one place the
    /// kinds are told apart for them.
    pub(in crate::workspace) fn behavior(&self) -> &dyn PaneBehavior {
        match self {
            PaneContent::Terminal(terminal) => terminal,
            PaneContent::File(file) => file,
            PaneContent::AgentChat(chat) => chat,
            PaneContent::PullRequest(pr) => pr,
        }
    }

    pub(in crate::workspace) fn as_terminal(&self) -> Option<&TerminalContent> {
        if let PaneContent::Terminal(terminal) = self {
            Some(terminal)
        } else {
            None
        }
    }

    pub(in crate::workspace) fn as_terminal_mut(&mut self) -> Option<&mut TerminalContent> {
        if let PaneContent::Terminal(terminal) = self {
            Some(terminal)
        } else {
            None
        }
    }

    pub(in crate::workspace) fn as_file(&self) -> Option<&FileContent> {
        if let PaneContent::File(file) = self {
            Some(file)
        } else {
            None
        }
    }

    pub(in crate::workspace) fn as_file_mut(&mut self) -> Option<&mut FileContent> {
        if let PaneContent::File(file) = self {
            Some(file)
        } else {
            None
        }
    }

    pub(in crate::workspace) fn as_agent_chat(&self) -> Option<&AgentChatContent> {
        if let PaneContent::AgentChat(chat) = self {
            Some(chat)
        } else {
            None
        }
    }

    pub(in crate::workspace) fn as_agent_chat_mut(&mut self) -> Option<&mut AgentChatContent> {
        if let PaneContent::AgentChat(chat) = self {
            Some(chat)
        } else {
            None
        }
    }

    pub(in crate::workspace) fn as_pull_request(&self) -> Option<&PrPaneContent> {
        if let PaneContent::PullRequest(pr) = self {
            Some(pr)
        } else {
            None
        }
    }

    pub(in crate::workspace) fn as_pull_request_mut(&mut self) -> Option<&mut PrPaneContent> {
        if let PaneContent::PullRequest(pr) = self {
            Some(pr)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PaneHosting;

    #[test]
    fn a_kinds_hosting_says_which_groups_take_it() {
        // (hosting, the side tabs take it, it may sit among the tabs)
        let cases = [
            (PaneHosting::Tabs, false, true),
            (PaneHosting::Either, true, true),
            (PaneHosting::Side, true, false),
        ];
        for (hosting, aside, among_tabs) in cases {
            assert_eq!(hosting.aside(), aside, "{hosting:?}");
            assert_eq!(hosting.among_tabs(), among_tabs, "{hosting:?}");
        }
    }
}
