use gpui::{AnyElement, ClickEvent, Context, IntoElement, Render, Window, div, prelude::*, px};

use super::summary::{Against, TargetRead};
use super::{CreatePrModal, CreateState};
use crate::surface::strings::{common, git as s};
use crate::ui::Disableable as _;
use crate::ui::theme;
use crate::ui::{
    Selectable as _, button, button_compact, button_primary, checkbox, form_input, markdown_editor,
};

impl CreatePrModal {
    /// Where it goes, what it takes, and what it leaves out.
    fn summary_lines(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = theme::current(cx);
        let line = |text: String, color| {
            div()
                .text_size(px(theme::MODAL_BODY_FONT_SIZE))
                .text_color(color)
                .child(text)
        };
        let base = self.base_value(cx);
        let mut lines = div().flex().flex_col().gap(px(theme::GAP_XS));
        let Some(read) = &self.publication else {
            return lines.child(line(common::loading(), t.text_muted));
        };
        lines = lines.child(match &read.target {
            TargetRead::Pushes {
                repo: Some(repo),
                shown,
            } => line(
                match &base {
                    Some(base) => s::pr_summary_target(repo.qualified(), &shown.branch, base),
                    None => s::pr_summary_target_default(repo.qualified(), &shown.branch),
                },
                t.text_body,
            ),
            TargetRead::Pushes { repo: None, shown } => line(
                s::pr_summary_not_github(&shown.remote, &shown.url),
                t.banner_warning_text,
            ),
            TargetRead::NoTarget => line(s::pr_summary_no_target(), t.banner_warning_text),
            TargetRead::Unreadable => line(s::pr_summary_target_unreadable(), t.text_muted),
        });
        lines = lines.child(match self.current_against(cx) {
            None => line(common::loading(), t.text_muted),
            Some(Against::Counted {
                label,
                commits,
                files,
            }) => line(s::pr_summary_counts(label, *commits, *files), t.text_muted),
            Some(Against::DefaultBase) => line(s::pr_summary_default_base(), t.text_muted),
            Some(Against::BaseNotFound(name)) => {
                line(s::pr_summary_base_not_found(name), t.banner_warning_text)
            }
            Some(Against::OnBase) => line(s::pr_summary_on_base(), t.banner_warning_text),
            Some(Against::Unknown) => line(s::pr_summary_unknown(), t.text_muted),
        });
        if read.uncommitted > 0 {
            lines = lines.child(line(
                s::pr_summary_uncommitted(read.uncommitted),
                t.banner_warning_text,
            ));
        }
        lines
    }
}

impl CreatePrModal {
    /// Why what is typed is not kept across a close or a restart, when it
    /// is not.
    fn keeping_line(&self, cx: &Context<Self>) -> Option<AnyElement> {
        use crate::github::DraftKeeping;
        use crate::surface::strings::pull_requests as pr;
        // Only once the target is read: before, nothing is known yet.
        self.publication.as_ref()?;
        let text = match self.creation_keeping(cx) {
            DraftKeeping::Saved => return None,
            DraftKeeping::MemoryOnly => pr::drafts_memory_only(),
            DraftKeeping::SaveFailed => pr::drafts_save_failing(),
            DraftKeeping::NoAccount => pr::drafts_no_account(),
        };
        Some(
            div()
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(theme::current(cx).banner_warning_text)
                .child(text)
                .into_any_element(),
        )
    }

    /// Which template the description came from, and the others to use
    /// instead while the user has not written one.
    fn template_line(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let t = theme::current(cx);
        if self.templates.is_empty() {
            return Some(
                div()
                    .text_size(px(theme::FONT_SIZE_SM))
                    .text_color(t.text_muted)
                    .child(s::pr_template_absent())
                    .into_any_element(),
            );
        }
        let kept = self.keeps_description(cx);
        let note = match (&self.template, kept) {
            (_, true) => s::pr_template_kept(),
            (Some(path), false) => s::pr_template_from(path),
            (None, false) => s::pr_template_none(),
        };
        let base_name = |path: &str| path.rsplit('/').next().unwrap_or(path).to_owned();
        let choices = self.templates.all().enumerate().map(|(ix, template)| {
            let path = template.path.clone();
            // The file name, unless another template shares it.
            let shared = self
                .templates
                .all()
                .filter(|other| base_name(&other.path) == base_name(&path))
                .count()
                > 1;
            let name = if shared {
                path.clone()
            } else {
                base_name(&path)
            };
            button_compact(("create-pr-template", ix), name)
                .debug_selector(move || format!("create-pr-template-{ix}"))
                .disabled(kept)
                .selected(self.template.as_deref() == Some(path.as_str()))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.use_template(&path, window, cx)
                }))
        });
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(theme::GAP_SM))
                .child(
                    div()
                        .text_size(px(theme::FONT_SIZE_SM))
                        .text_color(t.text_muted)
                        .child(note),
                )
                .children(choices)
                .into_any_element(),
        )
    }

    /// Branches of the push remote, as git last fetched them, matching what
    /// is typed: the base searched.
    fn base_line(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let suggestions = self.base_suggestions(cx);
        if suggestions.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_wrap()
                .gap(px(theme::GAP_SM))
                .children(suggestions.into_iter().enumerate().map(|(ix, branch)| {
                    let chosen = branch.clone();
                    button_compact(("create-pr-base", ix), branch)
                        .debug_selector(move || format!("create-pr-base-{ix}"))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.choose_base(chosen.clone(), window, cx)
                        }))
                }))
                .into_any_element(),
        )
    }
}

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
            .child(self.summary_lines(cx))
            .children(self.keeping_line(cx))
            .children(self.restored.then(|| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme::GAP_SM))
                    .child(
                        div()
                            .text_size(px(theme::FONT_SIZE_SM))
                            .text_color(t.text_muted)
                            .child(s::pr_draft_restored()),
                    )
                    .child(
                        button_compact("create-pr-discard", s::pr_draft_discard())
                            .debug_selector(|| "create-pr-discard".into())
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.discard_creation(window, cx)
                            })),
                    )
            }))
            .child(label(s::pr_field_title()))
            .child(form_input(&self.title, cx, 0))
            .child(label(s::pr_field_body()))
            .children(self.template_line(cx))
            .child(markdown_editor(&self.body, cx).tab_index(1))
            .child(label(s::pr_field_base()))
            .child(form_input(&self.base, cx, 2))
            .children(self.base_line(cx))
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
                        .disabled(
                            creating
                                || self.publication.is_none()
                                || self.title.read(cx).value().trim().is_empty(),
                        )
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
        match &self.state {
            CreateState::Failed(message) => {
                panel = panel.child(crate::ui::alert::error("create-pr-error", message.clone()));
            }
            CreateState::Exists(created) => {
                let label = match created.number {
                    Some(number) => s::pr_open_created(s::pr_number(number)),
                    None => s::pr_open_on_github(),
                };
                panel = panel
                    .child(crate::ui::alert::warning(
                        "create-pr-exists",
                        crate::surface::strings::error::github_pr_exists(),
                    ))
                    .child(
                        div().flex().justify_end().child(
                            button_primary("create-pr-open-existing", label)
                                .debug_selector(|| "create-pr-open-existing".into())
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.open_existing(window, cx)
                                })),
                        ),
                    );
            }
            CreateState::Editing | CreateState::Creating => {}
        }
        panel.child(footer)
    }
}
