//! Coming back to the file and scroll offset a detail was left at.

use super::*;

impl FilesState {
    /// The shown file and its content, to come back to.
    pub(in super::super) fn place_file(&self) -> Option<PlaceFile> {
        let path = &self.shown.as_ref()?.path;
        let file = self.files.iter().find(|file| &file.path == path)?;
        Some(PlaceFile {
            path: path.clone(),
            blob: file.blob.clone(),
        })
    }

    /// Come back to `file`, scrolled to `offset` if its content is the
    /// same: chosen now, shown once a page lists it.
    /// Whether the detail is coming back to `path`, for a test.
    #[cfg(test)]
    pub(in super::super) fn restoring_to(&self, path: &str) -> bool {
        self.restore
            .as_ref()
            .is_some_and(|restore| restore.path == path)
    }

    pub(in super::super) fn restore_to(&mut self, file: &PlaceFile, offset: (f32, f32)) {
        self.chosen = Some(file.path.clone());
        self.restore = Some(Restore {
            path: file.path.clone(),
            blob: file.blob.clone(),
            offset,
        });
    }

    /// Once the file come back to is shown, where it is and the offset to
    /// scroll to; given once. Dropped when reading ends without it.
    pub(in super::super) fn take_restored(&mut self) -> Option<Restored> {
        let path = &self.restore.as_ref()?.path;
        let at = self
            .shown
            .as_ref()
            .filter(|shown| &shown.path == path)
            .and_then(|shown| self.files.iter().position(|file| file.path == shown.path));
        let Some(index) = at else {
            if !matches!(self.paging, Paging::Reading { .. }) {
                self.restore = None;
            }
            return None;
        };
        let restore = self.restore.take()?;
        let same = restore.blob.is_some() && self.files[index].blob == restore.blob;
        Some(Restored {
            index,
            offset: same.then_some(restore.offset),
        })
    }

    #[cfg(test)]
    pub(in crate::workspace) fn coming_back(&self) -> bool {
        self.restore.is_some()
    }

    /// The place a reopened detail is still coming back to.
    pub(in super::super) fn pending_place(&self) -> Option<(PlaceFile, (f32, f32))> {
        let restore = self.restore.as_ref()?;
        Some((
            PlaceFile {
                path: restore.path.clone(),
                blob: restore.blob.clone(),
            },
            restore.offset,
        ))
    }
}
