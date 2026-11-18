//! A process-wide store's file: written shortly after a burst of changes
//! settles and at quit, or never when it could not be read. The state both
//! pull request stores keep about their file, with its transitions in one
//! place.

use std::path::PathBuf;
use std::time::Duration;

use gpui::Task;

/// Whether what a store holds outlasts a restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Keeping {
    /// Written to its file.
    #[default]
    Saved,
    /// The file could not be read (damaged, or from a newer daruda) and is
    /// left alone: what is held lasts until quit.
    MemoryOnly,
    /// Writing the file failed; what is held lasts until quit unless a
    /// later write succeeds.
    SaveFailed,
}

/// How long a burst of changes settles before the file is written.
pub(super) const SAVE_AFTER: Duration = Duration::from_millis(400);

#[derive(Default)]
pub(super) struct SavedFile {
    /// Where it is written; `None` keeps what it holds in memory only.
    dir: Option<PathBuf>,
    /// Changed since last written.
    dirty: bool,
    /// The last write failed.
    failed: bool,
    /// The write waiting for the burst to settle.
    pending: Option<Task<()>>,
}

impl SavedFile {
    pub(super) fn at(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            ..Self::default()
        }
    }

    /// Something changed. `true` when a write is to be scheduled now: the
    /// file is kept and none is waiting already.
    pub(super) fn changed(&mut self) -> bool {
        self.dirty = true;
        self.dir.is_some() && self.pending.is_none()
    }

    pub(super) fn scheduled(&mut self, write: Task<()>) {
        self.pending = Some(write);
    }

    /// Where to write, when there is something to write.
    pub(super) fn due(&self) -> Option<PathBuf> {
        self.dir.clone().filter(|_| self.dirty)
    }

    /// The write ended with `result`; the next change schedules another.
    pub(super) fn written<E>(&mut self, result: &Result<(), E>) {
        self.pending = None;
        match result {
            Ok(()) => {
                self.dirty = false;
                self.failed = false;
            }
            Err(_) => self.failed = true,
        }
    }

    /// Whether what it holds outlasts a restart.
    pub(super) fn keeping(&self) -> Keeping {
        match (self.dir.is_some(), self.failed) {
            (false, _) => Keeping::MemoryOnly,
            (true, true) => Keeping::SaveFailed,
            (true, false) => Keeping::Saved,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_file_is_written_once_per_burst_and_says_how_it_went() {
        let mut file = SavedFile::at(Some("/d".into()));
        assert_eq!(file.due(), None, "nothing changed");
        assert!(file.changed(), "the first change schedules a write");
        file.scheduled(Task::ready(()));
        assert!(!file.changed(), "one write per burst");
        assert_eq!(file.due(), Some("/d".into()));
        file.written(&Err(()));
        assert_eq!(file.keeping(), Keeping::SaveFailed);
        assert_eq!(file.due(), Some("/d".into()), "still to write");
        assert!(file.changed(), "a later change schedules again");
        file.written(&Ok::<(), ()>(()));
        assert_eq!((file.due(), file.keeping()), (None, Keeping::Saved));
    }

    #[test]
    fn a_file_not_read_is_never_written() {
        let mut file = SavedFile::at(None);
        assert!(!file.changed());
        assert_eq!((file.due(), file.keeping()), (None, Keeping::MemoryOnly));
    }
}
