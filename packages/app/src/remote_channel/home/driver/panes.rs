//! What the listed panes' workspaces say of them now: the conversation each
//! is in, which finds its thread even before the pane posts in this run.

use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::command::ListingRow;
use crate::remote_channel::threads::ConversationKey;
use crate::window_registry::WindowRegistry;
use gpui::App;
use std::collections::HashMap;

/// The conversation of every pane in `listing` that has a session. Enters
/// every workspace, as the listing does: call it outside any workspace's
/// update.
pub(super) fn conversations(
    listing: &[ListingRow],
    cx: &mut App,
) -> HashMap<PaneRef, ConversationKey> {
    let mut found = HashMap::new();
    WindowRegistry::for_each_workspace(cx, |ws, _window, cx| {
        let workspace = ws.uuid();
        for pane in listing
            .iter()
            .map(|row| row.summary.target)
            .filter(|pane| pane.workspace == workspace)
        {
            if let Some(key) = ws.remote_key(pane.pane, cx) {
                found.insert(pane, key);
            }
        }
    });
    found
}

#[cfg(test)]
mod tests {
    /// A listed pane is found by the conversation it is in once it has a
    /// session, and not before.
    #[gpui::test]
    fn a_listed_pane_is_found_by_its_session(cx: &mut gpui::TestAppContext) {
        let fixture = crate::test_support::workspace_with_agent_chat(cx);
        let pane = fixture.pane_ref(cx);
        let found = |cx: &mut gpui::TestAppContext| {
            cx.update(|cx| {
                let listing =
                    crate::remote_channel::command::flatten(&crate::control::exec::listing(cx));
                assert!(listing.iter().any(|row| row.summary.target == pane));
                super::conversations(&listing, cx).remove(&pane)
            })
        };
        assert_eq!(found(cx), None, "no session yet");
        fixture.workspace.update(cx, |ws, cx| {
            ws.set_pane_session_for_test(fixture.pane(), "s1", cx);
        });
        assert_eq!(
            found(cx).map(|key| key.session),
            Some("s1".to_owned()),
            "the session it is in"
        );
    }
}
