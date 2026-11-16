use super::*;
use crate::ui::list_table::{self, Column, ListTable};
use gpui::{Render, ScrollHandle, TestAppContext, VisualTestContext};

#[test]
fn navigation_artwork_distinguishes_the_pages() {
    let icons: std::collections::HashSet<_> = Page::ALL.map(Page::icon).into();
    assert_eq!(icons.len(), Page::ALL.len());
}

struct PageProbe {
    state: PageState,
    horizontal: ScrollHandle,
}

impl Render for PageProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let table = ListTable::new([
            (0, Column::Flexible(200.)),
            (1, Column::Fixed(96.)),
            (2, Column::Fixed(96.)),
            (3, Column::Fixed(84.)),
            (4, Column::Fixed(112.)),
            (5, Column::FixedUnclipped(140.)),
            (6, Column::FixedUnclipped(64.)),
        ]);
        let cell = |row: &'static str, column, cell: gpui::Div| {
            cell.debug_selector(move || format!("{row}-{column}"))
                .child(if column == 0 {
                    "A title that grows with the workspace"
                } else {
                    "Value"
                })
                .into_any_element()
        };
        let body = table
            .body()
            .debug_selector(|| "page-test-table".into())
            .child(table.header(|column, div| cell("heading", column, div), cx))
            .child(table.row(|column, div| cell("record", column, div)));
        frame(
            &self.state,
            div()
                .w_full()
                .h(px(32.))
                .debug_selector(|| "page-test-header".into())
                .into_any_element(),
            list_table::scroll("page-test-scroll", body)
                .track_scroll(&self.horizontal)
                .debug_selector(|| "page-test-table-scroll".into())
                .into_any_element(),
            cx,
        )
    }
}

#[gpui::test]
fn tables_fill_wide_pages_and_keep_columns_aligned_when_scrolling(cx: &mut TestAppContext) {
    crate::test_support::init_gpui_component(cx);
    let window = cx.add_window(|window, cx| {
        let probe = cx.new(|_| PageProbe {
            state: PageState::new(Page::Tasks),
            horizontal: ScrollHandle::new(),
        });
        crate::ui::Root::new(probe, window, cx)
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    let mut wide_title = None;
    for width in [800., 1280., 1920.] {
        vcx.simulate_resize(gpui::size(px(width), px(800.)));
        vcx.run_until_parked();
        let viewport = vcx.debug_bounds("workspace-page-scroll").unwrap();
        let content = vcx.debug_bounds("workspace-page-content").unwrap();
        let scroll = vcx.debug_bounds("page-test-table-scroll").unwrap();
        let table = vcx.debug_bounds("page-test-table").unwrap();
        let title = vcx.debug_bounds("record-0").unwrap();
        let heading = vcx.debug_bounds("heading-6").unwrap();
        let action = vcx.debug_bounds("record-6").unwrap();
        assert_eq!(viewport.size.width, px(width));
        assert_eq!(
            viewport.bottom(),
            px(800.),
            "the page fills the available height"
        );
        assert_eq!(
            content.size.width, viewport.size.width,
            "the body has no reading-width cap"
        );
        assert_eq!(scroll.left(), viewport.left() + px(theme::DOCK_PAGE_PAD));
        assert_eq!(scroll.right(), viewport.right() - px(theme::DOCK_PAGE_PAD));
        assert_eq!(
            heading.left(),
            action.left(),
            "header and body columns share geometry"
        );
        assert_eq!(heading.right(), action.right());
        assert_eq!(
            action.size.width,
            px(64.),
            "controls retain their width budget"
        );
        if width == 800. {
            assert!(
                table.size.width > scroll.size.width,
                "narrow views keep scrollable columns"
            );
            assert!(title.size.width >= px(theme::list_metrics::TITLE_MIN_W));
            vcx.simulate_event(gpui::ScrollEvent {
                source: gpui::ScrollSource::Pointer,
                position: scroll.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(-200.), px(0.))),
                touch_phase: gpui::TouchPhase::Started,
                modifiers: Default::default(),
            });
            vcx.run_until_parked();
            let shifted = vcx.debug_bounds("record-0").unwrap();
            let header = vcx.debug_bounds("heading-0").unwrap();
            assert!(
                shifted.left() < title.left(),
                "horizontal scrolling reaches off-screen columns"
            );
            assert_eq!(header.left(), shifted.left());
        } else {
            assert_eq!(
                table.size.width, scroll.size.width,
                "tables consume the full available width"
            );
            assert_eq!(action.right(), scroll.right() - px(theme::PAD_LG));
            if let Some(previous) = wide_title {
                assert_eq!(title.size.width - previous, px(1920. - 1280.));
            }
            wide_title = Some(title.size.width);
        }
    }
}
