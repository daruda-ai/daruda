//! What the listed panes' workspaces say of them now: the name each one's
//! thread root shows, and the conversation each is in, which finds its
//! thread even before the pane posts in this run.

use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::command::ListingRow;
use crate::remote_channel::threads::{ConversationKey, RootName};
use crate::window_registry::WindowRegistry;
use gpui::App;
use std::collections::HashMap;

/// One listed pane as its workspace sees it now.
pub(super) struct PaneNow {
    pub name: RootName,
    /// `None` until the pane's agent has a session.
    pub key: Option<ConversationKey>,
}

/// Every agent-chat pane in `listing` as its workspace sees it now. Enters
/// every workspace, as the listing does: call it outside any workspace's
/// update.
pub(super) fn now(listing: &[ListingRow], cx: &mut App) -> HashMap<PaneRef, PaneNow> {
    let mut found = HashMap::new();
    WindowRegistry::for_each_workspace(cx, |ws, _window, cx| {
        let workspace = ws.uuid();
        for pane in listing
            .iter()
            .map(|row| row.summary.target)
            .filter(|pane| pane.workspace == workspace)
        {
            if let Some(name) = ws.remote_name(pane.pane, cx) {
                let key = ws.remote_key(pane.pane, cx);
                found.insert(pane, PaneNow { name, key });
            }
        }
    });
    found
}

#[cfg(test)]
mod tests {
    /// A listed pane is named as its thread root names it, session or not,
    /// and found by the conversation it is in once it has a session.
    #[gpui::test]
    fn a_listed_pane_is_named_and_found_by_its_session(cx: &mut gpui::TestAppContext) {
        let fixture = crate::test_support::workspace_with_agent_chat(cx);
        let pane = fixture.pane_ref(cx);
        let found = |cx: &mut gpui::TestAppContext| {
            cx.update(|cx| {
                let listing =
                    crate::remote_channel::command::flatten(&crate::control::exec::listing(cx));
                assert!(listing.iter().any(|row| row.summary.target == pane));
                super::now(&listing, cx).remove(&pane)
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
