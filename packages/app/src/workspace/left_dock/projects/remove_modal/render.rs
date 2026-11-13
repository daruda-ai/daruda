use super::*;
use crate::ui::{Disableable as _, button, button_danger, checkbox, theme};
use gpui::{ClickEvent, IntoElement, Render, div, prelude::*, px};

impl Render for RemoveWorktreeModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::current(cx);
        let muted_text = t.text_muted;
        let faint_text = t.text_subtle;
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(theme::MODAL_PANEL_GAP))
            .child(
                div()
                    .text_size(px(theme::MODAL_BODY_FONT_SIZE))
                    .text_color(muted_text)
                    .child(match self.stage {
                        Stage::Inspecting => s::modal::remove_lane_checking(),
                        Stage::InspectionFailed => s::modal::remove_lane_check_body(),
                        Stage::Branch => s::modal::remove_lane_branch_retry_body(),
                        Stage::Ready(LaneRemovalState::Missing) => {
                            s::modal::remove_lane_missing_body()
                        }
                        Stage::Ready(LaneRemovalState::Unregistered) => {
                            s::modal::remove_lane_unregistered_body()
                        }
                        Stage::Ready(LaneRemovalState::Registered) => {
                            s::modal::remove_lane_body(&self.target_label)
                        }
                    }),
            )
            .child(
                div()
                    .text_size(px(theme::LANE_SUB_FONT_SIZE))
                    .text_color(faint_text)
                    .child(self.target_path.clone()),
            );

        // Optional checkbox row: "Also delete branch '<name>'".
        // Only shown when the lane actually has a branch
        // (Default kind / detached HEAD have nothing to delete).
        if self.stage == Stage::Ready(LaneRemovalState::Unregistered) {
            body = body.child(
                checkbox("remove-wt-folder", s::modal::remove_lane_delete_folder(), 0)
                    .checked(self.delete_folder)
                    .disabled(self.submitting)
                    .on_click(cx.listener(|this, _: &bool, _, cx| this.toggle_delete_folder(cx))),
            );
            if self.delete_folder {
                body = body.child(crate::ui::alert::warning(
                    "remove-lane-folder-warning",
                    s::modal::remove_lane_delete_folder_warning(),
                ));
            }
        }

        if let Some(branch) = self.branch.clone().filter(|_| {
            matches!(self.stage, Stage::Ready(_))
                && (self.stage != Stage::Ready(LaneRemovalState::Unregistered)
                    || self.delete_folder)
        }) {
            let label = SharedString::from(s::modal::remove_lane_also_delete_branch(&branch));
            body = body.child(
                checkbox("remove-wt-also-delete-branch", label, 1)
                    .checked(self.delete_branch_too)
                    .disabled(self.submitting)
                    .on_click(cx.listener(|this, _: &bool, _, cx| {
                        this.toggle_delete_branch(cx);
                    })),
            );
        }

        // Error block: alert banner + optional --force hint, stacked
        // so the dialog body shows them as a single element.
        let error_block = self.error.as_ref().map(|msg| {
            let mut stack = div()
                .flex()
                .flex_col()
                .gap(px(theme::MODAL_PANEL_GAP))
                .child(crate::ui::alert::error("remove-lane-error", msg.clone()));
            if self.allow_force {
                stack = stack.child(
                    div()
                        .text_size(px(theme::LANE_SUB_FONT_SIZE))
                        .text_color(faint_text)
                        .child(s::modal::remove_lane_force_hint()),
                );
            }
            stack
        });

        let confirm_label = if self.submitting {
            s::modal::remove_lane_removing()
        } else if self.stage == Stage::Inspecting {
            s::modal::remove_lane_checking()
        } else if self.stage == Stage::InspectionFailed {
            s::modal::remove_lane_retry_check()
        } else if self.stage == Stage::Branch {
            s::modal::remove_lane_retry_branch()
        } else if self.stage == Stage::Ready(LaneRemovalState::Unregistered) {
            if self.delete_folder {
                s::modal::remove_lane_delete_folder_confirm()
            } else {
                s::modal::remove_lane_forget()
            }
        } else if self.stage == Stage::Ready(LaneRemovalState::Missing) {
            s::modal::remove_lane_forget()
        } else if self.allow_force {
            s::modal::remove_lane_force()
        } else {
            s::modal::remove_lane_confirm()
        };

        let footer = div()
            .flex()
            .flex_row()
            .justify_end()
            .gap(px(theme::MODAL_FOOTER_GAP))
            .mt(px(theme::MODAL_FOOTER_MARGIN_TOP))
            .child(
                button("remove-wt-cancel", s::common::btn_cancel())
                    .disabled(self.submitting)
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.dismiss(window, cx);
                    })),
            )
            .child(
                button_danger("remove-wt-confirm", confirm_label)
                    .disabled(self.submitting || self.stage == Stage::Inspecting)
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.submit(window, cx);
                    })),
            );

        let mut panel = div()
            .tab_group()
            .flex()
            .flex_col()
            .key_context("RemoveWorktreeModal")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                this.handle_key(ev, window, cx);
            }))
            .gap(px(theme::MODAL_PANEL_GAP))
            .child(body);
        if let Some(b) = error_block {
            panel = panel.child(b);
        }
        panel.child(footer)
    }
}
