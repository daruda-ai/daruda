//! Places a pane's message in its thread: decides the thread on the main
//! thread, posts off it, and records the root a post created.

use super::roots::{RootFace, Roots};
use super::status::ShownStatus;
use super::{Book, ConversationKey, PaneConversation, Scope};
use crate::remote_channel::bridge::{MessageTail, PaneRef};
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::transport::{self, Credentials, Message, SentPart, slack};
use crate::surface::strings as s;
use daruda_config::remote::{ChannelKind, RemoteRecipient};
use daruda_store::remote_threads::RemoteThreadStore;
use gpui::App;
use std::collections::HashMap;

/// Every connection's pane threads, and where they persist.
pub(crate) struct Threads {
    pub(super) book: Book,
    /// The conversation each pane last spoke from on each connection: how a
    /// pane that starts a new conversation finds the thread its previous one
    /// used. Memory only — after a restart a pane is found by the
    /// conversation it resumes.
    pub(super) last: HashMap<(String, PaneRef), ConversationKey>,
    /// The title each pane's root on each connection was last given. Memory
    /// only, like `last`.
    pub(super) titles: HashMap<(String, PaneRef), String>,
    /// Each pane's latest status, for roots made later. A closed pane keeps
    /// `Closed` here so a root still being posted for it lands closed.
    pub(super) activity: HashMap<PaneRef, ShownStatus>,
    pub(super) roots: Roots,
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
            titles: HashMap::new(),
            activity: HashMap::new(),
            roots: Roots::default(),
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

    /// After placing a post from `pane`: note the root's title on this
    /// connection, and bring an existing root up to date. After a restart this
    /// run does not know what the root shows; one edit settles it, and a root
    /// that already shows this face is not edited again. The thread's id —
    /// its record's, or a new one for the root this post starts.
    pub(super) fn want_root_after_place(
        &mut self,
        scope: &Scope,
        pane: PaneRef,
        root: Option<&str>,
        conversation: &PaneConversation,
    ) -> String {
        self.titles
            .insert((scope.connection.clone(), pane), conversation.title.clone());
        let id = root
            .and_then(|root| self.book.for_root(scope, root))
            .map_or_else(
                || uuid::Uuid::new_v4().simple().to_string(),
                |record| record.id.clone(),
            );
        if let (Some(root), Some(&shown)) = (root, self.activity.get(&pane)) {
            self.roots.want(
                scope,
                &id,
                root.to_owned(),
                RootFace {
                    title: conversation.title.clone(),
                    shown,
                },
            );
        }
        id
    }

    /// Record the thread `id` a post started for `key`.
    pub(crate) fn remember(
        &mut self,
        scope: &Scope,
        id: String,
        key: ConversationKey,
        root: String,
        now: i64,
    ) {
        self.book.insert(scope, id, key, root, now);
        self.book.prune(now);
        self.save();
    }

    pub(super) fn save(&self) {
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
    /// The thread's id: its record's, or the one a root started now takes.
    id: String,
    pane: PaneRef,
    key: ConversationKey,
    title: String,
    /// What a root started by this post shows.
    shown: Option<ShownStatus>,
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
    let scope = scope(id, bot, recipient);
    let threads = &mut cx.global_mut::<RemoteChannels>().threads;
    let (root, divider) = threads.place(&scope, pane, &conversation, now());
    let shown = threads.activity.get(&pane).copied();
    let id = threads.want_root_after_place(&scope, pane, root.as_deref(), &conversation);
    Some(ThreadPlan {
        scope,
        id,
        pane,
        key: conversation.key,
        title: conversation.title,
        shown,
        root,
        divider,
    })
}

/// The identity a connection's threads are made under.
pub(super) fn scope(id: &str, bot: &BotSession, recipient: &RemoteRecipient) -> Scope {
    Scope {
        connection: id.to_owned(),
        bot: crate::remote_channel::lock::digest_of(&bot.credentials.bot),
        conversation: recipient.conversation_id.clone(),
    }
}

fn divider_text(previous: &ConversationKey, now: &PaneConversation) -> String {
    if previous.agent == now.key.agent {
        s::remote_channel::thread_new_conversation()
    } else {
        s::remote_channel::thread_switched_agent(&now.agent_name)
    }
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
    let status = plan.shown.map(ShownStatus::view);
    let stop = plan.shown.filter(|shown| shown.is_running()).map(|_| {
        (
            s::remote_channel::thread_stop_button(),
            super::stop::token(&plan.id),
        )
    });
    let view = slack::threads::RootView {
        title: &plan.title,
        status: status
            .as_ref()
            .map(|(emoji, label)| (*emoji, label.as_str())),
        stop: stop
            .as_ref()
            .map(|(label, value)| (label.as_str(), value.as_str())),
    };
    let (root, started) = match &plan.root {
        Some(root) => (root.clone(), None),
        None => match slack::threads::post_root(credentials, recipient, &view) {
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
    let face = |shown| RootFace {
        title: plan.title.clone(),
        shown,
    };
    if let Some(shown) = plan.shown {
        threads.roots.shown(&plan.scope, &root, face(shown));
    }
    if let Some(&current) = threads.activity.get(&plan.pane)
        && plan.shown != Some(current)
    {
        threads
            .roots
            .want(&plan.scope, &plan.id, root, face(current));
    }
    super::roots::flush(cx);
}

pub(super) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}
