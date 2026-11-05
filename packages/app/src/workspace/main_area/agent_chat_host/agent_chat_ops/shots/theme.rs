//! Offline mixed-theme fixture for transcript surfaces and permission cards.
use crate::workspace::Workspace;
use daruda_acp::{
    ChatItem, PermissionChoice, PermissionItem, PermissionKindView, PermissionResolution,
};
use gpui::{Context, Window};

const MARKDOWN: &str = "## Surface consistency\n\n| Element | Expected appearance |\n| --- | --- |\n| **Table body** | Pane-derived background |\n| [Open reference](https://example.invalid) | Readable link |\n\n- [x] Checked task\n- [ ] Pending task\n\n```rust\nlet answer = 42;\n```";

fn transcript() -> Vec<ChatItem> {
    let permission = |id, resolved| {
        ChatItem::Permission(PermissionItem {
            id,
            tool_call_id: format!("theme-permission-{id}"),
            tool_title: Some("Update the reference document".into()),
            raw_input_summary: None,
            options: vec![
                PermissionChoice {
                    option_id: "allow_once".into(),
                    name: "Allow once".into(),
                    kind: PermissionKindView::AllowOnce,
                },
                PermissionChoice {
                    option_id: "reject_once".into(),
                    name: "Reject".into(),
                    kind: PermissionKindView::RejectOnce,
                },
            ],
            resolved,
        })
    };
    vec![
        ChatItem::AssistantText {
            text: MARKDOWN.into(),
            streaming: false,
            message_id: None,
            phase: Default::default(),
        },
        permission(1, Some(PermissionResolution::Chosen("allow_once".into()))),
        permission(2, Some(PermissionResolution::Cancelled)),
        permission(3, None),
    ]
}

impl Workspace {
    pub(in crate::workspace) fn open_agent_chat_theme_for_shot(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_agent_chat_pane_seeded(
            None,
            |view, window, cx| {
                view.seed_transcript(transcript(), window, cx);
            },
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_covers_content_and_both_permission_results() {
        let items = transcript();
        assert!(items.iter().any(|item| matches!(item,
            ChatItem::Permission(p) if p.resolved.is_none()
                && p.options.iter().any(|o| o.kind == daruda_acp::PermissionKindView::AllowOnce)
                && p.options.iter().any(|o| o.kind == daruda_acp::PermissionKindView::RejectOnce)
        )), "the fixture must expose pending Allow and Reject actions");
        assert_eq!(items.len(), 4);
        assert!(MARKDOWN.contains("| --- | --- |"));
        assert!(
            matches!(&items[1], ChatItem::Permission(p) if matches!(p.resolved, Some(PermissionResolution::Chosen(_))))
        );
        assert!(
            matches!(&items[2], ChatItem::Permission(p) if p.resolved == Some(PermissionResolution::Cancelled))
        );
    }
}
