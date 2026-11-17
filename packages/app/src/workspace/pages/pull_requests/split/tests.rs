use gpui::TestAppContext;

use crate::ui::theme;
use crate::workspace::pages::Page;
use crate::workspace::pages::pull_requests::page_layout::PageLayout;
use crate::workspace::tests::pull_request_page::{item, linked_window};

#[gpui::test]
fn a_drag_sets_the_width_and_a_narrow_window_does_not_overwrite_it(cx: &mut TestAppContext) {
    let (f, _gh, _) = linked_window(cx);
    f.ws.update(cx, |ws, cx| {
        ws.show_page(Page::PullRequests, cx);
        ws.pages.pull_requests.measure(1400.0);
        let start = ws.pages.pull_requests.list_width();
        assert_eq!(start, theme::PR_SPLIT_LIST_W);

        ws.press_pr_divider(600.0, 1, cx);
        assert!(ws.drag_pr_divider(700.0, cx));
        assert_eq!(ws.pages.pull_requests.list_width(), start + 100.0);
        ws.drag_pr_divider(5000.0, cx);
        let widest = 1400.0 - theme::PR_SPLIT_DETAIL_MIN_W;
        assert_eq!(
            ws.pages.pull_requests.list_width(),
            widest,
            "the detail keeps its minimum"
        );
        ws.end_pr_divider_drag(cx);
        assert!(
            !ws.drag_pr_divider(800.0, cx),
            "a move after release drags nothing"
        );

        ws.pages.pull_requests.measure(1000.0);
        assert_eq!(
            ws.pages.pull_requests.list_width(),
            1000.0 - theme::PR_SPLIT_DETAIL_MIN_W,
            "a narrower window narrows the drawn list"
        );
        ws.pages.pull_requests.measure(1400.0);
        assert_eq!(
            ws.pages.pull_requests.list_width(),
            widest,
            "and widening it restores the chosen width"
        );

        ws.press_pr_divider(900.0, 2, cx);
        assert_eq!(
            ws.pages.pull_requests.list_width(),
            start,
            "a double press resets"
        );
        assert!(!ws.pr_divider_dragging());
    });
}

#[gpui::test]
fn the_list_beside_a_detail_can_be_hidden_and_shown_again(cx: &mut TestAppContext) {
    let (f, _gh, _) = linked_window(cx);
    let mut vcx = gpui::VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.open_page(Page::PullRequests, window, cx);
            ws.pages.pull_requests.measure(1400.0);
            assert_eq!(
                ws.pr_list_column_toggle(),
                None,
                "nothing to hide without a detail"
            );
            ws.open_pr_detail(item(8), window, cx);
            assert_eq!(ws.pr_page_layout(), PageLayout::Split);
            assert_eq!(ws.pr_list_column_toggle(), Some(true));

            ws.toggle_pr_list_column(window, cx);
            assert_eq!(ws.pr_page_layout(), PageLayout::Detail);
            assert_eq!(ws.pr_list_column_toggle(), Some(false));
            assert!(ws.window_runtime.focus_handle.is_focused(window));

            ws.toggle_pr_list_column(window, cx);
            assert_eq!(ws.pr_page_layout(), PageLayout::Split);
            assert!(ws.pages.pull_requests.focus_handle().is_focused(window));

            ws.pages.pull_requests.measure(800.0);
            assert_eq!(
                ws.pr_list_column_toggle(),
                None,
                "a narrow window offers nothing"
            );
        });
    });
}

#[gpui::test]
fn the_saved_layout_keeps_only_deliberate_choices(cx: &mut TestAppContext) {
    use daruda_store::project::PullRequestsLayout;

    let (f, _gh, _) = linked_window(cx);
    f.ws.update(cx, |ws, cx| {
        let page = &mut ws.pages.pull_requests;
        assert_eq!(
            page.layout_record(),
            PullRequestsLayout::default(),
            "nothing chosen, nothing kept"
        );
        page.measure(1400.0);
        ws.press_pr_divider(600.0, 1, cx);
        ws.drag_pr_divider(700.0, cx);
        ws.end_pr_divider_drag(cx);
        let record = ws.pages.pull_requests.layout_record();
        assert_eq!(record.list_width, Some(theme::PR_SPLIT_LIST_W + 100.0));
        assert!(!record.list_hidden);

        // (label, saved, restored width, hidden)
        let cases = [
            (
                "a saved choice comes back",
                PullRequestsLayout {
                    list_width: Some(480.0),
                    list_hidden: true,
                },
                480.0,
                true,
            ),
            (
                "an older file has none",
                PullRequestsLayout::default(),
                theme::PR_SPLIT_LIST_W,
                false,
            ),
            (
                "an unusable width keeps the default",
                PullRequestsLayout {
                    list_width: Some(f32::NAN),
                    list_hidden: false,
                },
                theme::PR_SPLIT_LIST_W,
                false,
            ),
            (
                "a width under the minimum is raised",
                PullRequestsLayout {
                    list_width: Some(10.0),
                    list_hidden: false,
                },
                theme::PR_SPLIT_LIST_MIN_W,
                false,
            ),
        ];
        for (label, saved, width, hidden) in cases {
            let page = &mut ws.pages.pull_requests;
            page.restore_layout(&saved);
            assert_eq!(page.list_width(), width, "{label}");
            assert_eq!(page.layout_record().list_hidden, hidden, "{label}");
        }
    });
}

#[gpui::test]
fn the_layout_survives_a_save_and_restore(cx: &mut TestAppContext) {
    let (f, _gh, _) = linked_window(cx);
    f.ws.update(cx, |ws, cx| {
        ws.pages.pull_requests.measure(1400.0);
        ws.press_pr_divider(600.0, 1, cx);
        ws.drag_pr_divider(660.0, cx);
        ws.end_pr_divider_drag(cx);
        let (saved, _) = ws.snapshot_for_disk(cx);
        assert_eq!(
            saved.pull_requests.list_width,
            Some(theme::PR_SPLIT_LIST_W + 60.0)
        );
        ws.pages.pull_requests.restore_layout(&Default::default());
        ws.pages.pull_requests.restore_layout(&saved.pull_requests);
        assert_eq!(
            ws.pages.pull_requests.list_width(),
            theme::PR_SPLIT_LIST_W + 60.0
        );
    });
}
