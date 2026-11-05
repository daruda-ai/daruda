//! Phone cards left live when the app quits.
//!
//! The quit handler blocks the foreground, so edits snapshot everything they
//! need first and run on their own threads; the handler then waits on them
//! with the quit deadline instead of the async pipeline settlement uses.

use std::sync::mpsc;
use std::time::Instant;

use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use gpui::App;

use super::{CardLedger, EditFailure, EditOutcome, OwnedCards, SendState};

impl<Receipt> CardLedger<Receipt> {
    /// Take every delivered, unsettled card's receipt and close the card.
    fn close_sent(&mut self) -> Vec<Receipt> {
        let mut sent = Vec::new();
        for entry in self.cards.values_mut() {
            match std::mem::replace(&mut entry.state, SendState::ClosedAtQuit) {
                SendState::Sent(receipt) => sent.push(receipt),
                state => entry.state = state,
            }
        }
        sent
    }
}

impl<Owner, Receipt: Clone, Message> OwnedCards<Owner, Receipt, Message> {
    /// The edits owed at quit: settlements still queued keep their result,
    /// and every delivered, unanswered card reads `cancelled`. Cards with no
    /// receipt cannot be edited and are left for tap-time cleanup.
    pub(crate) fn close_for_quit(&mut self, cancelled: &str) -> Vec<(Receipt, String)> {
        let mut edits = self.edits.take_all();
        edits.extend(
            self.ledger
                .close_sent()
                .into_iter()
                .map(|receipt| (receipt, cancelled.to_owned())),
        );
        edits
    }
}

type Edit = Box<dyn FnOnce() -> Result<(), EditFailure> + Send>;

/// One card's edit, self-contained so it can run without the foreground.
pub(crate) struct QuitEdit {
    edit: Edit,
    failed: &'static str,
    dedup: &'static str,
}

impl QuitEdit {
    /// `failed` and `dedup` name the log entry for a refused edit.
    pub(crate) fn new(
        failed: &'static str,
        dedup: &'static str,
        edit: impl FnOnce() -> Result<(), EditFailure> + Send + 'static,
    ) -> Self {
        Self {
            edit: Box::new(edit),
            failed,
            dedup,
        }
    }
}

/// Edits running on their own threads, reporting each one that finishes.
pub(crate) struct Settling {
    running: usize,
    finished: mpsc::Receiver<()>,
}

impl Settling {
    pub(crate) fn start(edits: Vec<QuitEdit>) -> Self {
        let (sender, finished) = mpsc::channel();
        let mut running = 0;
        for edit in edits {
            let sender = sender.clone();
            let spawned = std::thread::Builder::new()
                .name("card-quit-edit".into())
                .spawn(move || {
                    EditOutcome::logged((edit.edit)(), edit.failed, edit.dedup);
                    // SILENT-OK: the quit handler stopped waiting at its deadline.
                    let _ = sender.send(());
                });
            match spawned {
                Ok(_) => running += 1,
                Err(error) => crate::remote_channel::log_error(
                    "Phone card edit could not start at quit",
                    &error,
                    "cards.quit_spawn",
                ),
            }
        }
        Self { running, finished }
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.running == 0
    }

    /// Wait until every edit finished or `until` passed, returning how many
    /// were still running. Those are left for tap-time cleanup.
    pub(crate) fn wait(self, until: Instant) -> usize {
        let mut left = self.running;
        while left > 0 {
            let remaining = until.saturating_duration_since(Instant::now());
            match self.finished.recv_timeout(remaining) {
                Ok(()) => left -= 1,
                Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
        }
        if left > 0 {
            LogWriter::log(
                ErrorReport::new("Phone cards were still being cancelled at quit")
                    .severity(ErrorSeverity::Warning)
                    .at(file!(), line!())
                    .with_context("left", left.to_string())
                    .dedup("cards.quit_settle")
                    .build(),
            );
        }
        left
    }
}

/// Start editing every card the app can still edit on Telegram, Slack and
/// Discord: queued settlements with their result, unanswered cards as
/// cancelled by the quit.
pub(crate) fn begin(cx: &mut App) -> Settling {
    let cancelled = crate::surface::strings::notification::card_cancelled_at_quit();
    let mut edits = crate::telegram::global::TelegramBridge::quit_edits(&cancelled, cx);
    edits.extend(crate::remote_channel::global::RemoteChannels::quit_edits(
        &cancelled, cx,
    ));
    Settling::start(edits)
}

#[cfg(test)]
mod tests;
