//! Offline link fixtures for transcript and context-menu captures.
use crate::workspace::Workspace;
use daruda_store::observability::error_report::ErrorReport;
use gpui::{Context, Window};
use std::path::{Path, PathBuf};

fn write_fixture(root: &Path) -> std::io::Result<(PathBuf, String)> {
    std::fs::create_dir_all(root)?;
    let plan = root.join("implementation-plan.md");
    let design = root.join("design and scope.md");
    std::fs::write(
        &plan,
        "# Implementation plan\n\nRestore reliable agent chat links.\n",
    )?;
    std::fs::write(
        &design,
        "# Design and scope\n\nKeep file opening and path policy in the host.\n",
    )?;
    let preview = root.join("local preview.png");
    image::RgbaImage::from_fn(240, 120, |x, y| {
        if x < 80 {
            image::Rgba([32, 96, 160, 255])
        } else if y < 60 {
            image::Rgba([64, 160, 128, 255])
        } else {
            image::Rgba([224, 160, 64, 255])
        }
    })
    .save(&preview)
    .map_err(std::io::Error::other)?;
    let text = format!(
        "Saved the documents outside the repository.\n\n- [Implementation plan](<{}>)\n- [Design and scope](<{}>)\n\nBoth links open read-only reference files. Right-click a link to copy its path or reveal it in the file manager.\n\n![Local image preview](<{}>)",
        plan.display(),
        design.display(),
        preview.display(),
    );
    Ok((plan, text))
}

impl Workspace {
    pub(in crate::workspace) fn open_agent_chat_links_for_shot(
        &mut self,
        show_menu: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = daruda_store::persistence::default_data_dir().join("screenshot-links");
        let (path, text) = match write_fixture(&root) {
            Ok(fixture) => fixture,
            Err(error) => {
                self.report_error(
                    ErrorReport::new(crate::surface::strings::agent_chat::link_file_missing())
                        .from_error(&error)
                        .at(file!(), line!())
                        .build(),
                    cx,
                );
                return;
            }
        };
        let pane = self.open_agent_chat_pane_seeded(
            None,
            move |view, window, cx| {
                view.seed_transcript(
                    vec![
                        daruda_acp::ChatItem::AssistantText {
                            text,
                            streaming: false,
                            message_id: None,
                            phase: Default::default(),
                        },
                        daruda_acp::ChatItem::ToolCall(daruda_acp::ToolCallItem {
                            id: "missing-image".into(),
                            title: "View local and missing images".into(),
                            kind: daruda_acp::ToolKindView::Read,
                            tool_name: None,
                            status: daruda_acp::ToolStatusView::Completed,
                            diffs: Vec::new(),
                            output: vec![
                                daruda_acp::ToolOutputBlock::ResourceLink {
                                    uri: root
                                        .join("local preview.png")
                                        .to_string_lossy()
                                        .into_owned(),
                                    name: "local preview.png".into(),
                                    mime: Some("application/octet-stream".into()),
                                },
                                daruda_acp::ToolOutputBlock::ResourceLink {
                                    uri: root.join("missing.png").to_string_lossy().into_owned(),
                                    name: "missing.png".into(),
                                    mime: Some("image/png".into()),
                                },
                            ],
                            raw_input: None,
                            locations: Vec::new(),
                            parent_tool_id: None,
                            exit: None,
                        }),
                    ],
                    window,
                    cx,
                );
                view.set_fold_mode(
                    daruda_transcript::fold_mode::FoldMode::from_tokens([
                        "expanded",
                        "past.tool=expanded",
                        "last.tool=expanded",
                    ]),
                    window,
                    cx,
                );
            },
            window,
            cx,
        );
        if show_menu && let Some(pane) = pane {
            let viewport = window.viewport_size();
            let position = gpui::point(viewport.width / 2., viewport.height / 2.);
            self.open_link_menu_for_shot(pane, &path.to_string_lossy(), position, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_has_two_existing_outside_repository_documents() {
        let root = tempfile::tempdir().unwrap();
        let (path, text) = write_fixture(root.path()).unwrap();
        assert!(path.is_file());
        assert!(root.path().join("design and scope.md").is_file());
        assert!(text.contains("[Implementation plan](<"));
        assert!(text.contains("[Design and scope](<"));
    }
}
