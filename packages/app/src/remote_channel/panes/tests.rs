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

/// While the person views a page, every report that can change it schedules
/// one rebuild of it: a pane opening, a changed status, a name. An unchanged
/// status does not.
#[gpui::test]
fn reports_that_change_a_viewed_page_schedule_its_rebuild(cx: &mut gpui::TestAppContext) {
    use crate::remote_channel::global::RemoteChannels;
    use crate::remote_channel::threads::RootName;
    cx.update(|cx| {
        let _outgoing = crate::remote_channel::global::tests::setup(cx);
        crate::remote_channel::home::driver::opened("work", "1", "3", cx);
    });
    cx.run_until_parked();
    let pane = PaneRef {
        workspace: daruda_store::project::WorkspaceUuid::new(),
        pane: 1,
    };
    type Report = Box<dyn Fn(&mut gpui::App)>;
    let cases: [(&str, Report, bool); 5] = [
        ("a pane opens", Box::new(super::opened), true),
        (
            "work begins",
            Box::new(move |cx| super::activity(pane, PaneActivity::Working, cx)),
            true,
        ),
        (
            "the next pulse, still working",
            Box::new(move |cx| super::activity(pane, PaneActivity::Working, cx)),
            false,
        ),
        (
            "a name, though the pane has no thread",
            Box::new(move |cx| {
                let name = RootName {
                    title: "chat".into(),
                    subject: None,
                };
                super::named(pane, &name, cx)
            }),
            true,
        ),
        (
            "closed",
            Box::new(move |cx| super::activity(pane, PaneActivity::Closed, cx)),
            true,
        ),
    ];
    for (label, report, scheduled) in cases {
        cx.update(|cx| {
            report(cx);
            assert_eq!(
                cx.global::<RemoteChannels>()
                    .home
                    .rebuild_scheduled_for_test(),
                scheduled,
                "{label}"
            );
        });
        cx.run_until_parked();
    }
}
