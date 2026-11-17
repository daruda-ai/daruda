//! What a person has done reviewing pull requests' files while the window is
//! open: the files marked viewed, each at the content it was viewed at, and
//! where they stood when a detail closed, to come back to. GPUI-free.

use std::collections::{HashMap, VecDeque};

use super::PrTab;
use crate::workspace::pages::pull_requests::list::PrKey;

/// Pull requests whose progress is kept; the least recently touched goes.
pub(in crate::workspace) const KEPT: usize = 64;

/// Whether a listed file was marked viewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum Viewed {
    No,
    Yes,
    /// Marked, but its content changed since: the mark no longer holds.
    Changed,
}

/// The file shown when a detail closed, at the content it showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct PlaceFile {
    pub path: String,
    pub blob: Option<String>,
}

/// Where a detail stood when it closed.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::workspace) struct Place {
    pub tab: PrTab,
    pub file: Option<PlaceFile>,
    /// The detail's scroll offset, as GPUI keeps it: x and y, at most 0.
    pub offset: (f32, f32),
}

#[derive(Debug, Default)]
struct Entry {
    /// Path to the blob it was viewed at.
    viewed: HashMap<String, String>,
    place: Option<Place>,
}

/// One pull request's viewed marks.
pub(in crate::workspace) struct Marks<'a>(Option<&'a HashMap<String, String>>);

impl Marks<'_> {
    pub(in crate::workspace) fn viewed(&self, path: &str, blob: Option<&str>) -> Viewed {
        match self.0.and_then(|viewed| viewed.get(path)) {
            None => Viewed::No,
            Some(held) if Some(held.as_str()) == blob => Viewed::Yes,
            Some(_) => Viewed::Changed,
        }
    }
}

/// Every kept pull request's progress, the most recently touched last.
#[derive(Debug, Default)]
pub(in crate::workspace) struct ReviewProgress {
    entries: VecDeque<(PrKey, Entry)>,
}

impl ReviewProgress {
    pub(in crate::workspace) const fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    fn find(&self, key: &PrKey) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|(held, _)| held == key)
            .map(|(_, entry)| entry)
    }

    /// `key`'s entry, made the most recently touched; made if missing,
    /// letting the least recently touched go past [`KEPT`].
    fn touch(&mut self, key: &PrKey) -> &mut Entry {
        let entry = match self.entries.iter().position(|(held, _)| held == key) {
            Some(at) => self.entries.remove(at).map(|(_, entry)| entry),
            None => None,
        };
        self.entries
            .push_back((key.clone(), entry.unwrap_or_default()));
        while self.entries.len() > KEPT {
            self.entries.pop_front();
        }
        &mut self.entries.back_mut().expect("just pushed").1
    }

    /// Whether `path`, now at `blob`, was marked viewed.
    #[cfg(test)]
    pub(in crate::workspace) fn viewed(
        &self,
        key: &PrKey,
        path: &str,
        blob: Option<&str>,
    ) -> Viewed {
        self.marks(key).viewed(path, blob)
    }

    /// `key`'s marks, found once for a whole listing.
    pub(in crate::workspace) fn marks(&self, key: &PrKey) -> Marks<'_> {
        Marks(self.find(key).map(|entry| &entry.viewed))
    }

    /// Mark `path` viewed at `blob`, or clear a mark that holds. A file
    /// whose content GitHub does not identify cannot be marked.
    pub(in crate::workspace) fn toggle_viewed(
        &mut self,
        key: &PrKey,
        path: &str,
        blob: Option<&str>,
    ) {
        let Some(blob) = blob else {
            return;
        };
        let viewed = &mut self.touch(key).viewed;
        if viewed.get(path).is_some_and(|held| held == blob) {
            viewed.remove(path);
        } else {
            viewed.insert(path.to_owned(), blob.to_owned());
        }
    }

    /// The detail of `key` closed at `place`.
    pub(in crate::workspace) fn leave(&mut self, key: &PrKey, place: Place) {
        self.touch(key).place = Some(place);
    }

    /// Where the detail of `key` last closed.
    pub(in crate::workspace) fn place(&self, key: &PrKey) -> Option<&Place> {
        self.find(key).and_then(|entry| entry.place.as_ref())
    }
}

#[cfg(test)]
mod tests;
