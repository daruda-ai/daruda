//! Bounded settlement work retained until an identity can edit its messages.

use std::collections::VecDeque;
use std::time::Duration;
use uuid::Uuid;

pub(crate) const RETRY_INTERVAL: Duration = Duration::from_secs(15);
const PENDING_EDITS_CAP: usize = 64;

pub(crate) struct EditQueue<Receipt> {
    entries: VecDeque<Entry<Receipt>>,
}

struct Entry<Receipt> {
    id: Uuid,
    receipt: Receipt,
    label: String,
    state: State,
}

enum State {
    Pending,
    Editing,
}

impl<Receipt> Default for EditQueue<Receipt> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }
}

impl<Receipt: Clone> EditQueue<Receipt> {
    pub(crate) fn push(&mut self, receipt: Receipt, label: String) {
        self.entries.push_back(Entry {
            id: Uuid::new_v4(),
            receipt,
            label,
            state: State::Pending,
        });
        if self.entries.len() > PENDING_EDITS_CAP {
            self.entries.pop_front();
        }
    }

    pub(crate) fn claim(&mut self) -> Vec<(Uuid, Receipt, String)> {
        self.entries
            .iter_mut()
            .filter_map(|entry| {
                if matches!(entry.state, State::Editing) {
                    return None;
                }
                entry.state = State::Editing;
                Some((entry.id, entry.receipt.clone(), entry.label.clone()))
            })
            .collect()
    }

    /// Every edit still owed, in flight or not, leaving the queue empty so a
    /// completion that arrives later finds nothing to finish.
    pub(super) fn take_all(&mut self) -> Vec<(Receipt, String)> {
        self.entries
            .drain(..)
            .map(|entry| (entry.receipt, entry.label))
            .collect()
    }

    pub(crate) fn contains(&self, id: Uuid) -> bool {
        self.entries.iter().any(|entry| entry.id == id)
    }

    pub(crate) fn finish(&mut self, id: Uuid, outcome: EditOutcome) {
        match outcome {
            EditOutcome::Edited | EditOutcome::Gone => {
                self.entries.retain(|entry| entry.id != id);
            }
            EditOutcome::Transient => {
                if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
                    entry.state = State::Pending;
                }
            }
        }
    }
}

/// Why a platform refused to edit a sent card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditFailure {
    /// The message, its conversation or the bot's access to it no longer
    /// exists, or the request itself is malformed: retrying cannot succeed.
    Gone(String),
    /// Rate limits, server errors, lost connections and unreadable replies,
    /// which a later attempt may get past.
    Transient(String),
}

impl std::fmt::Display for EditFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gone(reason) | Self::Transient(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for EditFailure {}

/// How one claimed edit ended, deciding whether the queue keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditOutcome {
    Edited,
    Gone,
    /// Includes an editor that is not serving right now: the identity may
    /// come back and still own the message.
    Transient,
}

impl EditOutcome {
    pub(crate) fn of(result: &Result<(), EditFailure>) -> Self {
        match result {
            Ok(()) => Self::Edited,
            Err(EditFailure::Gone(_)) => Self::Gone,
            Err(EditFailure::Transient(_)) => Self::Transient,
        }
    }

    /// The outcome of `result`, logging a failure under `{dedup}` or, for a
    /// message that can never be edited, `{dedup}.gone`.
    #[track_caller]
    pub(crate) fn logged(result: Result<(), EditFailure>, message: &str, dedup: &str) -> Self {
        let outcome = Self::of(&result);
        match &result {
            Ok(()) => {}
            Err(error @ EditFailure::Gone(_)) => crate::remote_channel::log_error(
                &format!("{message}; it will not be retried"),
                error,
                &format!("{dedup}.gone"),
            ),
            Err(error @ EditFailure::Transient(_)) => {
                crate::remote_channel::log_error(message, error, dedup)
            }
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_work_is_retried_and_success_releases_it() {
        let mut queue = EditQueue::default();
        queue.push("message", "Approved".into());
        let (id, receipt, label) = queue.claim().pop().unwrap();
        assert_eq!((receipt, label.as_str()), ("message", "Approved"));
        assert!(queue.claim().is_empty());
        queue.finish(id, EditOutcome::Transient);
        assert_eq!(queue.claim().len(), 1);
        queue.finish(id, EditOutcome::Edited);
        assert!(queue.claim().is_empty());
        assert!(!queue.contains(id));
    }

    #[test]
    fn gone_work_is_dropped_and_never_claimed_again() {
        let mut queue = EditQueue::default();
        queue.push("message", "Approved".into());
        let id = queue.claim()[0].0;
        queue.finish(id, EditOutcome::Gone);
        assert!(!queue.contains(id));
        assert!(queue.claim().is_empty());
    }

    #[test]
    fn an_edit_result_maps_to_its_queue_outcome() {
        assert_eq!(EditOutcome::of(&Ok(())), EditOutcome::Edited);
        assert_eq!(
            EditOutcome::of(&Err(EditFailure::Gone("message to edit not found".into()))),
            EditOutcome::Gone
        );
        assert_eq!(
            EditOutcome::of(&Err(EditFailure::Transient("HTTP 502".into()))),
            EditOutcome::Transient
        );
        assert_eq!(
            EditFailure::Gone("chat not found".into()).to_string(),
            "chat not found"
        );
    }

    #[test]
    fn old_completions_cannot_remove_replacement_work_and_storage_is_bounded() {
        let mut queue = EditQueue::default();
        queue.push(0, "Old".into());
        let old = queue.claim()[0].0;
        for message in 1..=PENDING_EDITS_CAP {
            queue.push(message, "New".into());
        }
        queue.finish(old, EditOutcome::Edited);
        assert!(!queue.contains(old));
        assert_eq!(queue.claim().len(), PENDING_EDITS_CAP);
    }
}
