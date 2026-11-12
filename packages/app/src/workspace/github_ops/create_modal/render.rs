use gpui::{ClickEvent, Context, IntoElement, Render, Window, div, prelude::*, px};

use super::{CreatePrModal, CreateState};
use crate::surface::strings::{common, git as s};
use crate::ui::Disableable as _;
use crate::ui::theme;
use crate::ui::{button, button_primary, checkbox, form_input, markdown_editor};

impl Render for CreatePrModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::current(cx).clone();
        let label = |text: String| {
            div()
                .text_size(px(theme::MODAL_BODY_FONT_SIZE))
                .text_color(t.text_muted)
                .child(text)
        };
        let creating = self.state == CreateState::Creating;
        let submit_label = if creating {
            s::pr_creating()
        } else {
            s::pr_create_submit()
        };
        let body = div()
            .flex()
            .flex_col()
            .gap(px(theme::MODAL_PANEL_GAP))
            .child(label(s::pr_field_title()))
            .child(form_input(&self.title, cx, 0))
            .child(label(s::pr_field_body()))
            .child(markdown_editor(&self.body, cx).tab_index(1))
            .child(label(s::pr_field_base()))
            .child(form_input(&self.base, cx, 2))
            .child(
                checkbox("create-pr-draft", s::pr_draft(), 3_isize)
                    .checked(self.draft)
                    .on_click(cx.listener(|this, _: &bool, _window, cx| this.toggle_draft(cx))),
            );
        let footer =
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap(px(theme::MODAL_FOOTER_GAP))
                .mt(px(theme::MODAL_FOOTER_MARGIN_TOP))
                .child(button("create-pr-cancel", common::btn_cancel()).on_click(
                    cx.listener(|this, _: &ClickEvent, window, cx| this.dismiss(window, cx)),
                ))
                .child(
                    button_primary("create-pr-submit", submit_label)
                        .disabled(creating || self.title.read(cx).value().trim().is_empty())
                        .on_click(
                            cx.listener(|this, _: &ClickEvent, window, cx| this.submit(window, cx)),
                        ),
                );
        let mut panel = div()
            .flex()
            .flex_col()
            .key_context("CreatePrModal")
            .track_focus(&self.panel_focus)
            .tab_group()
            .gap(px(theme::MODAL_PANEL_GAP))
            .child(body);
        if let CreateState::Failed(message) = &self.state {
            panel = panel.child(crate::ui::alert::error("create-pr-error", message.clone()));
        }
        panel.child(footer)
    }
}
