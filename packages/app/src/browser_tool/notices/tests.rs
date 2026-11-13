use daruda_store::observability::error_report::ErrorReport;
use gpui::TestAppContext;

use super::*;
use crate::browser_tool::Unusable;

fn held(cx: &mut TestAppContext) -> usize {
    cx.update(|cx| cx.global::<BrowserTool>().undelivered.len())
}

#[gpui::test]
fn a_notice_with_no_window_open_is_held(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(BrowserTool::unusable(Unusable::Platform));
        surface(ErrorReport::new("first").build(), cx);
        surface(ErrorReport::new("second").build(), cx);
    });
    assert_eq!(held(cx), 2);
}

#[gpui::test]
fn registering_a_window_without_the_tool_does_nothing(cx: &mut TestAppContext) {
    cx.update(on_window_registered);
    cx.run_until_parked();
    assert!(cx.update(|cx| !cx.has_global::<BrowserTool>()));
}
