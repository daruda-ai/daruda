//! Which chat thread on a remote channel speaks for which agent conversation.
//!
//! Profile-scoped: a record names conversations of this profile's panes, so a
//! profile that takes over the same bot must not route into them. Each record
//! carries the bot and conversation it was made under; a reader ignores
//! records of another identity rather than deleting them.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::persistence::{LoadOutcome, load_json_file, save_json_atomic};

const SUBSYSTEM: &str = "remote_threads";
pub const SCHEMA_VERSION: u32 = 1;

/// One agent conversation: the agent that runs it and its session id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationKey {
    pub agent: String,
    pub session: String,
}

/// One pane's thread. `sessions` lists every conversation the pane has held
/// in it, oldest first, so a restored pane finds its thread by the session it
/// resumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRecord {
    pub id: String,
    pub connection: String,
    pub bot: String,
    pub conversation: String,
    pub root: String,
    pub sessions: Vec<ConversationKey>,
    /// Unix seconds.
    pub last_used: i64,
}

#[derive(Deserialize)]
struct File {
    version: u32,
    threads: Vec<ThreadRecord>,
}

#[derive(Serialize)]
struct FileRef<'a> {
    version: u32,
    threads: &'a [ThreadRecord],
}

/// The thread records of the current profile.
#[derive(Debug, Clone)]
pub struct RemoteThreadStore {
    path: PathBuf,
}

impl RemoteThreadStore {
    pub fn current() -> Self {
        Self {
            path: crate::storage::StorageLayout::current().remote_threads(),
        }
    }

    /// A store rooted at `dir`, for tests and isolated runs.
    pub fn at(dir: &Path) -> Self {
        Self {
            path: dir.join(crate::storage::StorageLayout::REMOTE_THREADS_FILE),
        }
    }

    /// Records of a known schema; anything else starts empty. A newer schema
    /// is left on disk untouched until this build next saves.
    pub fn load(&self) -> Vec<ThreadRecord> {
        match load_json_file::<File>(SUBSYSTEM, &self.path) {
            LoadOutcome::Parsed(file) if file.version == SCHEMA_VERSION => file.threads,
            LoadOutcome::Parsed(_) | LoadOutcome::Missing | LoadOutcome::Corrupt => Vec::new(),
        }
    }

    pub fn save(&self, threads: &[ThreadRecord]) -> std::io::Result<()> {
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        save_json_atomic(
            dir,
            &self.path,
            &FileRef {
                version: SCHEMA_VERSION,
                threads,
            },
        )
    }
}
