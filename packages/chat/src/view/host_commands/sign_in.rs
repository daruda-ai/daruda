//! What a fresh sign-in changes in a pane that was refused for its login.

use daruda_acp::ChatItem;
use gpui::Context;

use super::super::AgentChatView;

impl AgentChatView {
    /// The credentials this pane runs on were just signed in again. The
    /// sign-in failures ending the conversation no longer describe it, and
    /// their button would start a second login for nothing, so they go.
    ///
    /// Only the trailing ones: a failure the conversation has since moved past
    /// is history. And not while the failed turn's completion is still owed
    /// (trailing subagents keep the pane busy): that completion reports the
    /// failure as its reason, and would go out with none.
    pub fn signed_in_again(&mut self, cx: &mut Context<Self>) {
        if self.is_read_only() || self.activity.pending_completion.is_some() {
            return;
        }
        if self.drop_trailing_items(is_sign_in_failure) {
            self.rebuild_rows();
            cx.notify();
        }
    }
}

fn is_sign_in_failure(item: &ChatItem) -> bool {
    matches!(
        item,
        ChatItem::Failure(failure) if failure.remedy() == daruda_acp::Remedy::Reauthenticate
    )
}

#[cfg(test)]
mod tests {
    use daruda_acp::{AcpFailure, ChatItem};

    use super::super::super::TurnOutcome;
    use super::super::super::tests::make_test_view;

    fn auth_failure() -> ChatItem {
        ChatItem::Failure(AcpFailure::AuthRequired {
            message: "Authentication required".into(),
        })
    }

    fn other_failure() -> ChatItem {
        ChatItem::Failure(AcpFailure::unclassified("adapter exited"))
    }

    fn prompt(text: &str) -> ChatItem {
        ChatItem::UserText(text.into())
    }

    #[gpui::test]
    fn a_sign_in_drops_only_the_trailing_sign_in_failures(cx: &mut gpui::TestAppContext) {
        // (label, transcript, what a sign-in leaves)
        let cases = [
            (
                "the refusal ending the conversation goes",
                vec![prompt("hi"), auth_failure(), auth_failure()],
                vec![prompt("hi")],
            ),
            (
                "a failure a login does not fix stays",
                vec![prompt("hi"), auth_failure(), other_failure()],
                vec![prompt("hi"), auth_failure(), other_failure()],
            ),
            (
                "a refusal the conversation moved past is history",
                vec![prompt("hi"), auth_failure(), prompt("again")],
                vec![prompt("hi"), auth_failure(), prompt("again")],
            ),
        ];
        let view = make_test_view(cx);
        for (label, items, expected) in cases {
            view.update(cx, |v, _, cx| {
                v.items = items;
                v.signed_in_again(cx);
                assert_eq!(v.items, expected, "{label}");
            })
            .unwrap();
        }
    }

    /// The completion still owed for the refused turn names the refusal as
    /// its reason; cutting it first would send a bare "failed".
    #[gpui::test]
    fn a_refusal_an_owed_completion_reports_stays(cx: &mut gpui::TestAppContext) {
        let view = make_test_view(cx);
        view.update(cx, |v, _, cx| {
            v.items = vec![prompt("hi"), auth_failure()];
            v.owe_completion_for_test(TurnOutcome::Errored);

            v.signed_in_again(cx);

            assert_eq!(v.items, vec![prompt("hi"), auth_failure()]);
            assert!(v.failure_reason().is_some());
        })
        .unwrap();
    }
}
