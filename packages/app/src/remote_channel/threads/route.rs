//! The inbound side of pane threads: which pane a reply or a tap in a thread
//! is for, and where the answer to it goes.

use super::{ConversationKey, Scope};
use crate::remote_channel::bridge::{PaneRef, Routed};
use crate::remote_channel::command::RenderedReply;
use crate::remote_channel::dispatch::{self, Effect};
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::transport::{self, Credentials, IncomingKind, Message, slack};
use crate::surface::strings as s;
use daruda_config::remote::{ChannelKind, RemoteRecipient};
use gpui::App;

/// Decide what a text message on connection `id` comes to: a routed action,
/// or an answer given instead. A reply to a question card answers it; a reply
/// in a pane's thread goes to that pane, where a bare stop stops that pane;
/// a thread this app cannot place takes only daruda's own commands. `None`
/// when the connection is gone.
pub(crate) fn route_message(
    id: &str,
    bot: &BotSession,
    text: String,
    reply_to: Option<&String>,
    cx: &mut App,
) -> Option<Result<Routed, Effect>> {
    let card = cx
        .global::<RemoteChannels>()
        .connections
        .get(id)?
        .question_reply(reply_to);
    let thread = match card {
        Some(_) => ThreadReply::Untracked,
        None => route_reply(id, bot, reply_to.map(String::as_str), cx),
    };
    let target = dispatch::Target::Remote(id.into());
    let connection = cx.global_mut::<RemoteChannels>().connections.get_mut(id)?;
    Some(match thread {
        ThreadReply::Untracked => Ok(connection.core.route_text(text, reply_to.cloned(), card)),
        ThreadReply::Pane(pane) if is_bare_stop(&text) => {
            Err(dispatch::stop_pane(pane, &target, cx))
        }
        ThreadReply::Pane(pane) => {
            let card = connection.live_question_of(pane);
            Ok(connection.core.route_text_to(text, pane, card))
        }
        // A bare stop here would fall to the selection: another agent.
        ThreadReply::Closed | ThreadReply::Unknown if is_command(&text) && !is_bare_stop(&text) => {
            Ok(connection.core.route_text(text, None, None))
        }
        ThreadReply::Closed => Err(refusal(s::remote_channel::thread_closed())),
        ThreadReply::Unknown => Err(refusal(s::remote_channel::thread_unknown())),
    })
}

/// Whether `text` is one of daruda's commands, used rightly or not.
fn is_command(text: &str) -> bool {
    use crate::control::spec::{ParseError, parse};
    !matches!(
        parse(text),
        Err(ParseError::NotACommand | ParseError::Unknown { .. })
    )
}

/// Whether `text` is `/stop` naming no pane.
fn is_bare_stop(text: &str) -> bool {
    matches!(
        crate::control::spec::parse(text),
        Ok(crate::control::spec::ControlCommand::Stop { target: None })
    )
}

fn refusal(text: String) -> Effect {
    Effect::Reply(RenderedReply {
        text,
        keyboard: None,
    })
}

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
    let scope = Scope::serving(id, bot, recipient);
    let Some(conversations) = channels.threads.sessions_of_root(&scope, thread) else {
        return ThreadReply::Unknown;
    };
    match pane_holding(&scope, &conversations, cx) {
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
    let scope = Scope::serving(id, bot, recipient);
    let conversations = channels.threads.sessions_of_thread(&scope, thread)?;
    pane_holding(&scope, &conversations, cx)
}

/// The open pane whose conversation is one of `conversations`. A pane that
/// has moved on to a new conversation — cleared, or switched agent — is not
/// listed under it until it next posts, so the conversation it last posted
/// from on connection `id` still names it; unless the pane moved to a
/// conversation that has a thread of its own, which now speaks for it.
fn pane_holding(scope: &Scope, conversations: &[ConversationKey], cx: &mut App) -> Option<PaneRef> {
    if let Some(pane) = open_pane_with(conversations, cx) {
        return Some(pane);
    }
    let moved_on = cx
        .global::<RemoteChannels>()
        .threads
        .panes_last_in(&scope.connection, conversations);
    moved_on.into_iter().find(|pane| {
        if !is_open(*pane, cx) {
            return false;
        }
        match super::driver::conversation_of(*pane, cx) {
            Some(now) => !cx
                .global::<RemoteChannels>()
                .threads
                .has_thread(scope, &now.key),
            // Still connecting its new conversation: nothing else speaks for it.
            None => true,
        }
    })
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
            slack::threads::post_in_thread(credentials, recipient, thread, false, message, |_| {})
        }
        None => transport::send(kind, credentials, recipient, message),
    }
    .map(drop)
}
