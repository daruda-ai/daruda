//! The thread table: which root message speaks for which pane conversation.
//! GPUI-free; the driver loads it from and saves it to the profile store.

use daruda_store::remote_threads::{ConversationKey, ThreadRecord};

/// Records older than this are forgotten; their threads read as unknown.
pub(crate) const MAX_AGE_SECS: i64 = 30 * 24 * 60 * 60;
/// The newest records kept once older ones are pruned.
pub(crate) const MAX_THREADS: usize = 500;
/// A pane that clears its conversation often keeps only its recent sessions.
pub(crate) const MAX_SESSIONS: usize = 32;
/// `last_used` is rewritten at most this often, so routine use is not a save.
pub(crate) const TOUCH_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// The identity a thread was made under. A record of another bot or chat is
/// never matched: its root is not on this conversation's screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Scope {
    pub connection: String,
    pub bot: String,
    pub conversation: String,
}

impl Scope {
    fn holds(&self, record: &ThreadRecord) -> bool {
        record.connection == self.connection
            && record.bot == self.bot
            && record.conversation == self.conversation
    }
}

#[derive(Default)]
pub(crate) struct Book {
    threads: Vec<ThreadRecord>,
}

impl Book {
    pub(crate) fn from_records(threads: Vec<ThreadRecord>) -> Self {
        Self { threads }
    }

    pub(crate) fn records(&self) -> &[ThreadRecord] {
        &self.threads
    }

    /// The thread that holds `key`, if any.
    pub(crate) fn for_conversation(
        &self,
        scope: &Scope,
        key: &ConversationKey,
    ) -> Option<&ThreadRecord> {
        self.threads
            .iter()
            .find(|record| scope.holds(record) && record.sessions.contains(key))
    }

    /// The thread whose root message is `root`.
    pub(crate) fn for_root(&self, scope: &Scope, root: &str) -> Option<&ThreadRecord> {
        self.threads
            .iter()
            .find(|record| scope.holds(record) && record.root == root)
    }

    /// Record a new thread for `key` rooted at `root`. A conversation belongs to
    /// one thread, so any older claim on `key` in this scope is dropped.
    pub(crate) fn insert(&mut self, scope: &Scope, key: ConversationKey, root: String, now: i64) {
        self.release(scope, &key);
        self.threads.push(ThreadRecord {
            id: uuid::Uuid::new_v4().simple().to_string(),
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
    pub(crate) fn continue_with(
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
    pub(crate) fn touch(&mut self, scope: &Scope, root: &str, now: i64) -> bool {
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

    /// Drop records past [`MAX_AGE_SECS`], then all but the newest
    /// [`MAX_THREADS`]. Whether anything was dropped.
    pub(crate) fn prune(&mut self, now: i64) -> bool {
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
