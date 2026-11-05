use super::*;

fn request(kind: PermissionKindView) -> PermissionItem {
    PermissionItem {
        id: 1,
        tool_call_id: "call".into(),
        tool_title: Some("Write file".into()),
        raw_input_summary: Some("file: /tmp/example.rs".into()),
        options: vec![PermissionChoice {
            option_id: "chosen".into(),
            name: "Adapter label".into(),
            kind,
        }],
        resolved: Some(PermissionResolution::Chosen("chosen".into())),
    }
}

#[test]
fn permission_result_uses_choice_kind_and_preserves_adapter_label() {
    for (kind, status) in [
        (
            PermissionKindView::AllowOnce,
            s::agent_chat::permission_allowed(),
        ),
        (
            PermissionKindView::AllowAlways,
            s::agent_chat::permission_allowed(),
        ),
        (
            PermissionKindView::RejectOnce,
            s::agent_chat::permission_denied(),
        ),
        (
            PermissionKindView::RejectAlways,
            s::agent_chat::permission_denied(),
        ),
    ] {
        let (actual, choice) = resolution_label(&request(kind));
        assert_eq!(actual, status);
        assert_eq!(choice.as_deref(), Some("Adapter label"));
    }
}

#[test]
fn permission_unknown_choice_is_never_reported_as_allowed() {
    let mut card = request(PermissionKindView::AllowOnce);
    card.resolved = Some(PermissionResolution::Chosen("missing".into()));
    assert_eq!(
        resolution_label(&card),
        (s::agent_chat::permission_resolved(), Some("missing".into()))
    );
    card.resolved = Some(PermissionResolution::Cancelled);
    assert_eq!(
        resolution_label(&card),
        (s::agent_chat::permission_cancelled(), None)
    );
}

#[test]
fn permission_primary_never_promotes_a_persistent_grant_or_rejection() {
    let mut choices = vec![
        request(PermissionKindView::RejectOnce).options.remove(0),
        request(PermissionKindView::AllowAlways).options.remove(0),
    ];
    assert_eq!(primary_choice(&choices), None);
    choices.push(request(PermissionKindView::AllowOnce).options.remove(0));
    choices.push(request(PermissionKindView::AllowOnce).options.remove(0));
    assert_eq!(primary_choice(&choices), Some(2));
}

#[gpui::test]
fn permission_disclosure_defaults_closed_and_remeasures_its_own_row(cx: &mut gpui::TestAppContext) {
    use crate::fold::{FoldContext, FoldState};
    let key = FoldKey::Permission(0);
    for preset in daruda_transcript::fold_mode::FoldPreset::ALL {
        let state = FoldState::with_mode(preset.mode());
        assert!(!state.is_expanded(&key, FoldContext::last(false)));
    }
    let window = crate::view::tests::make_test_view(cx);
    window
        .update(cx, |view, window, cx| {
            view.items = vec![daruda_acp::ChatItem::Permission(request(
                PermissionKindView::RejectOnce,
            ))];
            assert!(crate::rows::collect_foldable_keys(&view.items).contains(&key));
            assert_eq!(
                crate::agent_chat_helpers::fold_key_item_index(&key, &view.items),
                Some(0)
            );
            view.toggle_fold(key.clone(), window, cx);
            assert!(view.fold.is_expanded(&key, FoldContext::last(false)));
            view.toggle_fold(key.clone(), window, cx);
            assert!(!view.fold.is_expanded(&key, FoldContext::last(false)));
        })
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let collapsed = visual
        .debug_bounds("agent-chat-permission-0")
        .expect("settled card visible");
    visual.simulate_click(collapsed.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    let expanded = visual.debug_bounds("agent-chat-permission-0").unwrap();
    assert!(
        expanded.size.height > collapsed.size.height,
        "opening must remeasure the permission row"
    );
    visual.simulate_click(
        gpui::point(collapsed.center().x, collapsed.center().y),
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    assert_eq!(
        visual
            .debug_bounds("agent-chat-permission-0")
            .unwrap()
            .size
            .height,
        collapsed.size.height
    );
}

#[gpui::test]
fn permission_pending_matches_mockup_padding(cx: &mut gpui::TestAppContext) {
    let window = crate::view::tests::make_test_view(cx);
    window
        .update(cx, |view, window, cx| {
            let mut card = request(PermissionKindView::AllowOnce);
            card.resolved = None;
            card.options = [
                PermissionKindView::AllowOnce,
                PermissionKindView::AllowAlways,
                PermissionKindView::RejectOnce,
            ]
            .into_iter()
            .enumerate()
            .map(|(ix, kind)| PermissionChoice {
                option_id: ix.to_string(),
                name: format!("Choice {ix}"),
                kind,
            })
            .collect();
            view.items = vec![daruda_acp::ChatItem::Permission(card)];
            view.toggle_fold(FoldKey::Permission(0), window, cx);
        })
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let card = visual.debug_bounds("agent-chat-permission-0").unwrap();
    let title = visual
        .debug_bounds("agent-chat-permission-title-0")
        .expect("request title laid out");
    assert_eq!(
        title.origin.x - card.origin.x,
        px(17.),
        "16px horizontal padding plus the border, as in approved A"
    );
    let status = visual
        .debug_bounds("agent-chat-permission-status-0")
        .unwrap();
    let input = visual
        .debug_bounds("agent-chat-permission-input-0")
        .unwrap();
    let actions = visual
        .debug_bounds("agent-chat-permission-actions-0")
        .unwrap();
    let status_icon = visual
        .debug_bounds("agent-chat-permission-status-icon-0")
        .expect("pending shield icon");
    assert_eq!(status_icon.size, gpui::size(px(14.), px(14.)));
    let input_icon = visual
        .debug_bounds("agent-chat-permission-input-icon-0")
        .expect("input type icon");
    assert_eq!(input_icon.size, gpui::size(px(14.), px(14.)));
    let copy = visual
        .debug_bounds("agent-chat-permission-copy-0")
        .expect("summary copy action");
    assert_eq!(copy.size, gpui::size(px(22.), px(22.)));
    assert_eq!(status.origin.y - card.origin.y, px(15.));
    assert_eq!(title.origin.y - status.bottom(), px(9.));
    assert_eq!(input.origin.y - title.bottom(), px(9.));
    assert_eq!(input.size.height, px(40.));
    assert_eq!(actions.origin.y - input.bottom(), px(11.));
    let mut previous_right = None;
    for selector in [
        "agent-chat-perm-0-0",
        "agent-chat-perm-0-1",
        "agent-chat-perm-0-2",
    ] {
        let button = visual.debug_bounds(selector).unwrap();
        assert_eq!(button.size.height, px(27.));
        assert_eq!(button.origin.y - actions.origin.y, px(12.));
        assert_eq!(card.bottom() - button.bottom(), px(15.));
        if let Some(right) = previous_right {
            assert_eq!(button.origin.x - right, px(6.));
        }
        previous_right = Some(button.right());
    }
    visual.simulate_click(copy.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    let copied = visual.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some("file: /tmp/example.rs"));
    assert!(window.read_with(&visual, |view, _| matches!(&view.items[0], daruda_acp::ChatItem::Permission(card) if card.resolved.is_none())).unwrap());
}

#[gpui::test]
fn waiting_label_keeps_amber_and_readable_contrast_across_pane_and_ui_themes(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        theme::init_if_missing(cx);
        for ui in ["daruda_light", "daruda_dark"] {
            assert!(theme::apply_ui_theme(ui, cx));
            for preset in daruda_config::theme_presets::PRESETS {
                let Some(colors) = daruda_config::theme_presets::colors_for_preset(preset.name)
                else {
                    continue;
                };
                let bg = colors.background;
                theme::set_agent_chat_bg(cx, bg.r, bg.g, bg.b);
                let surface = PaneSurfaceTokens::agent_chat(cx);
                let color = waiting_color(&surface);
                assert_eq!(color.h, theme::WARNING.h);
                assert_ne!(color, surface.foreground_muted);
                let ratio = theme::contrast_ratio(color, surface.background.blend(surface.tint));
                assert!(ratio >= 4.5, "{ui} / {}: {ratio:.2}:1", preset.name);
            }
        }
    });
}

#[test]
fn settled_request_icons_distinguish_allow_reject_cancel_and_unknown() {
    use daruda_ui::icons;
    let surface =
        PaneSurfaceTokens::from_background_and_foreground(theme::BG_EDITOR, theme::TEXT_BODY);
    for (kind, icon) in [
        (PermissionKindView::AllowOnce, icons::CHECK),
        (PermissionKindView::AllowAlways, icons::CHECK),
        (PermissionKindView::RejectOnce, icons::CLOSE),
        (PermissionKindView::RejectAlways, icons::CLOSE),
    ] {
        assert_eq!(resolution_icon(&request(kind), &surface, 0.0).0, icon);
    }
    let mut card = request(PermissionKindView::AllowOnce);
    card.resolved = Some(PermissionResolution::Cancelled);
    assert_eq!(resolution_icon(&card, &surface, 0.0).0, icons::MINIMIZE);
    card.resolved = Some(PermissionResolution::Chosen("unknown".into()));
    assert_eq!(resolution_icon(&card, &surface, 0.0).0, icons::INFO);
}
