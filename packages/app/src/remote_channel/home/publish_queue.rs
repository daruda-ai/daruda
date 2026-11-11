//! What each connection's page should show next, what it shows, whether a
//! publish is under way, and the thread links it has learned. GPUI-free;
//! `driver` drives it. Like the thread roots' queue: the latest board wins,
//! and a board the page already shows is not sent again.

use super::board::{Board, Outcome};
use super::token::Tap;
use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::threads::Scope;
use crate::remote_channel::transport::{LinkFailure, PageVerdict};
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
#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    /// The board on its way, which may already be on screen.
    InFlight(Board),
    /// Failed for a passing reason, or its connection could not send; built
    /// anew on the retry tick or the person's next opening.
    Waiting,
}

/// What a page knows of the link to one thread root. A message's link does
/// not change, so a known one is kept as long as the page.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Link {
    /// Being asked for; the board goes out without it meanwhile.
    Pending,
    Known(String),
    /// The last ask failed for a passing reason; asked again on the retry
    /// tick or the person's next opening.
    Failed,
    /// The channel refused to link this root: not asked again for this page,
    /// since the same ask would be refused again.
    Refused,
}

/// What a link that landed came to.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Learned {
    /// Known now: the page is worth building again.
    Known,
    /// Failed for a passing reason; `first` the first time this root failed
    /// on this page.
    Failed { first: bool },
    /// Refused: the row goes without a link for as long as the page.
    Refused,
    /// For a page that has been replaced or forgotten since.
    Stale,
}

/// Which page, of all a connection had, an ask was made for: a page another
/// identity replaced, even one under the same identity again, has another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PageEpoch(u64);

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

/// One connection's page, under the identity that serves it.
struct Page {
    scope: Scope,
    /// Tells this page's link answers from a replaced page's.
    epoch: PageEpoch,
    viewer: Viewer,
    wanted: Option<Board>,
    /// What the page shows now, as far as this run knows.
    published: Option<Board>,
    phase: Option<Phase>,
    /// The board the channel refused last: an equal board would be refused
    /// again, so it is not sent until another has gone out.
    refused: Option<Board>,
    /// What the person's last tap came to, shown atop every later board.
    outcome: Option<Outcome>,
    /// The link to each thread root the page's boards have shown.
    links: HashMap<String, Link>,
    /// Roots whose link failed on this page, so each failure is reported
    /// once rather than on every attempt.
    told_failed: HashSet<String>,
}

impl Page {
    fn new(scope: Scope, epoch: PageEpoch) -> Self {
        Self {
            scope,
            epoch,
            viewer: Viewer::NotSeen,
            wanted: None,
            published: None,
            phase: None,
            refused: None,
            outcome: None,
            links: HashMap::new(),
            told_failed: HashSet::new(),
        }
    }

    /// Give up what waits for a later trigger: a board that could not go out
    /// is built anew, since the panes have moved on meanwhile, and links that
    /// failed for a passing reason are asked again. Whether there was anything to give up.
    fn resume(&mut self) -> bool {
        let waiting = self.phase == Some(Phase::Waiting);
        if waiting {
            self.phase = None;
            self.wanted = None;
        }
        let links = self.links.len();
        self.links.retain(|_, link| *link != Link::Failed);
        waiting || self.links.len() != links
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
    /// one in flight, or on the one wanted next; either of the last two may
    /// already be on screen.
    pub(super) fn resolve(&self, scope: &Scope, tap: Tap) -> Option<PaneRef> {
        let page = current_ref(&self.pages, scope)?;
        let in_flight = match &page.phase {
            Some(Phase::InFlight(board)) => Some(board),
            _ => None,
        };
        [page.published.as_ref(), in_flight, page.wanted.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|board| board.resolve(tap))
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

    /// The boards to publish now: wanted, not already shown, not refused
    /// last, not in flight, and not waiting.
    pub(super) fn ready(&mut self) -> Vec<Publication> {
        let mut ready = Vec::new();
        for page in self.pages.values_mut() {
            // With a publish in flight, what the page will show is not known
            // until it settles; the wanted board is compared then.
            let in_flight = matches!(page.phase, Some(Phase::InFlight(_)));
            if !in_flight && page.wanted.is_some() && page.wanted == page.published {
                page.wanted = None;
            }
            if page.wanted.is_some() && page.wanted == page.refused {
                page.wanted = None;
            }
            if page.viewer != Viewer::Viewing || page.phase.is_some() {
                continue;
            }
            if let Some(board) = &page.wanted {
                page.refused = None;
                page.phase = Some(Phase::InFlight(board.clone()));
                ready.push(Publication {
                    scope: page.scope.clone(),
                    board: board.clone(),
                });
            }
        }
        ready
    }

    /// Record how `publication` went.
    pub(super) fn finish(&mut self, publication: &Publication, verdict: &PageVerdict) -> Settled {
        let Some(page) = current(&mut self.pages, &publication.scope) else {
            return Settled::Stale;
        };
        page.phase = None;
        match verdict {
            PageVerdict::Published => {
                page.published = Some(publication.board.clone());
                Settled::Published
            }
            PageVerdict::Off => {
                page.viewer = Viewer::Unavailable;
                page.wanted = None;
                // The channel may not keep the old view across the page
                // going off and on, so the next opening publishes again.
                page.published = None;
                Settled::Unavailable {
                    first: self
                        .told_unavailable
                        .insert(publication.scope.connection.clone()),
                }
            }
            PageVerdict::Transient(_) => {
                page.phase = Some(Phase::Waiting);
                Settled::Retry
            }
            PageVerdict::Refused(_) => {
                if page.wanted.as_ref() == Some(&publication.board) {
                    page.wanted = None;
                }
                page.refused = Some(publication.board.clone());
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

    /// Give `board` the links `scope`'s page knows for its threads.
    pub(super) fn link(&self, scope: &Scope, board: &mut Board) {
        let Some(page) = current_ref(&self.pages, scope) else {
            return;
        };
        board.link(|root| match page.links.get(root) {
            Some(Link::Known(link)) => Some(link.clone()),
            Some(Link::Pending | Link::Failed | Link::Refused) | None => None,
        });
    }

    /// The roots among `roots` whose link `scope`'s page has not asked for
    /// yet, each marked as being asked for now. `None` without a page.
    pub(super) fn ask_links<'a>(
        &mut self,
        scope: &Scope,
        roots: impl IntoIterator<Item = &'a str>,
    ) -> Option<LinkAsk> {
        let page = current(&mut self.pages, scope)?;
        let mut asked = Vec::new();
        for root in roots {
            if !page.links.contains_key(root) {
                page.links.insert(root.to_owned(), Link::Pending);
                asked.push(root.to_owned());
            }
        }
        Some(LinkAsk {
            epoch: page.epoch,
            roots: asked,
        })
    }

    /// The link `scope`'s page of `epoch` asked for `root` came back as
    /// `link`, or failed. Every ask of a page is answered for that page
    /// alone, so none is left pending by a replaced page's answer.
    pub(super) fn learn_link(
        &mut self,
        scope: &Scope,
        epoch: PageEpoch,
        root: &str,
        link: &Result<String, LinkFailure>,
    ) -> Learned {
        let Some(page) = current(&mut self.pages, scope).filter(|page| page.epoch == epoch) else {
            return Learned::Stale;
        };
        // Only this page's ask marked `root` pending, and nothing else
        // settles it, so the answer is kept as it is.
        let (entry, learned) = match link {
            Ok(link) => (Link::Known(link.clone()), Learned::Known),
            Err(LinkFailure::Transient(_)) => (
                Link::Failed,
                Learned::Failed {
                    first: page.told_failed.insert(root.to_owned()),
                },
            ),
            Err(LinkFailure::Refused(_)) => (Link::Refused, Learned::Refused),
        };
        page.links.insert(root.to_owned(), entry);
        learned
    }

    /// Forget the pages of scopes no connection can serve any more.
    pub(super) fn keep_scopes(&mut self, alive: impl Fn(&Scope) -> bool) {
        self.pages.retain(|_, page| alive(&page.scope));
    }

    /// The page `scope`'s connection has under `scope`; another identity's
    /// page is forgotten for a new one.
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
        let page = current_ref(&self.pages, scope)?;
        page.wanted.as_ref().or(page.published.as_ref())
    }

    /// The board `scope`'s page shows, as far as this run knows.
    #[cfg(test)]
    pub(super) fn published(&self, scope: &Scope) -> Option<&Board> {
        current_ref(&self.pages, scope)?.published.as_ref()
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

fn current_ref<'a>(pages: &'a HashMap<String, Page>, scope: &Scope) -> Option<&'a Page> {
    pages
        .get(&scope.connection)
        .filter(|page| page.scope == *scope)
}
