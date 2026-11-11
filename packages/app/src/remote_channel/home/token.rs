//! The values a page's row buttons carry, and the numbering that keeps them
//! naming the board they were shown on. GPUI-free.

use crate::remote_channel::bridge::PaneRef;

/// Distinct from every other button value: a card's is a bare
/// `Uuid::simple` or `apv:` / `qst:` / `faq:` plus one, a listing row's
/// `lst:`, a thread root's `thr:stop:`.
const PREFIX: &str = "hom";
const SELECT: &str = "sel";
const STOP: &str = "stop";

/// What a row button asks of its pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Act {
    Select,
    Stop,
}

/// A row button's value, read back: the act, and the row of the board
/// numbered `generation` it was shown on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tap {
    pub generation: u64,
    pub index: usize,
    pub act: Act,
}

impl Tap {
    pub(super) fn token(self) -> String {
        let act = match self.act {
            Act::Select => SELECT,
            Act::Stop => STOP,
        };
        format!("{PREFIX}:{}:{}:{act}", self.generation, self.index)
    }

    /// `None` when `data` is no row button's value.
    pub(crate) fn parse(data: &str) -> Option<Self> {
        let mut parts = data.split(':');
        if parts.next()? != PREFIX {
            return None;
        }
        let generation = parts.next()?.parse().ok()?;
        let index = parts.next()?.parse().ok()?;
        let act = match parts.next()? {
            SELECT => Act::Select,
            STOP => Act::Stop,
            _ => return None,
        };
        parts.next().is_none().then_some(Self {
            generation,
            index,
            act,
        })
    }
}

/// The generation a board's buttons carry. The page's own, apart from the
/// `/list` table's, so neither expires the other's buttons. Seeded at random
/// because a page outlives a restart on the person's screen, and bumped only
/// when the rows name other panes, so an unchanged board is not sent again.
pub(super) struct Numbering {
    generation: u64,
    panes: Vec<PaneRef>,
}

impl Default for Numbering {
    fn default() -> Self {
        Self {
            generation: uuid::Uuid::new_v4().as_u128() as u64,
            panes: Vec::new(),
        }
    }
}

impl Numbering {
    /// The generation for a board whose rows name `panes`, in order.
    pub(super) fn number(&mut self, panes: Vec<PaneRef>) -> u64 {
        if panes != self.panes {
            self.generation = self.generation.wrapping_add(1);
            self.panes = panes;
        }
        self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(pane: u64) -> PaneRef {
        PaneRef {
            workspace: Default::default(),
            pane,
        }
    }

    #[test]
    fn a_value_reads_back_as_the_tap_it_was_made_from() {
        for act in [Act::Select, Act::Stop] {
            let tap = Tap {
                generation: u64::MAX,
                index: 47,
                act,
            };
            assert_eq!(Tap::parse(&tap.token()), Some(tap), "{act:?}");
        }
    }

    /// No other button value reads as a row button's, nor a row button's as
    /// a thread stop or a `/list` row.
    #[test]
    fn a_row_value_is_no_other_button_value() {
        let thread = uuid::Uuid::new_v4().simple().to_string();
        let malformed = [
            "hom:1:2".to_owned(),
            "hom:1:2:go".to_owned(),
            "hom:x:2:sel".to_owned(),
            "hom:1:-2:sel".to_owned(),
            "hom:1:2:sel:3".to_owned(),
            "hom".to_owned(),
            String::new(),
            "lst:7:0".to_owned(),
            crate::remote_channel::threads::stop::token(&thread),
            thread.clone(),
            format!("apv:{thread}"),
            format!("qst:{thread}"),
            format!("faq:{thread}"),
        ];
        for value in malformed {
            assert_eq!(Tap::parse(&value), None, "{value:?}");
        }
        let state = crate::remote_channel::command::CommandState::default();
        for act in [Act::Select, Act::Stop] {
            let token = Tap {
                generation: 7,
                index: 0,
                act,
            }
            .token();
            assert_eq!(
                crate::remote_channel::threads::stop::thread_of(&token),
                None
            );
            assert_eq!(state.resolve_token(&token), None);
        }
    }

    /// The same panes keep their generation; other panes, or the same in
    /// another order, take a new one.
    #[test]
    fn a_board_naming_other_panes_takes_a_new_generation() {
        let mut numbering = Numbering::default();
        let first = numbering.number(vec![pane(1), pane(2)]);
        assert_eq!(numbering.number(vec![pane(1), pane(2)]), first);
        let reordered = numbering.number(vec![pane(2), pane(1)]);
        assert_ne!(reordered, first);
        assert_ne!(numbering.number(vec![pane(1), pane(2)]), reordered);
    }
}
