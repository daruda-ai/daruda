//! Clipboard contents pasted into panes. A terminal paste is decided here,
//! once per clipboard read: copied files, text, or an agent TUI's image key.
//! The composer reuses [`clipboard_paths`] for its attachments.

use daruda_terminal::view::TerminalView;
use gpui::{ClipboardEntry, ClipboardItem, Context, Entity};
use std::path::PathBuf;

use super::pane_tree::PaneId;
use crate::workspace::Workspace;

/// The files a clipboard snapshot names, as Finder's file copy carries them.
pub(in crate::workspace) fn clipboard_paths(item: &ClipboardItem) -> Vec<PathBuf> {
    item.entries()
        .iter()
        .filter_map(|entry| match entry {
            ClipboardEntry::ExternalPaths(paths) => Some(paths.paths()),
            _ => None,
        })
        .flatten()
        .cloned()
        .collect()
}

/// What one clipboard snapshot pastes into a terminal pane.
#[derive(Debug, PartialEq, Eq)]
enum TerminalPaste {
    /// Copied files go in as shell-quoted paths, not the bare file names
    /// macOS puts beside them.
    Paths(Vec<PathBuf>),
    Text(String),
    /// An image with no text form, for an agent TUI that reads the OS
    /// clipboard itself. A shell gets nothing: it would read the key as
    /// quoted-insert.
    AttachImage,
    Nothing,
}

impl TerminalPaste {
    fn of(item: &ClipboardItem, agent_attaches_images: bool) -> Self {
        let paths = clipboard_paths(item);
        if !paths.is_empty() {
            return Self::Paths(paths);
        }
        if let Some(text) = item.text() {
            return Self::Text(text);
        }
        let has_image = item
            .entries()
            .iter()
            .any(|entry| matches!(entry, ClipboardEntry::Image(_)));
        if has_image && agent_attaches_images {
            Self::AttachImage
        } else {
            Self::Nothing
        }
    }
}

impl Workspace {
    /// The one decision for a Paste into a terminal pane, so the clipboard is
    /// read once. The terminal's own Paste is the text-only fallback for
    /// hosts that do not install this.
    pub(in crate::workspace) fn paste_clipboard_to_terminal(
        &mut self,
        pane_id: PaneId,
        view: &Entity<TerminalView>,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        // Windows TUIs bind image paste to Alt+V, and Ctrl+V is the paste key.
        let agent_attaches_images =
            !cfg!(windows) && self.claude_sessions.pty_bindings.contains_key(&pane_id);
        match TerminalPaste::of(&item, agent_attaches_images) {
            TerminalPaste::Paths(paths) => {
                let text = self.shell_quoted_paths(&paths);
                view.update(cx, |view, cx| view.paste_literal(&text, cx));
            }
            TerminalPaste::Text(text) => {
                view.update(cx, |view, cx| view.paste_literal(&text, cx));
            }
            TerminalPaste::AttachImage => {
                view.update(cx, |view, cx| view.send_paste_image_key(cx));
            }
            TerminalPaste::Nothing => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_paths_collect_every_copied_file_and_ignore_text() {
        let item = ClipboardItem {
            entries: vec![
                ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
                    vec![PathBuf::from("/a b.png"), PathBuf::from("/c.txt")].into(),
                )),
                ClipboardEntry::String(gpui::ClipboardString::new("a b.png".into())),
            ],
        };
        assert_eq!(
            clipboard_paths(&item),
            [PathBuf::from("/a b.png"), PathBuf::from("/c.txt")]
        );
        assert!(clipboard_paths(&ClipboardItem::new_string("x".into())).is_empty());
    }

    fn image() -> ClipboardEntry {
        ClipboardEntry::Image(gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            vec![0x89, b'P', b'N', b'G'],
        ))
    }

    #[test]
    fn an_image_only_clipboard_reaches_only_an_agent_tui() {
        let item = ClipboardItem {
            entries: vec![image()],
        };
        assert_eq!(TerminalPaste::of(&item, true), TerminalPaste::AttachImage);
        assert_eq!(TerminalPaste::of(&item, false), TerminalPaste::Nothing);
    }

    #[test]
    fn copied_files_outrank_their_names_and_text_outranks_an_image() {
        let files = ClipboardItem {
            entries: vec![
                ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
                    vec![PathBuf::from("/a.png")].into(),
                )),
                ClipboardEntry::String(gpui::ClipboardString::new("a.png".into())),
            ],
        };
        assert_eq!(
            TerminalPaste::of(&files, true),
            TerminalPaste::Paths(vec![PathBuf::from("/a.png")])
        );
        let text = ClipboardItem {
            entries: vec![
                image(),
                ClipboardEntry::String(gpui::ClipboardString::new("ls".into())),
            ],
        };
        assert_eq!(
            TerminalPaste::of(&text, true),
            TerminalPaste::Text("ls".into())
        );
        assert_eq!(
            TerminalPaste::of(&ClipboardItem::new_string(String::new()), true),
            TerminalPaste::Nothing
        );
    }
}
