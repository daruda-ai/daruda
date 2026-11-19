//! The Create PR form's lines: each a leaf of what the form shows, built
//! from the form's state for `render`.

use gpui::{AnyElement, ClickEvent, Context, IntoElement, div, prelude::*, px};

use super::super::draft::{DraftOffer, DraftState};
use super::super::summary::{Against, TargetRead};
use super::super::{CreatePrModal, CreateState};
use crate::surface::strings::{common, git as s};
use crate::ui::Disableable as _;
use crate::ui::theme;
use crate::ui::{Selectable as _, button_compact, button_primary};

impl CreatePrModal {
    /// Where it goes, what it takes, and what it leaves out.
    pub(super) fn summary_lines(&self, cx: &Context<Self>) -> impl IntoElement {
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

    /// Why what is typed is not kept across a close or a restart, when it
    /// is not.
    pub(super) fn keeping_line(&self, cx: &Context<Self>) -> Option<AnyElement> {
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

    /// The Write button, and beside it why it cannot be pressed, that the
    /// agent is writing, what went wrong, or the way back to the old text.
    pub(super) fn draft_line(&self, cx: &Context<Self>) -> AnyElement {
        let t = theme::current(cx);
        let note = |text: String, color| {
            div()
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(color)
                .child(text)
        };
        let offer = self.draft_offer(cx);
        let sent = self.is_sent();
        let mut line = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(theme::GAP_SM));
        line = match &self.written {
            DraftState::Running { .. } => line
                .child(
                    button_compact("create-pr-write", s::pr_writing())
                        .debug_selector(|| "create-pr-write".into())
                        .disabled(true),
                )
                .child(
                    button_compact("create-pr-write-stop", s::pr_write_stop())
                        .debug_selector(|| "create-pr-write-stop".into())
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.stop_draft(cx))),
                ),
            DraftState::Idle | DraftState::Failed(_) | DraftState::Filled(_) => {
                line.child(
                    button_compact("create-pr-write", s::pr_write())
                        .debug_selector(|| "create-pr-write".into())
                        .disabled(sent || offer != DraftOffer::Ready)
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.write_draft(window, cx)
                        })),
                )
            }
        };
        line = match (&self.written, offer) {
            (DraftState::Running { .. }, _) => line,
            (DraftState::Failed(reason), _) => {
                line.child(note(reason.to_string(), t.banner_warning_text))
            }
            (DraftState::Filled(_), _) => {
                line.child(note(s::pr_write_filled(), t.text_muted)).child(
                    button_compact("create-pr-write-undo", s::pr_write_undo())
                        .debug_selector(|| "create-pr-write-undo".into())
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.undo_draft(window, cx)
                        })),
                )
            }
            (DraftState::Idle, DraftOffer::Ready) => line,
            (DraftState::Idle, DraftOffer::NoAgent) => {
                line.child(note(s::pr_write_no_agent(), t.text_muted))
            }
            (DraftState::Idle, DraftOffer::NoBase) => {
                line.child(note(s::pr_write_no_base(), t.text_muted))
            }
            (DraftState::Idle, DraftOffer::NothingOver) => {
                line.child(note(s::pr_write_nothing_over(), t.text_muted))
            }
        };
        line.into_any_element()
    }

    /// Which template the description came from, and the others to use
    /// instead while the user has not written one.
    pub(super) fn template_line(&self, cx: &Context<Self>) -> Option<AnyElement> {
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
        let note = match (self.template_path(), kept) {
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
                .selected(self.template_path() == Some(path.as_str()))
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
    pub(super) fn base_line(&self, cx: &Context<Self>) -> Option<AnyElement> {
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

    /// That the fields came back from a draft left unsent, and the way to
    /// start over.
    pub(super) fn restored_line(&self, cx: &Context<Self>) -> Option<AnyElement> {
        self.restored.then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(theme::GAP_SM))
                .child(
                    div()
                        .text_size(px(theme::FONT_SIZE_SM))
                        .text_color(theme::current(cx).text_muted)
                        .child(s::pr_draft_restored()),
                )
                .child(
                    button_compact("create-pr-discard", s::pr_draft_discard())
                        .debug_selector(|| "create-pr-discard".into())
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.discard_creation(window, cx)
                        })),
                )
                .into_any_element()
        })
    }

    /// What became of sending the form, below its fields: the failure,
    /// the pull request the branch already had, or the one created but
    /// left out of its stack.
    pub(super) fn outcome_lines(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let action =
            |button: crate::ui::Button| div().flex().justify_end().child(button).into_any_element();
        match &self.state {
            CreateState::Editing | CreateState::Creating => Vec::new(),
            CreateState::Failed(message) => {
                vec![crate::ui::alert::error("create-pr-error", message.clone()).into_any_element()]
            }
            CreateState::Exists(created) => {
                let label = match created.number {
                    Some(number) => s::pr_open_created(s::pr_number(number)),
                    None => s::pr_open_on_github(),
                };
                vec![
                    crate::ui::alert::warning(
                        "create-pr-exists",
                        crate::surface::strings::error::github_pr_exists(),
                    )
                    .into_any_element(),
                    action(
                        button_primary("create-pr-open-existing", label)
                            .debug_selector(|| "create-pr-open-existing".into())
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.open_existing(window, cx)
                            })),
                    ),
                ]
            }
            CreateState::Unstacked { left, retrying } => {
                let parent = s::pr_number(left.parent);
                let message = match left.number {
                    Some(number) => s::pr_unstacked(s::pr_number(number), parent, &left.reason),
                    None => s::pr_unstacked_unnamed(parent, &left.reason),
                };
                let mut lines = vec![
                    crate::ui::alert::warning("create-pr-unstacked", message).into_any_element(),
                ];
                // Without its number there is nothing to stack again.
                if left.number.is_some() {
                    let label = if *retrying {
                        s::pr_stacking()
                    } else {
                        s::pr_stack_again()
                    };
                    lines.push(action(
                        button_primary("create-pr-stack-again", label)
                            .debug_selector(|| "create-pr-stack-again".into())
                            .disabled(*retrying)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.stack_again(window, cx)
                            })),
                    ));
                }
                lines
            }
        }
    }
}
