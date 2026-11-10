//! Every connection's pane threads: the persisted thread table, what this run
//! knows of each pane, and the root updates owed. Fields are private; each
//! transition that touches the table also saves it, in one method.

use super::roots::{RootFace, Roots};
use super::status::{PaneActivity, ShownStatus};
use super::{Book, ConversationKey, PaneConversation, RootName, Scope, now};
use crate::remote_channel::bridge::PaneRef;
use crate::surface::strings as s;
use daruda_store::remote_threads::RemoteThreadStore;
use std::collections::HashMap;

pub(crate) struct Threads {
    book: Book,
    /// The conversation each pane last spoke from on each connection: how a
    /// pane that starts a new conversation finds the thread its previous one
    /// used. Memory only — after a restart a pane is found by the
    /// conversation it resumes.
    last: HashMap<(String, PaneRef), ConversationKey>,
    /// The name each pane's root on each connection was last given. Memory
    /// only, like `last`.
    names: HashMap<(String, PaneRef), RootName>,
    /// Each pane's latest status, for roots made later. A closed pane keeps
    /// `Closed` here so a root still being posted for it lands closed.
    activity: HashMap<PaneRef, ShownStatus>,
    roots: Roots,
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
            names: HashMap::new(),
            activity: HashMap::new(),
            roots: Roots::default(),
            store,
        };
        if threads.book.prune(now()) {
            threads.save();
        }
        threads
    }

    pub(super) fn roots(&mut self) -> &mut Roots {
        &mut self.roots
    }

    // --- Placing a post -------------------------------------------------

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
        self.names
            .insert((scope.connection.clone(), pane), conversation.name.clone());
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
                    name: conversation.name.clone(),
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

    /// Note what a root just posted shows, and ask for the pane's current
    /// status if it moved on while the root was posting.
    pub(super) fn root_posted(
        &mut self,
        scope: &Scope,
        id: &str,
        root: &str,
        pane: PaneRef,
        name: &RootName,
        posted: Option<ShownStatus>,
    ) {
        let face = |shown| RootFace {
            name: name.clone(),
            shown,
        };
        if let Some(shown) = posted {
            self.roots.shown(scope, root, face(shown));
        }
        if let Some(&current) = self.activity.get(&pane)
            && posted != Some(current)
        {
            self.roots.want(scope, id, root.to_owned(), face(current));
        }
    }

    // --- Pane activity ----------------------------------------------------

    pub(super) fn activity_of(&self, pane: PaneRef) -> Option<ShownStatus> {
        self.activity.get(&pane).copied()
    }

    /// Note `pane`'s activity. The status its roots should now show, or
    /// `None` when nothing changed.
    pub(super) fn note_activity(
        &mut self,
        pane: PaneRef,
        activity: PaneActivity,
        now: i64,
    ) -> Option<ShownStatus> {
        let previous = self.activity.get(&pane).copied();
        let shown = ShownStatus::next(previous, activity, now);
        if previous == Some(shown) {
            return None;
        }
        self.activity.insert(pane, shown);
        Some(shown)
    }

    /// Ask every root `pane` has to show `shown`. Found through the threads
    /// the pane posted in this run, whatever state their connections are in:
    /// a connection that cannot send now is caught up on its retry tick.
    pub(super) fn want_for_pane(&mut self, pane: PaneRef, shown: ShownStatus) {
        let wanted: Vec<_> = self
            .last
            .iter()
            .filter(|((_, held), _)| *held == pane)
            .filter_map(|((connection, _), key)| {
                let record = self.book.for_connection(connection, key)?;
                let name = self.names.get(&(connection.clone(), pane))?;
                Some((
                    Scope::of(record),
                    record.id.clone(),
                    record.root.clone(),
                    name.clone(),
                ))
            })
            .collect();
        for (scope, id, root, name) in wanted {
            self.roots.want(&scope, &id, root, RootFace { name, shown });
        }
    }

    /// Give `pane`'s roots its latest name, for the next update they get. The
    /// conversation's subject usually arrives mid-turn, after the post that
    /// last named it.
    pub(super) fn rename_pane(&mut self, pane: PaneRef, name: &RootName) {
        for (_, held) in self.names.iter_mut().filter(|((_, held), _)| *held == pane) {
            *held = name.clone();
        }
    }

    /// Forget where a closed pane posted. Its `Closed` status is kept, so a
    /// root still being posted for it is closed as soon as it lands.
    pub(super) fn forget_pane(&mut self, pane: PaneRef) {
        self.last.retain(|(_, held), _| *held != pane);
        self.names.retain(|(_, held), _| *held != pane);
    }

    // --- Finding a thread's pane -------------------------------------------

    /// The conversations of the thread rooted at `root`.
    pub(super) fn sessions_of_root(
        &self,
        scope: &Scope,
        root: &str,
    ) -> Option<Vec<ConversationKey>> {
        Some(self.book.for_root(scope, root)?.sessions.clone())
    }

    /// The conversations of the thread named `id`.
    pub(super) fn sessions_of_thread(
        &self,
        scope: &Scope,
        id: &str,
    ) -> Option<Vec<ConversationKey>> {
        Some(self.book.by_id(scope, id)?.sessions.clone())
    }

    /// The panes whose last post on `connection` came from one of
    /// `conversations`: a pane that has since moved on to a new conversation
    /// is still the thread's pane until it posts from it.
    pub(super) fn panes_last_in(
        &self,
        connection: &str,
        conversations: &[ConversationKey],
    ) -> Vec<PaneRef> {
        self.last
            .iter()
            .filter(|((held, _), key)| held == connection && conversations.contains(key))
            .map(|((_, pane), _)| *pane)
            .collect()
    }

    /// Forget the thread rooted at `root`: its message is gone.
    pub(super) fn forget_root(&mut self, scope: &Scope, root: &str) {
        if self.book.forget_root(scope, root) {
            self.save();
        }
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

    /// Whether this run still knows where `pane` posted.
    #[cfg(test)]
    pub(super) fn knows_pane(&self, pane: PaneRef) -> bool {
        self.last.keys().any(|(_, held)| *held == pane)
            || self.names.keys().any(|(_, held)| *held == pane)
    }

    #[cfg(test)]
    pub(super) fn roots_ref(&self) -> &Roots {
        &self.roots
    }
}

fn divider_text(previous: &ConversationKey, now: &PaneConversation) -> String {
    if previous.agent == now.key.agent {
        s::remote_channel::thread_new_conversation()
    } else {
        s::remote_channel::thread_switched_agent(&now.agent_name)
    }
}
