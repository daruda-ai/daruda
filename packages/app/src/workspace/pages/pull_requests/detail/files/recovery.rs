//! Reading whole a file whose patch GitHub did not send.

use super::*;

impl FilesState {
    /// Start reading the shown file whole: what to read, at the listing's
    /// commit. `None` unless GitHub sent no patch for it and the commit is
    /// known.
    pub(super) fn begin_recover(&mut self, ticket: ReadTicket) -> Option<RecoverStart> {
        if self.recovering.is_some() {
            return None;
        }
        let shown = self
            .shown
            .as_ref()
            .filter(|shown| shown.diff == FileDiff::TooLarge)?;
        let revision = self.revision.clone()?;
        if shown.revision.as_ref() != Some(&revision)
            || self
                .recovery_failure(shown)
                .is_some_and(|failure| !failure.can_retry())
        {
            return None;
        }
        let file = self.files.iter().find(|file| file.path == shown.path)?;
        self.recovery_failures.remove(&file.path);
        self.recovering = Some(Recovering {
            path: file.path.clone(),
            revision: revision.clone(),
            ticket,
        });
        Some(RecoverStart {
            path: file.path.clone(),
            change: file.change.clone(),
            lines: file.additions + file.deletions,
            merge_base: self
                .merge_base
                .as_ref()
                .filter(|(head, _)| *head == revision)
                .map(|(_, base)| base.clone()),
            head: revision,
        })
    }

    fn recovery_failure(&self, shown: &Shown) -> Option<&RecoverFailure> {
        let (revision, failure) = self.recovery_failures.get(&shown.path)?;
        (shown.revision.as_ref() == Some(revision)).then_some(failure)
    }

    /// Take the whole file's diff if it is the read waited on and the
    /// listing is still at its commit: it becomes the file's patch, as if
    /// GitHub had sent it.
    pub(super) fn land_recovered(
        &mut self,
        ticket: ReadTicket,
        answer: Result<Recovered, RecoverFailure>,
    ) -> bool {
        let Some(recovering) = self
            .recovering
            .as_mut()
            .filter(|held| held.ticket == ticket && self.revision.as_ref() == Some(&held.revision))
        else {
            return false;
        };
        let read = match answer {
            Ok(read) => read,
            Err(failure) => {
                self.recovery_failures.insert(
                    recovering.path.clone(),
                    (recovering.revision.clone(), failure),
                );
                self.recovering = None;
                return true;
            }
        };
        let path = recovering.path.clone();
        self.merge_base = Some((recovering.revision.clone(), read.merge_base));
        self.recovering = None;
        if let Some(file) = self.files.iter_mut().find(|file| file.path == path) {
            file.patch = Some(read.patch);
        }
        if self.shown.as_ref().is_some_and(|shown| shown.path == path) {
            let drawn = self.drawn();
            self.build_shown(&path, drawn);
        }
        true
    }

    pub(super) fn recovery_view(&self, shown: &Shown) -> Option<RecoveryView> {
        if shown.diff != FileDiff::TooLarge
            || self.revision.is_none()
            || shown.revision != self.revision
        {
            return None;
        }
        let failure = self.recovery_failure(shown);
        Some(match (&self.recovering, failure) {
            (Some(held), _) if held.path == shown.path => RecoveryView::Reading,
            (Some(_), failure) if failure.is_none_or(RecoverFailure::can_retry) => {
                RecoveryView::Busy
            }
            (_, Some(failure)) => RecoveryView::Failed(failure.clone()),
            _ => RecoveryView::Offered,
        })
    }
}
