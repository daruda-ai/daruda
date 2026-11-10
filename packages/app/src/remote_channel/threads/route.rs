//! The inbound side of pane threads: which pane a reply or a tap in a thread
//! is for, and where the answer to it goes.

use super::ConversationKey;
use super::driver::scope;
use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::transport::{self, Credentials, IncomingKind, Message, slack};
use daruda_config::remote::{ChannelKind, RemoteRecipient};
use gpui::App;

/// Where a reply posted in a thread goes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ThreadReply {
    /// Not a pane thread on this channel: routed as before threads.
    Untracked,
    /// The open pane the thread speaks for.
    Pane(PaneRef),
    /// A pane thread whose pane is open in no window.
    Closed,
    /// A thread no pane started and no message here answers.
    Unknown,
}

/// Route a reply posted under `thread` on connection `id`. A message sent
/// before threads keeps its old routing; a thread this app cannot place is
/// refused rather than guessed, since a guess delivers to the wrong agent.
pub(crate) fn route_reply(
    id: &str,
    bot: &BotSession,
    thread: Option<&str>,
    cx: &mut App,
) -> ThreadReply {
    let Some(thread) = thread else {
        return ThreadReply::Untracked;
    };
    let channels = cx.global::<RemoteChannels>();
    let Some(connection) = channels.connections.get(id) else {
        return ThreadReply::Untracked;
    };
    let Some(recipient) = &connection.config.recipient else {
        return ThreadReply::Untracked;
    };
    if !transport::pane_threads(connection.config.kind)
        || connection.core.sent_pings.contains_key(thread)
    {
        return ThreadReply::Untracked;
    }
    let scope = scope(id, bot, recipient);
    let Some(record) = channels.threads.book.for_root(&scope, thread) else {
        return ThreadReply::Unknown;
    };
    let conversations = record.sessions.clone();
    match pane_holding(id, &conversations, cx) {
        Some(pane) => ThreadReply::Pane(pane),
        None => ThreadReply::Closed,
    }
}

/// The open pane the thread `thread` speaks for, on connection `id`.
pub(super) fn pane_of_thread(
    id: &str,
    bot: &BotSession,
    thread: &str,
    cx: &mut App,
) -> Option<PaneRef> {
    let channels = cx.global::<RemoteChannels>();
    let recipient = channels.connections.get(id)?.config.recipient.as_ref()?;
    let scope = scope(id, bot, recipient);
    let conversations = channels
        .threads
        .book
        .by_id(&scope, thread)?
        .sessions
        .clone();
    pane_holding(id, &conversations, cx)
}

/// The open pane whose conversation is one of `conversations`. A pane that
/// has moved on to a new conversation — cleared, or switched agent — is not
/// listed under it until it next posts, so the conversation it last posted
/// from on connection `id` still names it.
fn pane_holding(id: &str, conversations: &[ConversationKey], cx: &mut App) -> Option<PaneRef> {
    if let Some(pane) = open_pane_with(conversations, cx) {
        return Some(pane);
    }
    let moved_on: Vec<PaneRef> = cx
        .global::<RemoteChannels>()
        .threads
        .last
        .iter()
        .filter(|((connection, _), key)| connection == id && conversations.contains(key))
        .map(|((_, pane), _)| *pane)
        .collect();
    moved_on.into_iter().find(|pane| is_open(*pane, cx))
}

fn open_pane_with(conversations: &[ConversationKey], cx: &mut App) -> Option<PaneRef> {
    let mut found = None;
    crate::window_registry::WindowRegistry::for_each_workspace(cx, |ws, _window, cx| {
        if found.is_none()
            && let Some(pane) = ws.pane_with_conversation(conversations, cx)
        {
            found = Some(PaneRef {
                workspace: ws.uuid(),
                pane,
            });
        }
    });
    found
}

fn is_open(pane: PaneRef, cx: &mut App) -> bool {
    let mut open = false;
    crate::remote_channel::dispatch::target::dispatch_to_workspace(cx, pane.workspace, |ws, _| {
        open |= ws.has_agent_chat(pane.pane)
    });
    open
}

/// The thread a reply to `incoming` belongs in: the one it was posted in,
/// or the one whose root holds the tapped stop button.
pub(crate) fn reply_thread(kind: ChannelKind, incoming: &IncomingKind) -> Option<String> {
    if !transport::pane_threads(kind) {
        return None;
    }
    match incoming {
        IncomingKind::Message {
            reply_to: Some(thread),
            ..
        } => Some(thread.clone()),
        IncomingKind::Callback {
            data, message_id, ..
        } if super::stop::thread_of(data).is_some() => Some(message_id.clone()),
        IncomingKind::Message { .. } | IncomingKind::Callback { .. } => None,
    }
}

/// Send a reply in `thread`, or at the top when there is none. Blocking.
pub(crate) fn send_reply(
    kind: ChannelKind,
    credentials: &Credentials,
    recipient: &RemoteRecipient,
    thread: Option<&str>,
    message: Message,
) -> transport::Result<()> {
    match thread {
        Some(thread) => {
            slack::threads::post_in_thread(credentials, recipient, thread, message, |_| {})
        }
        None => transport::send(kind, credentials, recipient, message),
    }
    .map(drop)
}
