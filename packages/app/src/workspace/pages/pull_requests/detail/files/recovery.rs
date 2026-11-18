//! Reading whole a file whose patch GitHub did not send.

use super::*;

impl FilesState {
    /// Start reading the shown file whole: what to read, at the listing's
    /// commit. `None` unless GitHub sent no patch for it and the commit is
    /// known.
    pub(super) fn begin_recover(&mut self, ticket: ReadTicket) -> Option<RecoverStart> {
        if self.recovery_busy() {
            return None;
        }
        let shown = self
            .shown
            .as_ref()
            .filter(|shown| shown.diff == FileDiff::TooLarge)?;
        let revision = self.revision.clone()?;
        let file = self.files.iter().find(|file| file.path == shown.path)?;
        self.recovering = Some(Recovering {
            path: file.path.clone(),
            revision: revision.clone(),
            ticket,
            failure: None,
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

    /// A file is being read whole.
    pub(super) fn recovery_busy(&self) -> bool {
        self.recovering
            .as_ref()
            .is_some_and(|held| held.failure.is_none())
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
                recovering.failure = Some(failure);
                return true;
            }
        };
        let path = recovering.path.clone();
        self.merge_base = Some((recovering.revision.clone(), read.merge_base));
        self.recovering = None;
        if let Some(file) = self.files.iter_mut().find(|file| file.path == path) {
            file.patch = Some(read.patch);
        }
        if let Some(limit) = self
            .shown
            .as_ref()
            .filter(|shown| shown.path == path)
            .map(|shown| shown.limit)
        {
            self.build_shown(&path, limit);
        }
        true
    }

    pub(super) fn recovery_view(&self, shown: &Shown) -> Option<RecoveryView> {
        if shown.diff != FileDiff::TooLarge || self.revision.is_none() {
            return None;
        }
        Some(
            match self
                .recovering
                .as_ref()
                .filter(|held| held.path == shown.path)
            {
                None if self.recovery_busy() => RecoveryView::Busy,
                None => RecoveryView::Offered,
                Some(Recovering {
                    failure: Some(failure),
                    ..
                }) => RecoveryView::Failed(failure.clone()),
                Some(_) => RecoveryView::Reading,
            },
        )
    }
}
