//! Pane-local permission requests and their settled audit disclosure.

use daruda_acp::{PermissionChoice, PermissionItem, PermissionKindView, PermissionResolution};
use daruda_ui::theme::{self, PaneSurfaceTokens};
use gpui::{
    AnyElement, Context, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px,
    relative,
};

use crate::fold::FoldKey;
use crate::render::card_frame::{CardFrame, CardTone};
use crate::render::fold_header::{FoldHeader, FoldRow};
use crate::strings as s;
use crate::view::AgentChatView;

mod content;
use content::{permission_icon, request_details};

/// Pixel geometry for permission cards and their decision controls.
struct Metrics {
    card_pad_x: f32,
    content_gap: f32,
    summary_pad_x: f32,
    summary_min_height: f32,
    summary_radius: f32,
    actions_space: f32,
    action_height: f32,
    action_line_height: f32,
    record_pad_y: f32,
    line_height: f32,
    title_line_height: f32,
    icon_size: f32,
    copy_size: f32,
}

const METRICS: Metrics = Metrics {
    card_pad_x: 16.0,
    content_gap: 9.0,
    summary_pad_x: 9.0,
    summary_min_height: 40.0,
    summary_radius: 3.0,
    actions_space: 11.0,
    action_height: 27.0,
    action_line_height: 18.0,
    record_pad_y: 9.0,
    line_height: 1.55,
    title_line_height: 1.6,
    icon_size: 14.0,
    copy_size: 22.0,
};

pub(in crate::render) fn permission_card(
    ix: usize,
    card: &PermissionItem,
    expanded: bool,
    dim: f32,
    window: &mut Window,
    cx: &mut Context<AgentChatView>,
) -> AnyElement {
    let surface = PaneSurfaceTokens::agent_chat(cx).dimmed(dim);
    let font = px(theme::FONT_SIZE_LG);
    let root = CardFrame::new(CardTone::Standard)
        .render(dim, cx)
        .debug_selector(move || format!("agent-chat-permission-{ix}"))
        .flex()
        .flex_col()
        .text_size(font)
        .line_height(relative(METRICS.line_height))
        .text_color(surface.foreground);

    if card.resolved.is_some() {
        let (status, choice) = resolution_label(card);
        let (icon, color) = resolution_icon(card, &surface, dim);
        let header = FoldHeader::with_title(
            div()
                .flex()
                .items_center()
                .gap(px(theme::GAP_STANDARD))
                .child(permission_icon(
                    "agent-chat-permission-result-icon",
                    ix,
                    icon,
                    color,
                ))
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::MEDIUM)
                        .child(SharedString::from(status)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(surface.foreground_muted)
                        .font_family(theme::FONT_FAMILY_MONOSPACE)
                        .text_size(px(theme::FONT_SIZE_XS))
                        .child(SharedString::from(choice.map_or_else(
                            || request_title(card),
                            |choice| {
                                s::agent_chat::permission_record_description(
                                    request_title(card),
                                    choice,
                                )
                            },
                        ))),
                )
                .into_any_element(),
        );
        return root
            .px_0()
            .py(px(METRICS.record_pad_y))
            .border_x_0()
            .rounded_none()
            .bg(surface.background)
            .text_size(px(theme::FONT_SIZE_SM))
            .child(
                FoldRow::block(
                    SharedString::from(format!("agent-chat-permission-record-{ix}")),
                    FoldKey::Permission(ix),
                    expanded,
                    header,
                    move |cx| request_details(ix, card, &surface, window, cx).into_any_element(),
                )
                .chrome(|row| row.min_h(px(theme::FONT_SIZE_SM * METRICS.line_height)))
                .render(dim, cx),
            )
            .into_any_element();
    }

    let warning = theme::dim_toward_gray(waiting_color(&surface), dim);
    let mut root = root
        .px(px(METRICS.card_pad_x))
        .py(px(theme::PAD_XL))
        .child(
            div()
                .debug_selector(move || format!("agent-chat-permission-status-{ix}"))
                .flex()
                .items_center()
                .gap(px(theme::GAP_STANDARD))
                .mb(px(METRICS.content_gap))
                .text_size(px(theme::FONT_SIZE_SM))
                .font_weight(FontWeight::MEDIUM)
                .text_color(warning)
                .child(permission_icon(
                    "agent-chat-permission-status-icon",
                    ix,
                    daruda_ui::icons::SHIELD_ALERT,
                    warning,
                ))
                .child(SharedString::from(s::agent_chat::permission_waiting())),
        )
        .child(request_details(ix, card, &surface, window, cx));
    let primary = primary_choice(&card.options);
    let mut actions = div()
        .debug_selector(move || format!("agent-chat-permission-actions-{ix}"))
        .flex()
        .flex_wrap()
        .gap(px(theme::GAP_STANDARD))
        .mt(px(METRICS.actions_space))
        .pt(px(METRICS.actions_space))
        .border_t_1()
        .border_color(surface.border_tint);
    for (choice_ix, choice) in card.options.iter().enumerate() {
        actions = actions.child(permission_button(
            SharedString::from(format!("agent-chat-perm-{ix}-{choice_ix}")),
            card.id,
            choice,
            primary == Some(choice_ix),
            &surface,
            cx,
        ));
    }
    root = root.child(actions);
    root.into_any_element()
}

fn request_title(card: &PermissionItem) -> String {
    card.tool_title
        .clone()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(s::agent_chat::permission_title)
}

/// Only one explicit allow-once action earns the accent. Long-lived grants and
/// all refusals stay equally visible as bordered choices in the adapter's order.
fn primary_choice(choices: &[PermissionChoice]) -> Option<usize> {
    choices
        .iter()
        .position(|choice| choice.kind == PermissionKindView::AllowOnce)
}

fn permission_button(
    id: SharedString,
    request_id: u64,
    choice: &PermissionChoice,
    primary: bool,
    surface: &PaneSurfaceTokens,
    cx: &mut Context<AgentChatView>,
) -> impl IntoElement + use<> {
    let label: SharedString = choice.name.clone().into();
    let kind = choice.kind;
    let option_id = choice.option_id.clone();
    let selector = id.clone();
    let button = if primary {
        daruda_ui::button_primary(id, label)
    } else {
        daruda_ui::button_chip_on_surface(id, label, surface, cx)
    };
    button
        .debug_selector(move || selector.to_string())
        .h(px(METRICS.action_height))
        .px(px(theme::PAD_LG))
        .py(px(theme::BUTTON_STANDARD_PAD_Y))
        .text_size(px(theme::FONT_SIZE_SM))
        .line_height(px(METRICS.action_line_height))
        .on_click(cx.listener(move |this, _, _window, cx| {
            this.respond_permission(request_id, option_id.clone(), kind, cx);
        }))
}

fn resolution_label(card: &PermissionItem) -> (String, Option<String>) {
    match &card.resolved {
        Some(PermissionResolution::Chosen(id)) => {
            let choice = card.options.iter().find(|choice| &choice.option_id == id);
            let status = match choice.map(|choice| choice.kind) {
                Some(PermissionKindView::AllowOnce | PermissionKindView::AllowAlways) => {
                    s::agent_chat::permission_allowed()
                }
                Some(PermissionKindView::RejectOnce | PermissionKindView::RejectAlways) => {
                    s::agent_chat::permission_denied()
                }
                None => s::agent_chat::permission_resolved(),
            };
            (
                status,
                Some(choice.map_or_else(|| id.clone(), |choice| choice.name.clone())),
            )
        }
        Some(PermissionResolution::Cancelled) => (s::agent_chat::permission_cancelled(), None),
        None => (s::agent_chat::permission_waiting(), None),
    }
}

fn waiting_color(surface: &PaneSurfaceTokens) -> gpui::Hsla {
    if surface.syntax_is_light {
        theme::PANE_PERMISSION_WARNING_LIGHT
    } else {
        theme::PANE_PERMISSION_WARNING_DARK
    }
}

fn resolution_icon(
    card: &PermissionItem,
    surface: &PaneSurfaceTokens,
    dim: f32,
) -> (&'static str, gpui::Hsla) {
    use daruda_ui::icons;
    let choice = match &card.resolved {
        Some(PermissionResolution::Chosen(id)) => card.options.iter().find(|o| &o.option_id == id),
        _ => None,
    };
    match choice.map(|choice| choice.kind) {
        Some(PermissionKindView::AllowOnce | PermissionKindView::AllowAlways) => (
            icons::CHECK,
            theme::dim_toward_gray(
                if surface.syntax_is_light {
                    theme::AGENT_READING_LIGHT
                } else {
                    theme::AGENT_READING
                },
                dim,
            ),
        ),
        Some(PermissionKindView::RejectOnce | PermissionKindView::RejectAlways) => {
            (icons::CLOSE, surface.foreground_muted)
        }
        None if card.resolved == Some(PermissionResolution::Cancelled) => {
            (icons::MINIMIZE, surface.foreground_muted)
        }
        None => (icons::INFO, surface.foreground_muted),
    }
}

#[cfg(test)]
mod tests;
