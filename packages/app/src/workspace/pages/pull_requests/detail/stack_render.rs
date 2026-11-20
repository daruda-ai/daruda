//! The pull request's native stack beside the detail: where it sits, the
//! branch the stack targets, and each pull request of it to go to.

use daruda_github::{PrState, RepoId};
use gpui::{AnyElement, Context, div, prelude::*, px};

use super::PrDetailId;
use super::view::StackView;
use crate::surface::strings::{git as git_s, pull_requests as s};
use crate::ui::{Disableable as _, button_compact, theme};
use crate::workspace::Workspace;

pub(super) fn stack(
    id: PrDetailId,
    repo: RepoId,
    view: &StackView,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let note = |text: String| {
        div()
            .text_size(px(theme::FONT_SIZE_SM))
            .text_color(t.text_subtle)
            .child(text)
    };
    let place = match view.shown_at {
        Some(position) => s::stack_place(position, view.size, &view.base),
        // Not among those listed: no position is claimed.
        None => s::stack_place_unlisted(view.size, &view.base),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .child(note(place))
        .children(view.entries.iter().enumerate().map(|(ix, entry)| {
            let state = match entry.state {
                PrState::Open => None,
                PrState::Draft => Some(git_s::pr_state_draft()),
                PrState::Merged => Some(git_s::pr_state_merged()),
                PrState::Closed => Some(git_s::pr_state_closed()),
            };
            let label = match state {
                Some(state) => format!(
                    "{}. {} {} · {state}",
                    entry.position,
                    git_s::pr_number(entry.number),
                    entry.title
                ),
                None => format!(
                    "{}. {} {}",
                    entry.position,
                    git_s::pr_number(entry.number),
                    entry.title
                ),
            };
            let (repo, number) = (repo.clone(), entry.number);
            let row = button_compact(("pr-stack-entry", ix), label)
                .debug_selector(move || format!("pr-stack-entry-{ix}"));
            if entry.shown {
                row.disabled(true).into_any_element()
            } else {
                row.on_click(cx.listener(move |ws, _, window, cx| {
                    ws.open_stack_entry(id, repo.clone(), number, window, cx)
                }))
                .into_any_element()
            }
        }))
        .children((view.more > 0).then(|| note(s::stack_more(view.more))))
        .into_any_element()
}
