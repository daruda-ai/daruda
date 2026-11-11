use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::threads::roots;
use crate::remote_channel::threads::status::{Ending, PaneActivity};

/// The home hears of a report only when the pane's shown status changed: a
/// busy pane's repeated pulse costs it nothing, and closing always counts.
#[gpui::test]
fn only_a_changed_status_counts_as_a_change(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let _outgoing = crate::remote_channel::global::tests::setup(cx);
        let pane = PaneRef {
            workspace: daruda_store::project::WorkspaceUuid::new(),
            pane: 1,
        };
        let cases = [
            (PaneActivity::Working, true, "work begins"),
            (
                PaneActivity::Working,
                false,
                "the next pulse, still working",
            ),
            (PaneActivity::AwaitingYou, true, "paused on the person"),
            (PaneActivity::AwaitingYou, false, "still waiting"),
            (PaneActivity::Ended(Ending::Completed), true, "ended"),
            (PaneActivity::Closed, true, "closed"),
            (PaneActivity::Closed, false, "closed already"),
        ];
        for (activity, changed, label) in cases {
            assert_eq!(roots::pane_activity(pane, activity, cx), changed, "{label}");
        }
        let never_reported = PaneRef { pane: 2, ..pane };
        assert!(
            roots::pane_activity(never_reported, PaneActivity::Closed, cx),
            "a pane closing before it reported"
        );
    });
}
