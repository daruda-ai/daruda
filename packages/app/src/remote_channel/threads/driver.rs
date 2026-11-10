//! Places a pane's message in its thread: decides the thread on the main
//! thread, posts off it, and records the root a post created.

use super::roots::RootLook;
use super::status::ShownStatus;
use super::{ConversationKey, PaneConversation, RootName, Scope, now};
use crate::remote_channel::bridge::{MessageTail, PaneRef};
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::transport::{self, Credentials, Message, SentPart, slack};
use daruda_config::remote::{ChannelKind, RemoteRecipient};
use gpui::App;

/// Where one pane message goes.
pub(crate) struct ThreadPlan {
    scope: Scope,
    /// The thread's id: its record's, or the one a root started now takes.
    id: String,
    pane: PaneRef,
    key: ConversationKey,
    name: RootName,
    /// What a root started by this post shows.
    shown: Option<ShownStatus>,
    /// `None`: the pane has no thread yet, and the post starts one.
    root: Option<String>,
    /// Said in the thread first: the pane moved to a new conversation.
    divider: Option<String>,
    /// Also shown in the conversation, where the root has scrolled away.
    broadcast: bool,
}

/// Decide the thread for a message from `pane` on connection `id`; `carded`
/// when it asks the person something with buttons. `None` when the channel
/// has no pane threads or the pane has no conversation yet; the message then
/// goes out at the top as before.
pub(crate) fn plan(
    id: &str,
    kind: ChannelKind,
    bot: &BotSession,
    recipient: &RemoteRecipient,
    pane: PaneRef,
    carded: bool,
    cx: &mut App,
) -> Option<ThreadPlan> {
    if !transport::pane_threads(kind) {
        return None;
    }
    let conversation = conversation_of(pane, cx)?;
    let scope = Scope::serving(id, bot, recipient);
    let threads = &mut cx.global_mut::<RemoteChannels>().threads;
    let (root, divider) = threads.place(&scope, pane, &conversation, now());
    let shown = threads.activity_of(pane);
    let id = threads.want_root_after_place(&scope, pane, root.as_deref(), &conversation);
    let broadcast = surfaces(root.is_some(), carded, shown);
    Some(ThreadPlan {
        scope,
        id,
        pane,
        key: conversation.key,
        name: conversation.name,
        shown,
        root,
        divider,
        broadcast,
    })
}

/// Whether a reply also shows in the conversation. A long-lived thread's
/// root scrolls away, so what the person must see — a turn's result, or a
/// question with buttons — surfaces there too; progress stays in the
/// thread. A post that starts its thread is already at the bottom.
pub(super) fn surfaces(existing_root: bool, carded: bool, shown: Option<ShownStatus>) -> bool {
    existing_root && (carded || matches!(shown, Some(ShownStatus::Ended(_))))
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
    let (root, started) = match &plan.root {
        Some(root) => (root.clone(), None),
        None => {
            let look = RootLook::new(&plan.name, plan.shown, &plan.id);
            match slack::threads::post_root(credentials, recipient, &look.view()) {
                Ok(root) => (root.clone(), Some(root)),
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
    if let Some(divider) = &plan.divider
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
            plan.broadcast,
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
    let threads = &mut cx.global_mut::<RemoteChannels>().threads;
    threads.remember(
        &plan.scope,
        plan.id.clone(),
        plan.key.clone(),
        root.clone(),
        now(),
    );
    threads.root_posted(
        &plan.scope,
        &plan.id,
        &root,
        plan.pane,
        &plan.name,
        plan.shown,
    );
    super::roots::flush(cx);
}
