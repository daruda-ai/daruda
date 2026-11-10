//! Places a pane's message in its thread: decides the thread on the main
//! thread, posts off it, and records the root a post created.

use super::{Book, ConversationKey, PaneConversation, Scope};
use crate::remote_channel::bridge::{MessageTail, PaneRef};
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::transport::{self, Credentials, Message, slack};
use crate::surface::strings as s;
use daruda_config::remote::{ChannelKind, RemoteRecipient};
use daruda_store::remote_threads::RemoteThreadStore;
use gpui::App;
use std::collections::HashMap;

/// Every connection's pane threads, and where they persist.
pub(crate) struct Threads {
    book: Book,
    /// The conversation each pane last spoke from on each connection: how a
    /// pane that starts a new conversation finds the thread its previous one
    /// used. Memory only — after a restart a pane is found by the
    /// conversation it resumes.
    last: HashMap<(String, PaneRef), ConversationKey>,
    store: Option<RemoteThreadStore>,
}

impl Threads {
    pub(crate) fn load() -> Self {
        // Tests never read or write the profile's records.
        let store = (!cfg!(test)).then(RemoteThreadStore::current);
        let records = store.as_ref().map(RemoteThreadStore::load);
        let mut threads = Self {
            book: Book::from_records(records.unwrap_or_default()),
            last: HashMap::new(),
            store,
        };
        if threads.book.prune(now()) {
            threads.save();
        }
        threads
    }

    /// The thread `conversation` of `pane` speaks in, and the divider owed
    /// when the pane carried its thread over from an earlier conversation.
    /// `None` root: the pane has no thread on this connection yet.
    pub(super) fn place(
        &mut self,
        scope: &Scope,
        pane: PaneRef,
        conversation: &PaneConversation,
        now: i64,
    ) -> (Option<String>, Option<String>) {
        let previous = self
            .last
            .insert((scope.connection.clone(), pane), conversation.key.clone());
        if let Some(record) = self.book.for_conversation(scope, &conversation.key) {
            let root = record.root.clone();
            if self.book.touch(scope, &root, now) {
                self.save();
            }
            return (Some(root), None);
        }
        let Some(previous) = previous else {
            return (None, None);
        };
        match self
            .book
            .continue_with(scope, &previous, conversation.key.clone(), now)
        {
            Some(root) => {
                self.save();
                (Some(root), Some(divider_text(&previous, conversation)))
            }
            None => (None, None),
        }
    }

    /// Record the thread a post started for `key`.
    pub(super) fn remember(&mut self, scope: &Scope, key: ConversationKey, root: String, now: i64) {
        self.book.insert(scope, key, root, now);
        self.book.prune(now);
        self.save();
    }

    fn save(&self) {
        if let Some(store) = &self.store
            && let Err(error) = store.save(self.book.records())
        {
            crate::remote_channel::log_error(
                "Remote thread records failed to save",
                &error,
                "remote.thread.save",
            );
        }
    }
}

/// Where one pane message goes.
pub(crate) struct ThreadPlan {
    scope: Scope,
    key: ConversationKey,
    title: String,
    /// `None`: the pane has no thread yet, and the post starts one.
    root: Option<String>,
    /// Said in the thread first: the pane moved to a new conversation.
    divider: Option<String>,
}

/// Decide the thread for a message from `pane` on connection `id`. `None`
/// when the channel has no pane threads or the pane has no conversation yet;
/// the message then goes out at the top as before.
pub(crate) fn plan(
    id: &str,
    kind: ChannelKind,
    bot: &BotSession,
    recipient: &RemoteRecipient,
    pane: PaneRef,
    cx: &mut App,
) -> Option<ThreadPlan> {
    if !transport::pane_threads(kind) {
        return None;
    }
    let conversation = conversation_of(pane, cx)?;
    let scope = Scope {
        connection: id.to_owned(),
        bot: crate::remote_channel::lock::digest_of(&bot.credentials.bot),
        conversation: recipient.conversation_id.clone(),
    };
    let threads = &mut cx.global_mut::<RemoteChannels>().threads;
    let (root, divider) = threads.place(&scope, pane, &conversation, now());
    Some(ThreadPlan {
        scope,
        key: conversation.key,
        title: conversation.title,
        root,
        divider,
    })
}

fn divider_text(previous: &ConversationKey, now: &PaneConversation) -> String {
    if previous.agent == now.key.agent {
        s::remote_channel::thread_new_conversation()
    } else {
        s::remote_channel::thread_switched_agent(&now.agent_name)
    }
}

fn conversation_of(pane: PaneRef, cx: &mut App) -> Option<PaneConversation> {
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
    on_sent: impl FnMut(&str),
) -> Posted {
    let (root, started) = match &plan.root {
        Some(root) => (root.clone(), None),
        None => match slack::threads::post_root(credentials, recipient, &plan.title) {
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
        },
    };
    if let Some(divider) = &plan.divider
        && let Err(error) = slack::threads::post_in_thread(
            credentials,
            recipient,
            &root,
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
        result: slack::threads::post_in_thread(credentials, recipient, &root, message, on_sent),
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

/// Remember the thread a post started for the plan's conversation.
pub(crate) fn record_root(plan: &ThreadPlan, root: String, cx: &mut App) {
    cx.global_mut::<RemoteChannels>()
        .threads
        .remember(&plan.scope, plan.key.clone(), root, now());
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}
