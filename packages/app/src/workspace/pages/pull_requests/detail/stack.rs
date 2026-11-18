//! The shown pull request's native stack: read with the detail, shown beside
//! it with each pull request of the stack to go to. A server without stacks,
//! or one not stacked, shows nothing. A refresh reads it again only while it
//! is stacked (or not read yet): one answered unstacked, or a server without
//! stacks, is not asked again on every refresh while the detail is open. An
//! auto-merge with the stack reads it again itself before it goes.

use daruda_github::{GhError, StackRead};
use gpui::Context;

use super::read_slot::{ReadSlot, ReadTicket};
use super::{PrDetailId, PrDetailState};
use crate::gh_host::GhHost;
use crate::workspace::Workspace;
use crate::workspace::spawn_helpers::spawn_bg_work_and_mutate;

impl PrDetailState {
    /// The stack as last read, kept up while a refresh reads it again: an
    /// auto-merge with the stack checks the range anew before it goes.
    pub(in crate::workspace) fn stack(&self) -> Option<&StackRead> {
        self.stack.as_ref().and_then(ReadSlot::last_known)
    }

    /// Whether a read of the detail reads the stack too: not once GitHub
    /// answered that there is none to show.
    pub(super) fn stack_worth_reading(&self) -> bool {
        !matches!(
            self.stack(),
            Some(StackRead::NotStacked | StackRead::Unsupported)
        )
    }

    fn begin_stack_read(&mut self) -> ReadTicket {
        let ticket = self.tickets.issue();
        ReadSlot::begin(&mut self.stack, ticket);
        ticket
    }
}

impl Workspace {
    /// Read the stack of the detail `id` shows, keeping what is shown.
    pub(super) fn read_pr_stack(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let ticket = detail.begin_stack_read();
        let (repo, number) = (detail.item.repo.clone(), detail.item.pr.number);
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || gh.pr_stack(&repo, number),
            move |ws, result, cx| ws.land_pr_stack(id, ticket, result, cx),
        )
        .detach();
    }

    /// A failed read keeps what was shown: the stack is beside the point
    /// of the detail, so it is not reported.
    fn land_pr_stack(
        &mut self,
        id: PrDetailId,
        ticket: ReadTicket,
        result: Result<StackRead, GhError>,
        cx: &mut Context<Self>,
    ) {
        // Logged, not toasted: lasting breakage stays diagnosable.
        if let Err(e) = &result {
            daruda_store::observability::log_writer::LogWriter::log(
                daruda_store::observability::error_report::ErrorReport::new(
                    crate::surface::strings::error::github_pr_stack_read_failed(),
                )
                .severity(daruda_store::observability::error_report::ErrorSeverity::Warning)
                .from_error(e)
                .at(file!(), line!())
                .dedup("github.pr_stack.read")
                .build(),
            );
        }
        let Some(slot) = self.pr_detail_mut(id).and_then(|d| d.stack.as_mut()) else {
            return;
        };
        if slot.land_keeping_ready(ticket, result.map_err(|e| e.to_string())) {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{PrDetailId, PrDetailState};
    use daruda_github::StackRead;

    /// A newer read wins: the answer to one asked before it is dropped.
    #[test]
    fn an_older_answer_is_dropped() {
        let item = crate::workspace::tests::pull_request_detail::item(7, "feat");
        let mut state = PrDetailState::new(PrDetailId(1), item);
        let first = state.begin_stack_read();
        let second = state.begin_stack_read();
        let slot = state.stack.as_mut().unwrap();
        assert!(!slot.land(first, Ok(StackRead::Unsupported)));
        assert!(slot.land(second, Ok(StackRead::NotStacked)));
        assert_eq!(state.stack(), Some(&StackRead::NotStacked));
    }

    /// A refresh keeps the stack up until its answer, failed or not.
    #[test]
    fn a_refresh_keeps_the_stack_shown() {
        let item = crate::workspace::tests::pull_request_detail::item(7, "feat");
        let mut state = PrDetailState::new(PrDetailId(1), item);
        let first = state.begin_stack_read();
        assert!(
            state
                .stack
                .as_mut()
                .unwrap()
                .land(first, Ok(StackRead::NotStacked))
        );
        let again = state.begin_stack_read();
        assert_eq!(
            state.stack(),
            Some(&StackRead::NotStacked),
            "while rereading"
        );
        let slot = state.stack.as_mut().unwrap();
        assert!(slot.land_keeping_ready(again, Err("offline".into())));
        assert_eq!(
            state.stack(),
            Some(&StackRead::NotStacked),
            "after a failure"
        );
    }
}
