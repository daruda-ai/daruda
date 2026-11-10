//! The thread table: which root message speaks for which pane conversation.
//! GPUI-free; `state` loads it from and saves it to the profile store.

use daruda_store::remote_threads::{ConversationKey, ThreadRecord};

/// Records older than this are forgotten; their threads read as unknown.
pub(super) const MAX_AGE_SECS: i64 = 30 * 24 * 60 * 60;
/// The newest records kept once older ones are pruned.
pub(super) const MAX_THREADS: usize = 500;
/// A pane that clears its conversation often keeps only its recent sessions.
pub(super) const MAX_SESSIONS: usize = 32;
/// `last_used` is rewritten at most this often, so routine use is not a save.
pub(super) const TOUCH_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// The identity a thread was made under. A record of another bot or chat is
/// never matched: its root is not on this conversation's screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Scope {
    pub connection: String,
    pub bot: String,
    pub conversation: String,
}

impl Scope {
    /// The identity threads on connection `id` are made under now.
    pub(crate) fn serving(
        id: &str,
        bot: &crate::remote_channel::runtime::BotSession,
        recipient: &daruda_config::remote::RemoteRecipient,
    ) -> Self {
        Self {
            connection: id.to_owned(),
            bot: crate::remote_channel::lock::digest_of(&bot.credentials.bot),
            conversation: recipient.conversation_id.clone(),
        }
    }

    /// The identity `record` was made under.
    pub(super) fn of(record: &ThreadRecord) -> Self {
        Self {
            connection: record.connection.clone(),
            bot: record.bot.clone(),
            conversation: record.conversation.clone(),
        }
    }

    fn holds(&self, record: &ThreadRecord) -> bool {
        record.connection == self.connection
            && record.bot == self.bot
            && record.conversation == self.conversation
    }
}

#[derive(Default)]
pub(super) struct Book {
    threads: Vec<ThreadRecord>,
}

impl Book {
    pub(super) fn from_records(threads: Vec<ThreadRecord>) -> Self {
        Self { threads }
    }

    pub(super) fn records(&self) -> &[ThreadRecord] {
        &self.threads
    }

    /// The thread that holds `key`, if any.
    pub(super) fn for_conversation(
        &self,
        scope: &Scope,
        key: &ConversationKey,
    ) -> Option<&ThreadRecord> {
        self.threads
            .iter()
            .find(|record| scope.holds(record) && record.sessions.contains(key))
    }

    /// The newest thread on `connection` holding `key`, under whichever
    /// identity made it: the roots a pane's status reaches.
    pub(super) fn for_connection(
        &self,
        connection: &str,
        key: &ConversationKey,
    ) -> Option<&ThreadRecord> {
        self.threads
            .iter()
            .filter(|record| record.connection == connection && record.sessions.contains(key))
            .max_by_key(|record| record.last_used)
    }

    /// The thread named `id`, as its stop button names it.
    pub(super) fn by_id(&self, scope: &Scope, id: &str) -> Option<&ThreadRecord> {
        self.threads
            .iter()
            .find(|record| scope.holds(record) && record.id == id)
    }

    /// The thread whose root message is `root`.
    pub(super) fn for_root(&self, scope: &Scope, root: &str) -> Option<&ThreadRecord> {
        self.threads
            .iter()
            .find(|record| scope.holds(record) && record.root == root)
    }

    /// Record a new thread for `key` rooted at `root`. A conversation belongs to
    /// one thread, so any older claim on `key` in this scope is dropped.
    pub(super) fn insert(
        &mut self,
        scope: &Scope,
        id: String,
        key: ConversationKey,
        root: String,
        now: i64,
    ) {
        self.release(scope, &key);
        self.threads.push(ThreadRecord {
            id,
            connection: scope.connection.clone(),
            bot: scope.bot.clone(),
            conversation: scope.conversation.clone(),
            root,
            sessions: vec![key],
            last_used: now,
        });
    }

    /// Carry `from`'s thread over to the pane's next conversation `to`.
    /// Returns the root that now holds `to`, or `None` when `from` had none.
    pub(super) fn continue_with(
        &mut self,
        scope: &Scope,
        from: &ConversationKey,
        to: ConversationKey,
        now: i64,
    ) -> Option<String> {
        if from == &to || self.for_conversation(scope, from).is_none() {
            return None;
        }
        // `from`'s record keeps `from`, so releasing `to` cannot empty it.
        self.release(scope, &to);
        let record = self
            .threads
            .iter_mut()
            .find(|record| scope.holds(record) && record.sessions.contains(from))?;
        record.sessions.push(to);
        let excess = record.sessions.len().saturating_sub(MAX_SESSIONS);
        record.sessions.drain(..excess);
        record.last_used = now;
        Some(record.root.clone())
    }

    /// Note a use of `root`. Whether the change is worth saving.
    pub(super) fn touch(&mut self, scope: &Scope, root: &str, now: i64) -> bool {
        match self
            .threads
            .iter_mut()
            .find(|record| scope.holds(record) && record.root == root)
        {
            Some(record) if now - record.last_used >= TOUCH_INTERVAL_SECS => {
                record.last_used = now;
                true
            }
            Some(_) | None => false,
        }
    }

    /// Forget the thread rooted at `root`: its message is gone.
    pub(super) fn forget_root(&mut self, scope: &Scope, root: &str) -> bool {
        let before = self.threads.len();
        self.threads
            .retain(|record| !(scope.holds(record) && record.root == root));
        self.threads.len() != before
    }

    /// Drop records past [`MAX_AGE_SECS`], then all but the newest
    /// [`MAX_THREADS`]. Whether anything was dropped.
    pub(super) fn prune(&mut self, now: i64) -> bool {
        let before = self.threads.len();
        self.threads
            .retain(|record| now - record.last_used <= MAX_AGE_SECS);
        if self.threads.len() > MAX_THREADS {
            self.threads
                .sort_by_key(|record| std::cmp::Reverse(record.last_used));
            self.threads.truncate(MAX_THREADS);
        }
        self.threads.len() != before
    }

    /// Remove `key` from every thread in `scope`; a thread left with no
    /// conversation is dropped.
    fn release(&mut self, scope: &Scope, key: &ConversationKey) {
        for record in self.threads.iter_mut().filter(|record| scope.holds(record)) {
            record.sessions.retain(|held| held != key);
        }
        self.threads
            .retain(|record| !(scope.holds(record) && record.sessions.is_empty()));
    }
}
