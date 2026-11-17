//! Project scope and list controls use the same order as Tasks and Flows.

use gpui::{AnyElement, App, IntoElement, div, prelude::*, px};

use super::remote_search::SearchMode;
use super::{PrPageSnapshot, list};
use crate::github::RefineProblem;
use crate::surface::strings::{common, pull_requests as s};
use crate::ui::{
    ButtonVariants as _, Selectable as _, button, button_group, button_icon, icons, list_page, tab,
    tab_bar, theme,
};

pub(in crate::workspace) fn header(
    snap: &PrPageSnapshot,
    close: AnyElement,
    cx: &App,
) -> AnyElement {
    let workspace = snap.workspace.clone();
    let refresh = button_icon("pr-refresh", icons::REFRESH, cx)
        .debug_selector(|| "pr-refresh".into())
        .tooltip(if snap.list.activity.busy() {
            snap.list.activity.label().unwrap_or_else(common::refresh)
        } else {
            common::refresh()
        })
        .loading(snap.list.activity.busy())
        .on_click(move |_, _, cx| {
            if let Some(ws) = workspace.upgrade() {
                ws.update(cx, |ws, cx| ws.refresh_pr_list(cx));
            }
        });
    let action = div()
        .flex()
        .min_w_0()
        .items_center()
        .gap(px(theme::GAP_LG))
        .children(
            snap.list
                .activity
                .label()
                .or_else(|| snap.list.refreshed.clone())
                .map(|refreshed| {
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(theme::FONT_SIZE_SM))
                        .text_color(theme::current(cx).text_subtle)
                        .child(refreshed)
                }),
        )
        .child(refresh);
    list_page::header(
        s::title(),
        crate::workspace::pages::chrome::project_picker(&snap.scope, snap.workspace.clone(), cx),
        action,
        close,
        cx,
    )
    .into_any_element()
}

/// The audience tabs, with the state they list beside them, or below them
/// where the rows are stacked: squeezed beside the state, the tabs would
/// clip their own labels.
pub(super) fn filters(snap: &PrPageSnapshot, stacked: bool, cx: &App) -> impl IntoElement {
    let row = div().flex().gap(px(theme::GAP_LG));
    let row = if stacked {
        row.flex_col().items_start()
    } else {
        row.items_end()
    };
    row.child(
        div()
            .w_full()
            .min_w_0()
            .when(!stacked, |tabs| tabs.flex_1())
            .child(audiences(snap)),
    )
    .child(states(snap, cx))
}

fn states(snap: &PrPageSnapshot, cx: &App) -> impl IntoElement {
    let workspace = snap.workspace.clone();
    button_group("pr-state", cx)
        .children(list::STATES.into_iter().enumerate().map(|(index, state)| {
            button(("pr-state", index), list::state_label(state))
                .selected(snap.list.state == state)
                .debug_selector(move || format!("pr-state-{index}"))
        }))
        .on_click(move |indices: &Vec<usize>, _, cx| {
            if let Some(state) = indices.first().and_then(|index| list::STATES.get(*index))
                && let Some(ws) = workspace.upgrade()
            {
                ws.update(cx, |ws, cx| ws.set_pr_page_state(*state, cx));
            }
        })
}

fn audiences(snap: &PrPageSnapshot) -> impl IntoElement {
    let workspace = snap.workspace.clone();
    tab_bar("pr-filter")
        .w_full()
        .selected_index(
            list::FILTERS
                .iter()
                .position(|filter| *filter == snap.list.filter)
                .unwrap_or(0),
        )
        .children(
            list::FILTERS
                .into_iter()
                .enumerate()
                .map(|(index, filter)| {
                    tab(common::filter_count(
                        list::filter_label(filter),
                        &snap.counts[index],
                    ))
                    .debug_selector(move || format!("pr-filter-{index}"))
                }),
        )
        .on_click(move |index, _, cx| {
            if let Some(filter) = list::FILTERS.get(*index)
                && let Some(ws) = workspace.upgrade()
            {
                ws.update(cx, |ws, cx| ws.set_pr_page_filter(*filter, cx));
            }
        })
}

/// The search field, what it searches beside it, and in a GitHub search
/// its facets as chips that each remove themselves.
pub(super) fn search(snap: &PrPageSnapshot, cx: &App) -> impl IntoElement {
    let workspace = snap.workspace.clone();
    let field = list_page::toolbar().child(list_page::search(
        "pr-search-clear",
        common::search_clear().into(),
        &snap.search,
        snap.typed,
        move |window, cx| {
            if let Some(ws) = workspace.upgrade() {
                ws.update(cx, |ws, cx| ws.clear_pr_search(window, cx));
            }
        },
        cx,
    ));
    div()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .child(field.child(search_modes(snap, cx)))
        .children((!snap.facets.is_empty()).then(|| facet_chips(snap, cx)))
}

fn search_modes(snap: &PrPageSnapshot, cx: &App) -> impl IntoElement {
    let workspace = snap.workspace.clone();
    let modes = [SearchMode::Loaded, SearchMode::GitHub];
    button_group("pr-search-mode", cx)
        .children(modes.into_iter().enumerate().map(|(index, mode)| {
            let label = match mode {
                SearchMode::Loaded => s::search_loaded(),
                SearchMode::GitHub => s::search_github(),
            };
            button(("pr-search-mode", index), label)
                .selected(snap.mode == mode)
                .debug_selector(move || format!("pr-search-mode-{index}"))
        }))
        .on_click(move |indices: &Vec<usize>, window, cx| {
            if let Some(mode) = indices.first().and_then(|index| modes.get(*index))
                && let Some(ws) = workspace.upgrade()
            {
                ws.update(cx, |ws, cx| ws.set_pr_search_mode(*mode, window, cx));
            }
        })
}

/// Each facet as a chip: its `key:value`, and a button that removes it.
fn facet_chips(snap: &PrPageSnapshot, cx: &App) -> impl IntoElement {
    let t = theme::current(cx);
    div()
        .debug_selector(|| "pr-facets".into())
        .flex()
        .flex_wrap()
        .gap(px(theme::GAP_SM))
        .children(snap.facets.iter().enumerate().map(|(index, facet)| {
            let workspace = snap.workspace.clone();
            div()
                .debug_selector(move || format!("pr-facet-{index}"))
                .flex()
                .items_center()
                .gap(px(theme::GAP_XS))
                .pl(px(theme::PAD_SM))
                .rounded(px(theme::RADIUS_SM))
                .border_1()
                .border_color(t.border)
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_body)
                .child(facet.token())
                .child(
                    button_icon(("pr-facet-remove", index), icons::CLOSE, cx)
                        .debug_selector(move || format!("pr-facet-remove-{index}"))
                        .tooltip(s::remove_facet())
                        .on_click(move |_, window, cx| {
                            if let Some(ws) = workspace.upgrade() {
                                ws.update(cx, |ws, cx| ws.remove_pr_facet(index, window, cx));
                            }
                        }),
                )
        }))
}

pub(super) fn results(snap: &PrPageSnapshot, cx: &App) -> impl IntoElement {
    let workspace = snap.workspace.clone();
    let clear = (snap.typed || !snap.query.is_empty()).then(|| {
        button("pr-clear-filters", common::clear_filters())
            .ghost()
            .tab_stop(true)
            .debug_selector(|| "pr-clear-filters".into())
            .on_click(move |_, window, cx| {
                if let Some(ws) = workspace.upgrade() {
                    ws.update(cx, |ws, cx| ws.clear_pr_filters(window, cx));
                }
            })
            .into_any_element()
    });
    div()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .child(list_page::results(
            match (snap.mode, snap.problem, snap.list.found) {
                // Nothing was sent: what keeps it is what the line says.
                (_, Some(RefineProblem::NeedsAllTab), _) => s::search_needs_all_tab(),
                (_, Some(RefineProblem::Unsupported), _) => s::search_unsupported(),
                (SearchMode::Loaded, None, _) => s::result_count(snap.visible, snap.total),
                (SearchMode::GitHub, None, Some(found)) => {
                    s::result_count_github(snap.visible, found)
                }
                (SearchMode::GitHub, None, None) => s::result_count_github_loaded(snap.visible),
            },
            clear,
            cx,
        ))
        .children(snap.list.sources.label().map(|label| {
            div()
                .debug_selector(|| "pr-source-summary".into())
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(theme::current(cx).text_subtle)
                .child(label)
        }))
        .children((!snap.list.accounts.is_empty()).then(|| {
            div()
                .debug_selector(|| "pr-accounts".into())
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(theme::current(cx).text_subtle)
                .child(s::accounts(&snap.list.accounts))
        }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn list_filters_keep_their_query_meaning() {
        assert_eq!(super::list::FILTERS.len(), 3);
    }
}
