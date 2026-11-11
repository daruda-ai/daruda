//! What each connection's page should show next, what it shows, and whether
//! a publish is under way: `page` keeps one page's state and `links` the
//! thread links it has learned, while the queue keeps a page per connection
//! under the identity that serves it. GPUI-free; `driver` drives it. Like
//! the thread roots' queue: the latest board wins, and a board the page
//! already shows is not sent again.

mod links;
mod page;

pub(super) use links::Learned;
pub(super) use page::PageEpoch;
pub(crate) use page::Viewer;

use super::board::{Board, Outcome};
use super::token::Tap;
use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::threads::Scope;
use crate::remote_channel::transport::{LinkFailure, PageVerdict};
use page::Page;
use std::collections::{HashMap, HashSet};

/// The links a page asked for at once, all answered for that page.
#[derive(Debug)]
pub(super) struct LinkAsk {
    pub epoch: PageEpoch,
    pub roots: Vec<String>,
}

/// A board on its way to the page of `scope`'s connection.
#[derive(Clone, Debug)]
pub(super) struct Publication {
    pub scope: Scope,
    pub board: Board,
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
    /// Refused; this board is not sent again, but a newer one wanted
    /// meanwhile is sent at once.
    Dropped,
    /// For a page another identity has replaced since.
    Stale,
}

#[derive(Default)]
pub(super) struct PublishQueue {
    pages: HashMap<String, Page>,
    /// Connections whose page was reported off, so it is reported once.
    told_unavailable: HashSet<String>,
    /// The epoch the next page made takes.
    next_epoch: u64,
}

impl PublishQueue {
    /// The person opened the page of `scope`'s connection: keep it up to
    /// date, and let what waits on a passing failure go again once the page
    /// is built.
    pub(super) fn opened(&mut self, scope: &Scope) {
        let page = self.page(scope);
        page.viewer = Viewer::Viewing;
        page.resume();
    }

    /// The retry tick: every page lets what waits on a passing failure go
    /// again once built. Whether any page has something to build for.
    pub(super) fn retry(&mut self) -> bool {
        self.pages
            .values_mut()
            .fold(false, |due, page| page.resume() | due)
    }

    /// The person tapped a button on the page of `scope`'s connection: it is
    /// in view, and shows what the tap came to from now on, if anything.
    pub(super) fn tapped(&mut self, scope: &Scope, outcome: Option<Outcome>) {
        self.opened(scope);
        if let Some(outcome) = outcome {
            self.page(scope).outcome = Some(outcome);
        }
    }

    /// What the last tap on `scope`'s page came to.
    pub(super) fn outcome(&self, scope: &Scope) -> Option<&Outcome> {
        current_ref(&self.pages, scope)?.outcome.as_ref()
    }

    /// The pane `tap` names on `scope`'s page: on the board it shows, on the
    /// one in flight, or on the one wanted next.
    pub(super) fn resolve(&self, scope: &Scope, tap: Tap) -> Option<PaneRef> {
        current_ref(&self.pages, scope)?.resolve(tap)
    }

    /// Ask the page of `scope`'s connection to show `board`, if the person
    /// is viewing it.
    pub(super) fn want(&mut self, scope: &Scope, board: Board) {
        if let Some(page) = current(&mut self.pages, scope) {
            page.want(board);
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

    /// The boards to publish now, one per page at most.
    pub(super) fn ready(&mut self) -> Vec<Publication> {
        self.pages
            .values_mut()
            .filter_map(|page| {
                Some(Publication {
                    board: page.ready()?,
                    scope: page.scope.clone(),
                })
            })
            .collect()
    }

    /// Record how `publication` went.
    pub(super) fn finish(&mut self, publication: &Publication, verdict: &PageVerdict) -> Settled {
        let Some(page) = current(&mut self.pages, &publication.scope) else {
            return Settled::Stale;
        };
        page.finish(&publication.board, verdict);
        match verdict {
            PageVerdict::Published => Settled::Published,
            PageVerdict::Off => Settled::Unavailable {
                first: self
                    .told_unavailable
                    .insert(publication.scope.connection.clone()),
            },
            PageVerdict::Transient(_) => Settled::Retry,
            PageVerdict::Refused(_) => Settled::Dropped,
        }
    }

    /// Keep `publication` waiting: its connection cannot send right now.
    /// Expected while it reconnects, so not a failure.
    pub(super) fn hold(&mut self, publication: &Publication) {
        if let Some(page) = current(&mut self.pages, &publication.scope) {
            page.hold();
        }
    }

    /// Give `board` the links `scope`'s page knows for its threads.
    pub(super) fn link(&self, scope: &Scope, board: &mut Board) {
        if let Some(page) = current_ref(&self.pages, scope) {
            page.links.fill(board);
        }
    }

    /// The roots among `roots` whose link `scope`'s page has not asked for
    /// yet, each marked as being asked for now. `None` without a page.
    pub(super) fn ask_links<'a>(
        &mut self,
        scope: &Scope,
        roots: impl IntoIterator<Item = &'a str>,
    ) -> Option<LinkAsk> {
        let page = current(&mut self.pages, scope)?;
        Some(LinkAsk {
            epoch: page.epoch,
            roots: page.links.ask(roots),
        })
    }

    /// The link `scope`'s page of `epoch` asked for `root` came back as
    /// `link`, or failed. `None` once that page has been replaced or
    /// forgotten: its asks were its own, and a page made since asks anew, so
    /// no answer settles another page's ask.
    pub(super) fn learn_link(
        &mut self,
        scope: &Scope,
        epoch: PageEpoch,
        root: &str,
        link: &Result<String, LinkFailure>,
    ) -> Option<Learned> {
        let page = current(&mut self.pages, scope).filter(|page| page.epoch == epoch)?;
        Some(page.links.learn(root, link))
    }

    /// Forget the pages of scopes no connection can serve any more.
    pub(super) fn keep_scopes(&mut self, alive: impl Fn(&Scope) -> bool) {
        self.pages.retain(|_, page| alive(&page.scope));
    }

    /// The page `scope`'s connection has under `scope`; another identity's
    /// page is forgotten for a new one, which takes a new epoch.
    fn page(&mut self, scope: &Scope) -> &mut Page {
        let fresh = PageEpoch(self.next_epoch);
        let page = self
            .pages
            .entry(scope.connection.clone())
            .or_insert_with(|| Page::new(scope.clone(), fresh));
        if page.scope != *scope {
            *page = Page::new(scope.clone(), fresh);
        }
        if page.epoch == fresh {
            self.next_epoch += 1;
        }
        page
    }

    /// The board `scope`'s page is about to show, or else shows.
    #[cfg(test)]
    pub(super) fn board(&self, scope: &Scope) -> Option<&Board> {
        current_ref(&self.pages, scope)?.board()
    }

    /// The board `scope`'s page shows, as far as this run knows.
    #[cfg(test)]
    pub(super) fn published(&self, scope: &Scope) -> Option<&Board> {
        current_ref(&self.pages, scope)?.published()
    }

    #[cfg(test)]
    pub(super) fn viewer(&self, scope: &Scope) -> Option<Viewer> {
        current_ref(&self.pages, scope).map(|page| page.viewer)
    }
}

/// The page that `scope` still serves, if no other identity replaced it.
fn current<'a>(pages: &'a mut HashMap<String, Page>, scope: &Scope) -> Option<&'a mut Page> {
    pages
        .get_mut(&scope.connection)
        .filter(|page| page.scope == *scope)
}

fn current_ref<'a>(pages: &'a HashMap<String, Page>, scope: &Scope) -> Option<&'a Page> {
    pages
        .get(&scope.connection)
        .filter(|page| page.scope == *scope)
}
