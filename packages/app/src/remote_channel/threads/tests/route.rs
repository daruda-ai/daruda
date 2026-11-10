use super::super::route::{ThreadReply, route_reply};
use super::*;
use crate::remote_channel::global::RemoteChannels;

/// The defect this guards: after `/clear` or a switch, the pane's thread did
/// not list its new conversation until the pane next posted, so replies and
/// stop taps in the thread were refused while the pane was open and running.
#[gpui::test]
fn a_pane_that_moved_to_a_new_conversation_still_answers_in_its_thread(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = crate::test_support::workspace_with_agent_chat(cx);
    let pane_id = fixture.pane();
    let key = fixture.workspace.update(cx, |ws, cx| {
        ws.set_pane_session_for_test(pane_id, "s1", cx);
        ws.remote_conversation(pane_id, cx).unwrap()
    });
    let pane = fixture.pane_ref(cx);
    let bot = cx.update(|cx| {
        let _outgoing = crate::remote_channel::global::tests::setup(cx);
        let channels = cx.global_mut::<RemoteChannels>();
        let connection = &channels.connections["work"];
        let bot = connection.bot.clone().unwrap();
        let scope = Scope::serving("work", &bot, connection.config.recipient.as_ref().unwrap());
        channels.threads.place(&scope, pane, &key, NOW);
        channels
            .threads
            .remember(&scope, "t1".into(), key.key.clone(), "1.1".into(), NOW);
        bot
    });
    fixture
        .workspace
        .update(cx, |ws, cx| ws.set_pane_session_for_test(pane_id, "s2", cx));
    cx.update(|cx| {
        assert_eq!(
            route_reply("work", &bot, Some("1.1"), cx),
            ThreadReply::Pane(pane)
        );
    });

    // Once the conversation the pane moved to has a thread of its own, that
    // thread speaks for the pane; the old one no longer reaches it.
    cx.update(|cx| {
        let channels = cx.global_mut::<RemoteChannels>();
        let connection = &channels.connections["work"];
        let scope = Scope::serving("work", &bot, connection.config.recipient.as_ref().unwrap());
        let moved = ConversationKey {
            agent: key.key.agent.clone(),
            session: "s2".into(),
        };
        channels
            .threads
            .remember(&scope, "t2".into(), moved, "2.2".into(), NOW);
        assert_eq!(
            route_reply("work", &bot, Some("1.1"), cx),
            ThreadReply::Closed
        );
    });
}
