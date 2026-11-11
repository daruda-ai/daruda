//! The thread links one page has learned: which roots were asked for, which
//! are known, and which failed or were refused. Each `Page` keeps its own;
//! `driver` asks for the links.

use super::super::board::Board;
use crate::remote_channel::transport::LinkFailure;
use std::collections::{HashMap, HashSet};

/// What a page knows of the link to one thread root. A message's link does
/// not change, so a known one is kept as long as the page.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Link {
    /// Being asked for; the board goes out without it meanwhile.
    Pending,
    Known(String),
    /// The last ask failed: asked again on the person's next opening. Not
    /// on the retry tick, which would ask every tick while the channel keeps
    /// answering so; meanwhile the row only lacks its button.
    Failed,
    /// The channel refused to link this root: not asked again until the
    /// person opens the page again, since the same ask would be refused.
    Refused,
}

/// What a link that landed came to.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::remote_channel::home) enum Learned {
    /// Known now: the page is worth building again.
    Known,
    /// Failed for a passing reason; `first` the first time this root failed
    /// on this page.
    Failed { first: bool },
    /// Refused: the row goes without a link until the next opening;
    /// `first` the first time this root was refused on this page.
    Refused { first: bool },
}

#[derive(Default)]
pub(super) struct PageLinks {
    /// The link to each thread root the page's boards have shown.
    links: HashMap<String, Link>,
    /// Roots whose link failed on this page, so each failure is reported
    /// once rather than on every attempt.
    told_failed: HashSet<String>,
    /// Likewise for roots whose link was refused.
    told_refused: HashSet<String>,
}

impl PageLinks {
    /// The roots among `roots` not asked for yet, each marked as being asked
    /// for now.
    pub(super) fn ask<'a>(&mut self, roots: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let mut asked = Vec::new();
        for root in roots {
            if !self.links.contains_key(root) {
                self.links.insert(root.to_owned(), Link::Pending);
                asked.push(root.to_owned());
            }
        }
        asked
    }

    /// The link asked for `root` came back as `link`, or failed. Only the
    /// page's own ask marked `root` pending, and nothing else settles it,
    /// so the answer is kept as it is.
    pub(super) fn learn(&mut self, root: &str, link: &Result<String, LinkFailure>) -> Learned {
        let (entry, learned) = match link {
            Ok(link) => (Link::Known(link.clone()), Learned::Known),
            Err(LinkFailure::Transient(_)) => (
                Link::Failed,
                Learned::Failed {
                    first: self.told_failed.insert(root.to_owned()),
                },
            ),
            Err(LinkFailure::Refused(_)) => (
                Link::Refused,
                Learned::Refused {
                    first: self.told_refused.insert(root.to_owned()),
                },
            ),
        };
        self.links.insert(root.to_owned(), entry);
        learned
    }

    /// Give `board` the links known for its threads.
    pub(super) fn fill(&self, board: &mut Board) {
        board.link(|root| match self.links.get(root) {
            Some(Link::Known(link)) => Some(link.clone()),
            Some(Link::Pending | Link::Failed | Link::Refused) | None => None,
        });
    }

    /// Let the links that failed or were refused be asked again.
    pub(super) fn forget_failures(&mut self) {
        self.links
            .retain(|_, link| !matches!(link, Link::Failed | Link::Refused));
    }
}
