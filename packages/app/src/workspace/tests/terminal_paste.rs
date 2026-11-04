//! Paste into a terminal pane through the real action dispatch: the bytes a
//! recording `TerminalInput` receives are what the PTY would get.

use super::*;
use crate::workspace::main_area::pane::PaneContent;
use daruda_terminal::view::{Paste, TerminalInput, TerminalView};
use std::sync::{Arc, Mutex};

fn paste_into_terminal(
    cx: &mut TestAppContext,
    clipboard: gpui::ClipboardItem,
    agent_running: bool,
) -> Vec<u8> {
    let (window_handle, workspace) = build_workspace(cx);
    cx.run_until_parked();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let sink = sent.clone();
    cx.update_window(window_handle.into(), |_, _, cx| {
        workspace.update(cx, |ws, cx| {
            let pane = ws
                .active_runtime_mut()
                .panes
                .iter_mut()
                .find(|pane| matches!(pane.content, PaneContent::Terminal(_)))
                .expect("a terminal pane");
            let pane_id = pane.id;
            let PaneContent::Terminal(terminal) = &mut pane.content else {
                return;
            };
            terminal.view = cx.new(|cx| {
                let session = daruda_terminal::TerminalSession::new(
                    daruda_terminal::TerminalDims::default(),
                    daruda_terminal::TerminalConfig::default(),
                )
                .unwrap();
                TerminalView::new_with_input(
                    session,
                    cx.focus_handle(),
                    TerminalInput::new(move |bytes| sink.lock().unwrap().extend_from_slice(bytes)),
                )
            });
            if agent_running {
                // What a routed SessionStart hook records for this pane.
                ws.claude_sessions.pty_bindings.insert(
                    pane_id,
                    daruda_agent::pty_tracker::PtyBinding {
                        terminal_id: None,
                        claude_pid: None,
                        session_id: "session".into(),
                    },
                );
            }
            cx.notify();
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window_handle.into(), |_, window, cx| {
        let view = workspace
            .read(cx)
            .active_runtime()
            .panes
            .iter()
            .find_map(|pane| pane.terminal_view().cloned())
            .unwrap();
        let focus = view.read(cx).focus_handle().clone();
        focus.focus(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.write_to_clipboard(clipboard);
    cx.update_window(window_handle.into(), |_, window, cx| {
        window.dispatch_action(Box::new(Paste), cx);
    })
    .unwrap();
    cx.run_until_parked();
    sent.lock().unwrap().clone()
}

fn image_only() -> gpui::ClipboardItem {
    gpui::ClipboardItem {
        entries: vec![gpui::ClipboardEntry::Image(gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            vec![0x89, b'P', b'N', b'G'],
        ))],
    }
}

#[cfg(not(windows))]
#[gpui::test]
async fn an_image_only_clipboard_sends_an_agent_tui_its_image_key(cx: &mut TestAppContext) {
    assert_eq!(
        paste_into_terminal(cx, image_only(), true),
        daruda_terminal::ansi::PASTE_IMAGE_KEY
    );
}

#[gpui::test]
async fn an_image_only_clipboard_sends_a_shell_nothing(cx: &mut TestAppContext) {
    assert!(paste_into_terminal(cx, image_only(), false).is_empty());
}

#[gpui::test]
async fn copied_files_paste_as_quoted_paths_not_their_names(cx: &mut TestAppContext) {
    // Finder puts the bare file name beside the file list.
    let files = gpui::ClipboardItem {
        entries: vec![
            gpui::ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
                vec![std::path::PathBuf::from("/tmp/shot one.png")].into(),
            )),
            gpui::ClipboardEntry::String(gpui::ClipboardString::new("shot one.png".into())),
        ],
    };
    assert_eq!(
        paste_into_terminal(cx, files, false),
        b"'/tmp/shot one.png'"
    );
}

#[gpui::test]
async fn text_pastes_unchanged(cx: &mut TestAppContext) {
    let text = gpui::ClipboardItem::new_string("echo hi".into());
    assert_eq!(paste_into_terminal(cx, text, true), b"echo hi");
}
