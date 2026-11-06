//! Pane-local permission requests; a settled one becomes [`record`]'s summary bar.

use daruda_acp::{PermissionChoice, PermissionItem, PermissionKindView};
use daruda_ui::theme::{self, PaneSurfaceTokens};
use gpui::{
    AnyElement, Context, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px,
    relative,
};

use crate::render::card_frame::{CardFrame, CardTone};
use crate::strings as s;
use crate::view::AgentChatView;

mod content;
mod record;
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
    if card.resolved.is_some() {
        return record::permission_record(ix, card, expanded, dim, window, cx);
    }
    let surface = PaneSurfaceTokens::agent_chat(cx).dimmed(dim);
    let root = CardFrame::new(CardTone::Standard)
        .render(dim, cx)
        .debug_selector(move || format!("agent-chat-permission-{ix}"))
        .flex()
        .flex_col()
        .text_size(px(theme::FONT_SIZE_LG))
        .line_height(relative(METRICS.line_height))
        .text_color(surface.foreground);
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

fn waiting_color(surface: &PaneSurfaceTokens) -> gpui::Hsla {
    if surface.syntax_is_light {
        theme::PANE_PERMISSION_WARNING_LIGHT
    } else {
        theme::PANE_PERMISSION_WARNING_DARK
    }
}

#[cfg(test)]
mod tests;
