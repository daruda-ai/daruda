//! Every workspace's agent panes, arranged as its sidebar arranges their
//! projects, and what each workspace says of its panes now: the name each
//! one's thread root shows, and the conversation each is in, which finds its
//! thread even before the pane posts in this run.

use crate::control::result::ChatSummary;
use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::threads::{ConversationKey, RootName};
use crate::window_registry::WindowRegistry;
use crate::workspace::ArrangedSection;
use gpui::App;
use std::collections::HashMap;

/// One pane as its workspace sees it now.
pub(super) struct PaneNow {
    pub name: RootName,
    /// `None` until the pane's agent has a session.
    pub key: Option<ConversationKey>,
}

/// Every window's agent panes and how their workspaces see them.
pub(super) struct Panes {
    /// Window by window, in the order `/list` walks them, a window without
    /// panes included so each keeps its place.
    pub windows: Vec<Vec<ArrangedSection<ChatSummary>>>,
    pub now: HashMap<PaneRef, PaneNow>,
}

/// Every agent-chat pane, window by window. Enters every workspace, as the
/// listing does: call it outside any workspace's update.
pub(super) fn now(cx: &mut App) -> Panes {
    let mut panes = Panes {
        windows: Vec::new(),
        now: HashMap::new(),
    };
    WindowRegistry::for_each_workspace(cx, |ws, _window, cx| {
        let sections = ws.control_arrangement(cx);
        for summary in sections
            .iter()
            .flat_map(ArrangedSection::projects)
            .flat_map(|project| &project.lanes)
            .flat_map(|lane| &lane.panes)
        {
            let pane = summary.target;
            if let Some(name) = ws.remote_name(pane.pane, cx) {
                let key = ws.remote_key(pane.pane, cx);
                panes.now.insert(pane, PaneNow { name, key });
            }
        }
        panes.windows.push(sections);
    });
    panes
}

#[cfg(test)]
mod tests {
    use crate::workspace::ArrangedSection;

    /// A pane is arranged under its project and lane, named as its thread
    /// root names it, session or not, and found by the conversation it is
    /// in once it has a session.
    #[gpui::test]
    fn a_pane_is_arranged_named_and_found_by_its_session(cx: &mut gpui::TestAppContext) {
        let fixture = crate::test_support::workspace_with_agent_chat(cx);
        let pane = fixture.pane_ref(cx);
        let found = |cx: &mut gpui::TestAppContext| {
            cx.update(|cx| {
                let mut panes = super::now(cx);
                let arranged: Vec<_> = panes
                    .windows
                    .iter()
                    .flatten()
                    .flat_map(ArrangedSection::projects)
                    .flat_map(|project| &project.lanes)
                    .flat_map(|lane| &lane.panes)
                    .map(|summary| summary.target)
                    .collect();
                assert_eq!(arranged, [pane], "the one agent pane, arranged");
                panes.now.remove(&pane)
            })
        };
        let before = found(cx).expect("an agent-chat pane is named");
        assert!(!before.name.title.is_empty());
        assert_eq!(before.key, None, "no session yet");
        fixture.workspace.update(cx, |ws, cx| {
            ws.set_pane_session_for_test(fixture.pane(), "s1", cx);
        });
        let after = found(cx).expect("still named");
        let root = fixture
            .workspace
            .update(cx, |ws, cx| ws.remote_conversation(fixture.pane(), cx))
            .expect("a pane with a session has a conversation");
        assert_eq!(after.name, root.name, "named as its thread root is");
        assert_eq!(after.key, Some(root.key), "the session it is in");
        assert_eq!(after.key.map(|key| key.session).as_deref(), Some("s1"));
    }
}
