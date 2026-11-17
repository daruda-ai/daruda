//! Which read of something the detail shows may land. A refresh, a closed
//! log or a replaced slot leaves the earlier read in flight; its answer must
//! not fill the slot a newer read waits in.

use super::Load;

/// Names one read of one detail. Issued by [`Tickets`] and never reused
/// within that detail, so a slot rebuilt after a close or a refresh still
/// refuses an answer to a read made before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) struct ReadTicket(u64);

/// Issues a detail's [`ReadTicket`]s.
#[derive(Debug, Default)]
pub(super) struct Tickets(u64);

impl Tickets {
    pub(super) fn issue(&mut self) -> ReadTicket {
        self.0 += 1;
        ReadTicket(self.0)
    }
}

/// Something read from GitHub, and the one read whose answer it waits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct ReadSlot<T> {
    waiting: Option<ReadTicket>,
    load: Load<T>,
}

impl<T> ReadSlot<T> {
    /// A slot waiting on its first read.
    pub(super) fn reading(ticket: ReadTicket) -> Self {
        Self {
            waiting: Some(ticket),
            load: Load::Loading,
        }
    }

    /// A slot holding `load` with no read out, as a capture seeds it.
    pub(in crate::workspace) fn settled(load: Load<T>) -> Self {
        Self {
            waiting: None,
            load,
        }
    }

    /// Read again; what is shown stays until the answer.
    pub(super) fn reread(&mut self, ticket: ReadTicket) {
        self.waiting = Some(ticket);
    }

    pub(in crate::workspace) fn load(&self) -> &Load<T> {
        &self.load
    }

    /// What was read, when no further read of it is out.
    pub(super) fn current(&self) -> Option<&T> {
        match (&self.load, self.waiting) {
            (Load::Ready(value), None) => Some(value),
            _ => None,
        }
    }

    /// Take `answer` if it is the read this slot waits on; `false` and
    /// nothing changed otherwise. Each read lands at most once.
    pub(super) fn land(&mut self, ticket: ReadTicket, answer: Result<T, String>) -> bool {
        if self.waiting != Some(ticket) {
            return false;
        }
        self.waiting = None;
        self.load = match answer {
            Ok(value) => Load::Ready(value),
            Err(reason) => Load::Failed(reason),
        };
        true
    }

    /// As [`Self::land`], but a failure keeps a value already shown: a
    /// refresh that fails says so elsewhere and leaves the last read up.
    pub(super) fn land_keeping_ready(
        &mut self,
        ticket: ReadTicket,
        answer: Result<T, String>,
    ) -> bool {
        match answer {
            Err(_) if matches!(self.load, Load::Ready(_)) && self.waiting == Some(ticket) => {
                self.waiting = None;
                true
            }
            answer => self.land(ticket, answer),
        }
    }

    /// The read this slot waits on, for a test to answer it late.
    #[cfg(test)]
    pub(in crate::workspace) fn waiting(&self) -> Option<ReadTicket> {
        self.waiting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_read_a_slot_waits_on_lands_and_only_once() {
        let mut tickets = Tickets::default();
        let (first, second) = (tickets.issue(), tickets.issue());
        let mut slot = ReadSlot::reading(first);
        slot.reread(second);
        assert!(!slot.land(first, Ok(1)), "an outdated read");
        assert_eq!(slot.load(), &Load::Loading);
        assert!(slot.land(second, Ok(2)));
        assert_eq!(slot.load(), &Load::Ready(2));
        assert!(!slot.land(second, Ok(3)), "a read lands once");
        assert_eq!(slot.load(), &Load::Ready(2));
    }

    #[test]
    fn a_rebuilt_slot_refuses_a_read_made_before_it() {
        let mut tickets = Tickets::default();
        let before = tickets.issue();
        let slot_after_reopen = ReadSlot::<u8>::reading(tickets.issue());
        let mut slot = slot_after_reopen;
        assert!(!slot.land(before, Ok(1)));
    }

    #[test]
    fn a_failure_keeps_what_is_shown_only_when_asked() {
        let mut tickets = Tickets::default();
        // (label, keep ready, expected)
        let cases = [
            ("plain", false, Load::Failed("nope".to_owned())),
            ("keeping", true, Load::Ready("old".to_owned())),
        ];
        for (label, keep, expected) in cases {
            let mut slot = ReadSlot::settled(Load::Ready("old".to_owned()));
            let ticket = tickets.issue();
            slot.reread(ticket);
            let landed = if keep {
                slot.land_keeping_ready(ticket, Err("nope".into()))
            } else {
                slot.land(ticket, Err("nope".into()))
            };
            assert!(landed, "{label}");
            assert_eq!(slot.load(), &expected, "{label}");
            assert_eq!(slot.waiting(), None, "{label}: the read is spent");
        }
    }
}
