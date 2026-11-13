//! Browser tool notices raised before any workspace window is open.

use gpui::{AppContext as _, BorrowAppContext as _, Entity, TestAppContext};

use super::{Workspace, fresh_test_data_dir};
use crate::browser_tool::BrowserTool;
use crate::settings_store::SettingsStore;
use crate::surface::strings::browser_tool as s;
use crate::test_support::init_gpui_component;

/// Registers from inside construction, as a production window does.
fn open_registered_workspace(cx: &mut TestAppContext) -> Entity<Workspace> {
    let config = daruda_config::Config::default();
    let holder = std::cell::RefCell::new(None);
    cx.add_window(|window, cx| {
        let workspace = cx.new(|cx| {
            let ws = Workspace::new_with_project_for_test(
                &config,
                None,
                fresh_test_data_dir(),
                window,
                cx,
            );
            Workspace::join_window_registry(window, cx);
            ws
        });
        *holder.borrow_mut() = Some(workspace.clone());
        gpui_component::Root::new(workspace, window, cx)
    });
    holder.into_inner().expect("workspace constructed")
}

fn notices(workspace: &Entity<Workspace>, cx: &mut TestAppContext) -> usize {
    let title = s::unsupported_platform();
    workspace.read_with(cx, |ws, _| {
        ws.error_history()
            .iter()
            .filter(|report| report.title == title)
            .count()
    })
}

#[gpui::test]
fn a_startup_notice_reaches_the_first_window(cx: &mut TestAppContext) {
    init_gpui_component(cx);
    cx.update(|cx| {
        SettingsStore::init(cx);
        let mut config = (*SettingsStore::global(cx).user_arc()).clone();
        config.browser.enabled = true;
        cx.update_global::<SettingsStore, _>(|store, _| store.set_user_for_testing(config));
        // A test build's host has no release, so turning on raises the notice.
        BrowserTool::init(cx);
        BrowserTool::settings_changed(cx);
    });

    let first = open_registered_workspace(cx);
    cx.run_until_parked();
    let second = open_registered_workspace(cx);
    cx.run_until_parked();

    assert_eq!(notices(&first, cx), 1, "the first window shows it");
    assert_eq!(notices(&second, cx), 0, "it is shown once");
}
