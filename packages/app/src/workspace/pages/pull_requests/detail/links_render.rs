//! Under the worktree: why it is tied to the pull request, untying it,
//! the worktrees that were untied from it, and tying one by hand.

use gpui::{AnyElement, Context, div, prelude::*, px};

use super::view::SideView;
use crate::github::links::Source;
use crate::surface::strings::pull_requests as s;
use crate::ui::{button_compact, theme};
use crate::workspace::Workspace;

pub(super) fn links(view: &SideView, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    let note = |text: String| {
        div()
            .text_size(px(theme::FONT_SIZE_SM))
            .text_color(t.text_subtle)
            .child(text)
    };
    let link = &view.link;
    let linked = view.lane.as_ref().zip(link.source).map(|(lane, source)| {
        let lane = lane.lane;
        let why = match source {
            Source::Branch => s::link_by_branch(),
            Source::Chosen => s::link_chosen(),
            Source::Review => s::link_review(),
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(theme::GAP_SM))
            .child(note(why))
            // A fork review worktree exists for this pull request.
            .when(source != Source::Review, |line| {
                line.child(
                    button_compact("pr-link-unlink", s::link_unlink())
                        .debug_selector(|| "pr-link-unlink".into())
                        .on_click(cx.listener(move |ws, _, _, cx| ws.unlink_pr_lane(lane, cx))),
                )
            })
    });
    let unlinked = link.unlinked.iter().enumerate().map(|(ix, lane)| {
        let target = lane.lane;
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(theme::GAP_SM))
            .child(note(s::link_unlinked(&lane.name)))
            .child(
                button_compact(("pr-link-restore", ix), s::link_restore())
                    .debug_selector(move || format!("pr-link-restore-{ix}"))
                    .on_click(cx.listener(move |ws, _, _, cx| ws.restore_pr_lane(target, cx))),
            )
    });
    let item = view.item.clone();
    let candidates = (!link.candidates.is_empty()).then(|| {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(theme::GAP_SM))
            .child(note(s::link_choose()))
            .children(link.candidates.iter().enumerate().map(|(ix, lane)| {
                let target = lane.lane;
                let (repo, number) = item.clone();
                button_compact(("pr-link-choose", ix), lane.name.clone())
                    .debug_selector(move || format!("pr-link-choose-{ix}"))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.link_pr_to_lane(target, &repo, number, cx)
                    }))
            }))
    });
    use crate::github::Keeping;
    let keeping = match link.keeping {
        Keeping::Saved => None,
        Keeping::MemoryOnly => Some(s::links_memory_only()),
        Keeping::SaveFailed => Some(s::links_save_failing()),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .children(keeping.map(|text| {
            div()
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.banner_warning_text)
                .child(text)
        }))
        .children(linked)
        .children(unlinked)
        .children(candidates)
        .into_any_element()
}
