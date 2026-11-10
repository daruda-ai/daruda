//! Places a pane's message in its thread: decides the thread on the main
//! thread, posts off it, and records the root a post created.

use super::look::RootLook;
use super::plan::Placement;
pub(crate) use super::plan::ThreadPlan;
use super::{PaneConversation, Scope, now};
use crate::remote_channel::bridge::{MessageTail, PaneRef, PingWeight};
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::transport::{self, Credentials, Message, SentPart, slack};
use daruda_config::remote::{ChannelKind, RemoteRecipient};
use gpui::App;

/// Decide the thread for a message from `pane` on connection `id`, weighed
/// by the code that sent it. `None` when the channel has no pane threads or
/// the pane has no conversation yet; the message then goes out at the top as
/// before.
pub(crate) fn plan(
    id: &str,
    kind: ChannelKind,
    bot: &BotSession,
    recipient: &RemoteRecipient,
    pane: PaneRef,
    weight: PingWeight,
    cx: &mut App,
) -> Option<ThreadPlan> {
    if !transport::pane_threads(kind) {
        return None;
    }
    let conversation = conversation_of(pane, cx)?;
    let scope = Scope::serving(id, bot, recipient);
    Some(cx.global_mut::<RemoteChannels>().threads.plan_post(
        scope,
        pane,
        conversation,
        weight,
        now(),
    ))
}

pub(super) fn conversation_of(pane: PaneRef, cx: &mut App) -> Option<PaneConversation> {
    let mut found = None;
    crate::remote_channel::dispatch::target::dispatch_to_workspace(cx, pane.workspace, |ws, cx| {
        if found.is_none() {
            found = ws.remote_conversation(pane.pane, cx);
        }
    });
    found
}

/// What a thread post came to: the root it started, if any, and the
/// message's own result.
pub(crate) struct Posted {
    pub root: Option<String>,
    pub result: transport::Result<Vec<String>>,
}

/// Post `message` under the plan's thread, starting the thread first when
/// the pane has none. Blocking; runs off the main thread.
pub(crate) fn deliver(
    plan: &ThreadPlan,
    credentials: &Credentials,
    recipient: &RemoteRecipient,
    mut message: Message,
    on_sent: impl FnMut(SentPart),
) -> Posted {
    let (root, divider, broadcast, started) = match &plan.placement {
        Placement::Reply {
            root,
            divider,
            broadcast,
        } => (root.clone(), divider.as_ref(), *broadcast, None),
        Placement::Start => {
            let look = RootLook::new(&plan.name, plan.shown, &plan.id);
            match slack::threads::post_root(credentials, recipient, &look.view()) {
                Ok(root) => (root.clone(), None, false, Some(root)),
                Err(error) => {
                    crate::remote_channel::log_error(
                        "Remote pane thread failed to start",
                        &error,
                        "remote.thread.root",
                    );
                    // The message still reaches the person, at the top as it
                    // did before threads.
                    return Posted {
                        root: None,
                        result: slack::send(credentials, recipient, message, on_sent),
                    };
                }
            }
        }
    };
    if let Some(divider) = divider
        && let Err(error) = slack::threads::post_in_thread(
            credentials,
            recipient,
            &root,
            false,
            Message::plain(divider.clone(), None),
            |_| {},
        )
    {
        crate::remote_channel::log_error(
            "Remote thread divider failed to send",
            &error,
            "remote.thread.divider",
        );
    }
    drop_header_under_root(&mut message);
    Posted {
        root: started,
        result: slack::threads::post_in_thread(
            credentials,
            recipient,
            &root,
            broadcast,
            message,
            on_sent,
        ),
    }
}

/// The root names the pane, so a reply under it repeats only what is new —
/// unless the header is all the message has.
pub(super) fn drop_header_under_root(message: &mut Message) {
    let body = match &message.tail {
        MessageTail::Plain(text) | MessageTail::Markdown(text) => text,
    };
    if !body.trim().is_empty() {
        message.header.clear();
    }
}

/// Remember the thread a post started for the plan's conversation, and
/// bring its root up to the pane's status if that moved while it was posting.
pub(crate) fn record_root(plan: &ThreadPlan, root: String, cx: &mut App) {
    cx.global_mut::<RemoteChannels>()
        .threads
        .root_started(plan, root, now());
    super::roots::flush(cx);
}
