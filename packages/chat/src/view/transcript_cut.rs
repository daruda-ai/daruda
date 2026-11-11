//! Cutting the conversation's tail, the one way items leave `items`.

use daruda_acp::ChatItem;

use super::super::agent_chat_helpers::fold_key_index;
use super::AgentChatView;

impl AgentChatView {
    /// Cut the conversation to its first `len` items, and with it every record
    /// keyed by an index past the cut — the next item pushed would reuse that
    /// index and inherit the record. Rows are left to the caller's rebuild.
    pub(super) fn truncate_transcript(&mut self, len: usize) {
        let items = &self.items;
        // Resolved against the transcript before the cut, while a key past it
        // can still name its item.
        self.fold.truncate(len, |key| fold_key_index(key, items));
        self.items.truncate(len);
        self.overage_stop_markers.retain(|&marker| marker < len);
        self.activity.turn_records.retain(|&record, _| record < len);
    }

    /// Cut the run of trailing items `drop` selects. `false` when the last
    /// item is not one of them and nothing changed.
    pub(super) fn drop_trailing_items(&mut self, drop: impl Fn(&ChatItem) -> bool) -> bool {
        let kept = self
            .items
            .iter()
            .rposition(|item| !drop(item))
            .map_or(0, |ix| ix + 1);
        if kept == self.items.len() {
            return false;
        }
        self.truncate_transcript(kept);
        true
    }
}

#[cfg(test)]
mod tests {
    use daruda_acp::{AcpFailure, ChatItem};

    use super::super::tests::make_test_view;

    fn failure() -> ChatItem {
        ChatItem::Failure(AcpFailure::unclassified("adapter exited"))
    }

    fn is_failure(item: &ChatItem) -> bool {
        matches!(item, ChatItem::Failure(_))
    }

    #[gpui::test]
    fn only_the_trailing_run_is_cut(cx: &mut gpui::TestAppContext) {
        let prompt = || ChatItem::UserText("hi".into());
        // (label, transcript, what the cut leaves, whether it changed anything)
        let cases = [
            (
                "a trailing run goes whole",
                vec![prompt(), failure(), failure()],
                vec![prompt()],
                true,
            ),
            ("all of it", vec![failure()], vec![], true),
            (
                "a match the conversation moved past stays",
                vec![failure(), prompt()],
                vec![failure(), prompt()],
                false,
            ),
        ];
        let view = make_test_view(cx);
        for (label, items, expected, changed) in cases {
            view.update(cx, |v, _, _| {
                v.items = items;
                assert_eq!(v.drop_trailing_items(is_failure), changed, "{label}");
                assert_eq!(v.items, expected, "{label}");
            })
            .unwrap();
        }
    }

    /// A record filed under a cut item's index would otherwise hang on the
    /// next item that lands there.
    #[gpui::test]
    fn a_cut_forgets_the_records_of_what_it_dropped(cx: &mut gpui::TestAppContext) {
        let view = make_test_view(cx);
        view.update(cx, |v, _, _| {
            v.items = vec![ChatItem::UserText("hi".into()), failure()];
            v.record_turn_for_test(std::time::Duration::from_secs(3));
            v.overage_stop_markers.insert(1);
            assert!(v.activity.turn_records.contains_key(&1));

            v.drop_trailing_items(is_failure);

            assert!(v.activity.turn_records.is_empty());
            assert!(v.overage_stop_markers.is_empty());
        })
        .unwrap();
    }
}
