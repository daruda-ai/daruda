//! Task end history projected into the ledger, one change at a time.
use std::collections::HashSet;

use super::state::{Membership, Origin, RunWatch};
use crate::runs::result::{self, RunKey};
use daruda_store::tasks::TasksBook;

impl RunWatch {
    /// Retain only the ends `next` adds or changes relative to `previous`.
    /// Commits that leave a task's history alone are no lifecycle change.
    pub(crate) fn sync_task_history(&mut self, previous: Option<&TasksBook>, next: &TasksBook) {
        self.sync_history_from(previous, next, &HashSet::new());
    }

    /// Join a newly opened log: its carried starts enter the scope before its
    /// history is compared with what the process already retained. An end
    /// that history adds for a carried or `restored` key was settled before
    /// this process saw the run, so it is recovered like a judged one.
    pub(crate) fn adopt_log(
        &mut self,
        carried: Vec<RunKey>,
        restored: Vec<RunKey>,
        previous: Option<&TasksBook>,
        next: &TasksBook,
    ) {
        let mut from_before: HashSet<RunKey> = restored.into_iter().collect();
        for key in carried {
            // A restored ledger entry already holds this key's solicitation.
            if self.membership(&key) != Membership::Scoped {
                self.register(key.clone());
            }
            from_before.insert(key);
        }
        self.sync_history_from(previous, next, &from_before);
    }

    fn sync_history_from(
        &mut self,
        previous: Option<&TasksBook>,
        next: &TasksBook,
        from_before: &HashSet<RunKey>,
    ) {
        for record in next.records() {
            let before = previous.and_then(|book| book.get(&record.spec.id));
            for ended in &record.ended_executions {
                if before.is_some_and(|old| old.ended_executions.contains(ended)) {
                    continue;
                }
                if let Some(snapshot) = result::task_snapshot(record, &ended.exec) {
                    let origin = if from_before.contains(&snapshot.key) {
                        Origin::Recovered
                    } else {
                        Origin::Live
                    };
                    self.settle(snapshot, origin);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
