//! Every connection's pane threads: the persisted thread table, what this run
//! knows of each pane, and the root updates owed. Fields are private; each
//! transition that touches the table also saves it, in one method.

use super::plan::{Placement, ThreadPlan};
use super::root_queue::{Finished, RootFace, RootUpdate, Roots};
use super::status::{PaneActivity, ShownStatus};
use super::{Book, ConversationKey, PaneConversation, RootName, Scope, now};
use crate::remote_channel::bridge::{PaneRef, PingWeight};
use crate::remote_channel::transport::slack::threads::RootFailure;
use crate::surface::strings as s;
use daruda_store::remote_threads::RemoteThreadStore;
use std::collections::HashMap;

/// How long a closed pane's status outlives it: past any root of its still
/// being posted.
const CLOSED_KEPT_SECS: i64 = 10 * 60;

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
    /// Each open pane's latest status, for roots made later.
    activity: HashMap<PaneRef, ShownStatus>,
    /// When each pane closed, so a root still being posted for it lands
    /// closed. Kept apart from `activity` and only briefly: a reopened
    /// window can number its panes the same way, and must not read closed.
    closed: HashMap<PaneRef, i64>,
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
            closed: HashMap::new(),
            roots: Roots::default(),
            store,
        };
        if threads.book.prune(now()) {
            threads.save();
        }
        threads
    }

    // --- Root updates ---------------------------------------------------

    /// The root updates to send now; those waiting are not among them.
    pub(super) fn due_updates(&mut self) -> Vec<RootUpdate> {
        self.roots.ready()
    }

    /// Let the waiting updates of the scopes `sendable` names be sent again.
    pub(super) fn resume_updates(&mut self, sendable: impl Fn(&Scope) -> bool) {
        self.roots.resume(sendable);
    }

    /// Record how `update`'s edit went; a root found gone takes its thread
    /// record with it.
    pub(super) fn settle_update(
        &mut self,
        update: &RootUpdate,
        result: &Result<(), RootFailure>,
    ) -> Finished {
        let finished = self.roots.finish(update, result);
        if finished == Finished::Missing {
            self.forget_root(&update.scope, &update.root);
        }
        finished
    }

    /// Keep `update` for the retry tick: its connection cannot send now.
    pub(super) fn hold_update(&mut self, update: &RootUpdate) {
        self.roots.hold(update);
    }

    /// Drop updates for scopes no connection serves any more.
    pub(super) fn keep_live_scopes(&mut self, alive: impl Fn(&Scope) -> bool) {
        self.roots.keep_scopes(alive);
    }

    // --- Placing a post -------------------------------------------------

    /// Decide the post `conversation` of `pane` makes on `scope`'s
    /// connection, weighed by its sender, and bring an existing root up to
    /// date with it.
    pub(super) fn plan_post(
        &mut self,
        scope: Scope,
        pane: PaneRef,
        conversation: PaneConversation,
        weight: PingWeight,
        now: i64,
    ) -> ThreadPlan {
        let thread = self.place(&scope, pane, &conversation, now);
        let shown = self.activity_of(pane);
        let root = thread.as_ref().map(|(root, _)| root.as_str());
        let id = self.want_root_after_place(&scope, pane, root, &conversation);
        let placement = match thread {
            None => Placement::Start,
            // A long-lived thread's root scrolls away, so an outcome — what
            // the sender marked the person must see — surfaces in the
            // conversation too; progress stays in the thread. A post that
            // starts its thread is at the bottom already.
            Some((root, divider)) => Placement::Reply {
                root,
                divider,
                broadcast: weight == PingWeight::Outcome,
            },
        };
        ThreadPlan {
            scope,
            id,
            pane,
            key: conversation.key,
            name: conversation.name,
            shown,
            placement,
        }
    }

    /// Record the root `plan`'s post started, and bring it up to the pane's
    /// status and name if either moved on while it was posting.
    pub(super) fn root_started(&mut self, plan: &ThreadPlan, root: String, now: i64) {
        self.remember(
            &plan.scope,
            plan.id.clone(),
            plan.key.clone(),
            root.clone(),
            now,
        );
        self.root_posted(
            &plan.scope,
            &plan.id,
            &root,
            plan.pane,
            &plan.name,
            plan.shown,
        );
    }

    /// The thread `conversation` of `pane` speaks in, with the divider owed
    /// when the pane carried it over from an earlier conversation. `None`:
    /// the pane has no thread on this connection yet.
    fn place(
        &mut self,
        scope: &Scope,
        pane: PaneRef,
        conversation: &PaneConversation,
        now: i64,
    ) -> Option<(String, Option<String>)> {
        // A pane that posts is open, whatever an earlier pane by its number
        // did.
        self.closed.remove(&pane);
        let previous = self
            .last
            .insert((scope.connection.clone(), pane), conversation.key.clone());
        if let Some(record) = self.book.for_conversation(scope, &conversation.key) {
            let root = record.root.clone();
            if self.book.touch(scope, &root, now) {
                self.save();
            }
            return Some((root, None));
        }
        let previous = previous?;
        let root = self
            .book
            .continue_with(scope, &previous, conversation.key.clone(), now)?;
        self.save();
        Some((root, Some(divider_text(&previous, conversation))))
    }

    /// After placing a post from `pane`: note the root's title on this
    /// connection, and bring an existing root up to date. After a restart this
    /// run does not know what the root shows; one edit settles it, and a root
    /// that already shows this face is not edited again. The thread's id —
    /// its record's, or a new one for the root this post starts.
    fn want_root_after_place(
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
    fn root_posted(
        &mut self,
        scope: &Scope,
        id: &str,
        root: &str,
        pane: PaneRef,
        name: &RootName,
        posted: Option<ShownStatus>,
    ) {
        let posted = posted.map(|shown| RootFace {
            name: name.clone(),
            shown,
        });
        if let Some(face) = &posted {
            self.roots.shown(scope, root, face.clone());
        }
        // The name may have changed while the root was posting, too.
        let current = self.activity_of(pane).map(|shown| RootFace {
            name: self
                .names
                .get(&(scope.connection.clone(), pane))
                .cloned()
                .unwrap_or_else(|| name.clone()),
            shown,
        });
        if let Some(current) = current
            && posted.as_ref() != Some(&current)
        {
            self.roots.want(scope, id, root.to_owned(), current);
        }
    }

    // --- Pane activity ----------------------------------------------------

    pub(crate) fn activity_of(&self, pane: PaneRef) -> Option<ShownStatus> {
        match self.closed.contains_key(&pane) {
            true => Some(ShownStatus::Closed),
            false => self.activity.get(&pane).copied(),
        }
    }

    /// Note `pane`'s activity. The status its roots should now show, or
    /// `None` when nothing changed.
    pub(super) fn note_activity(
        &mut self,
        pane: PaneRef,
        activity: PaneActivity,
        now: i64,
    ) -> Option<ShownStatus> {
        let previous = self.activity_of(pane);
        let shown = ShownStatus::next(previous, activity, now);
        if previous == Some(shown) {
            return None;
        }
        if shown == ShownStatus::Closed {
            self.activity.remove(&pane);
            self.closed.insert(pane, now);
        } else {
            self.closed.remove(&pane);
            self.activity.insert(pane, shown);
        }
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

    /// Give `pane`'s roots its latest name. Every post names them anew; this
    /// carries a subject that arrived since, as it usually does mid-turn, to
    /// roots that would otherwise keep the old one until the next post.
    /// Whether any root's name changed.
    pub(super) fn rename_pane(&mut self, pane: PaneRef, name: &RootName) -> bool {
        let mut renamed = false;
        for (_, held) in self.names.iter_mut().filter(|((_, held), _)| *held == pane) {
            renamed |= *held != *name;
            *held = name.clone();
        }
        if renamed && let Some(shown) = self.activity_of(pane) {
            self.want_for_pane(pane, shown);
        }
        renamed
    }

    /// Forget panes closed long enough ago that no root of theirs can still
    /// be posting.
    pub(super) fn forget_closed(&mut self, now: i64) {
        self.closed.retain(|_, at| now - *at < CLOSED_KEPT_SECS);
    }

    /// The root of the thread `pane` speaks in on `scope`'s connection, as a
    /// post of its would be placed: `current`'s, the conversation the pane is
    /// in now, else that of the one it last posted from, which its next post
    /// continues. Read-only: nothing is touched or saved.
    pub(crate) fn root_for(
        &self,
        scope: &Scope,
        pane: PaneRef,
        current: Option<&ConversationKey>,
    ) -> Option<String> {
        let last = self.last.get(&(scope.connection.clone(), pane));
        let record = current
            .and_then(|key| self.book.for_conversation(scope, key))
            .or_else(|| last.and_then(|key| self.book.for_conversation(scope, key)))?;
        Some(record.root.clone())
    }

    /// Whether `key` has a thread of its own in `scope`.
    pub(super) fn has_thread(&self, scope: &Scope, key: &ConversationKey) -> bool {
        self.book.for_conversation(scope, key).is_some()
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
    fn forget_root(&mut self, scope: &Scope, root: &str) {
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

    /// The face `root` on `connection` is waiting to show, sent or not.
    #[cfg(test)]
    pub(super) fn wanted(&self, connection: &str, root: &str) -> Option<RootFace> {
        self.roots.wanted(connection, root)
    }
}

fn divider_text(previous: &ConversationKey, now: &PaneConversation) -> String {
    if previous.agent == now.key.agent {
        s::remote_channel::thread_new_conversation()
    } else {
        s::remote_channel::thread_switched_agent(&now.agent_name)
    }
}
