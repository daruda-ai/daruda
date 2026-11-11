//! What each connection's page should show next, what it shows, and whether
//! a publish is under way. GPUI-free; `driver` drives it. Like the thread
//! roots' queue: the latest board wins, and a board the page already shows
//! is not sent again.

use super::board::Board;
use crate::remote_channel::threads::Scope;
use crate::remote_channel::transport::slack::home::PublishVerdict;
use std::collections::{HashMap, HashSet};

/// Whether the person has a page daruda should keep up to date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Viewer {
    /// Not opened since daruda started: nothing is published unasked.
    NotSeen,
    Viewing,
    /// The channel turned the page off; nothing is published until the
    /// person opens it again.
    Unavailable,
}

/// Where a page's next publish stands, when one is owed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    InFlight,
    /// Failed for a passing reason, or its connection could not send; sent
    /// again on the retry tick or the person's next opening.
    Waiting,
}

/// A board on its way to the page of `scope`'s connection.
#[derive(Clone, Debug)]
pub(super) struct Publication {
    pub scope: Scope,
    pub board: Board,
}

/// One connection's page, under the identity that serves it.
struct Page {
    scope: Scope,
    viewer: Viewer,
    wanted: Option<Board>,
    /// What the page shows now, as far as this run knows.
    published: Option<Board>,
    phase: Option<Phase>,
}

impl Page {
    fn new(scope: Scope) -> Self {
        Self {
            scope,
            viewer: Viewer::NotSeen,
            wanted: None,
            published: None,
            phase: None,
        }
    }
}

/// What a finished publish leaves to do.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Settled {
    /// Shown; send whatever changed meanwhile.
    Published,
    /// The page is off. `first` the first time this connection said so.
    Unavailable { first: bool },
    /// Kept for a later trigger.
    Retry,
    /// Refused; this board is not sent again.
    Dropped,
    /// For a page another identity has replaced since.
    Stale,
}

#[derive(Default)]
pub(super) struct PublishQueue {
    pages: HashMap<String, Page>,
    /// Connections whose page was reported off, so it is reported once.
    told_unavailable: HashSet<String>,
}

impl PublishQueue {
    /// The person opened the page of `scope`'s connection: keep it up to
    /// date, and let a publish waiting on a passing failure go again.
    pub(super) fn opened(&mut self, scope: &Scope) {
        let page = self.page(scope);
        page.viewer = Viewer::Viewing;
        if page.phase == Some(Phase::Waiting) {
            page.phase = None;
        }
    }

    /// Ask the page of `scope`'s connection to show `board`, if the person
    /// is viewing it.
    pub(super) fn want(&mut self, scope: &Scope, board: Board) {
        if let Some(page) = self.pages.get_mut(&scope.connection)
            && page.scope == *scope
            && page.viewer == Viewer::Viewing
        {
            page.wanted = Some(board);
        }
    }

    /// The connections' pages the person is viewing, under the identity
    /// each was opened with.
    pub(super) fn viewing(&self) -> impl Iterator<Item = &Scope> {
        self.pages
            .values()
            .filter(|page| page.viewer == Viewer::Viewing)
            .map(|page| &page.scope)
    }

    /// The boards to publish now: wanted, not already shown, not in flight,
    /// and not waiting unless `retrying`.
    pub(super) fn ready(&mut self, retrying: bool) -> Vec<Publication> {
        let mut ready = Vec::new();
        for page in self.pages.values_mut() {
            if retrying && page.phase == Some(Phase::Waiting) {
                page.phase = None;
            }
            if page.wanted.is_some() && page.wanted == page.published {
                page.wanted = None;
            }
            if page.viewer != Viewer::Viewing || page.phase.is_some() {
                continue;
            }
            if let Some(board) = &page.wanted {
                page.phase = Some(Phase::InFlight);
                ready.push(Publication {
                    scope: page.scope.clone(),
                    board: board.clone(),
                });
            }
        }
        ready
    }

    /// Record how `publication` went.
    pub(super) fn finish(
        &mut self,
        publication: &Publication,
        verdict: &PublishVerdict,
    ) -> Settled {
        let Some(page) = current(&mut self.pages, &publication.scope) else {
            return Settled::Stale;
        };
        page.phase = None;
        match verdict {
            PublishVerdict::Published => {
                page.published = Some(publication.board.clone());
                Settled::Published
            }
            PublishVerdict::Disabled => {
                page.viewer = Viewer::Unavailable;
                page.wanted = None;
                Settled::Unavailable {
                    first: self
                        .told_unavailable
                        .insert(publication.scope.connection.clone()),
                }
            }
            PublishVerdict::Transient(_) => {
                page.phase = Some(Phase::Waiting);
                Settled::Retry
            }
            PublishVerdict::Refused(_) => {
                if page.wanted.as_ref() == Some(&publication.board) {
                    page.wanted = None;
                }
                Settled::Dropped
            }
        }
    }

    /// Keep `publication` waiting: its connection cannot send right now.
    /// Expected while it reconnects, so not a failure.
    pub(super) fn hold(&mut self, publication: &Publication) {
        if let Some(page) = current(&mut self.pages, &publication.scope) {
            page.phase = Some(Phase::Waiting);
        }
    }

    /// Forget the pages of scopes no connection can serve any more.
    pub(super) fn keep_scopes(&mut self, alive: impl Fn(&Scope) -> bool) {
        self.pages.retain(|_, page| alive(&page.scope));
    }

    /// The page `scope`'s connection has under `scope`; another identity's
    /// page is forgotten for a new one.
    fn page(&mut self, scope: &Scope) -> &mut Page {
        let page = self
            .pages
            .entry(scope.connection.clone())
            .or_insert_with(|| Page::new(scope.clone()));
        if page.scope != *scope {
            *page = Page::new(scope.clone());
        }
        page
    }

    #[cfg(test)]
    pub(super) fn viewer(&self, scope: &Scope) -> Option<Viewer> {
        self.pages
            .get(&scope.connection)
            .filter(|page| page.scope == *scope)
            .map(|page| page.viewer)
    }
}

/// The page that `scope` still serves, if no other identity replaced it.
fn current<'a>(pages: &'a mut HashMap<String, Page>, scope: &Scope) -> Option<&'a mut Page> {
    pages
        .get_mut(&scope.connection)
        .filter(|page| page.scope == *scope)
}
