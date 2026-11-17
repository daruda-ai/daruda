//! Project scope and list controls use the same order as Tasks and Flows.

use gpui::{AnyElement, App, IntoElement, div, prelude::*, px};

use super::{PrPageSnapshot, list};
use crate::surface::strings::{common, pull_requests as s};
use crate::ui::{ButtonVariants as _, button, button_icon, icons, list_page, tab, tab_bar, theme};

pub(in crate::workspace) fn header(
    snap: &PrPageSnapshot,
    close: AnyElement,
    cx: &App,
) -> AnyElement {
    let workspace = snap.workspace.clone();
    let refresh = button_icon("pr-refresh", icons::REFRESH, cx)
        .debug_selector(|| "pr-refresh".into())
        .tooltip(common::refresh())
        .on_click(move |_, _, cx| {
            if let Some(ws) = workspace.upgrade() {
                ws.update(cx, |ws, cx| ws.refresh_pr_list(cx));
            }
        });
    let action = div()
        .flex()
        .items_center()
        .gap(px(theme::GAP_LG))
        .children(snap.list.refreshed.clone().map(|refreshed| {
            div()
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(theme::current(cx).text_subtle)
                .child(refreshed)
        }))
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

pub(super) fn filters(snap: &PrPageSnapshot) -> impl IntoElement {
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

pub(super) fn search(snap: &PrPageSnapshot, cx: &App) -> impl IntoElement {
    let workspace = snap.workspace.clone();
    list_page::toolbar().child(list_page::search(
        "pr-search-clear",
        common::search_clear().into(),
        &snap.search,
        !snap.query.is_empty(),
        move |window, cx| {
            if let Some(ws) = workspace.upgrade() {
                ws.update(cx, |ws, cx| ws.clear_pr_search(window, cx));
            }
        },
        cx,
    ))
}

pub(super) fn results(snap: &PrPageSnapshot, cx: &App) -> impl IntoElement {
    let workspace = snap.workspace.clone();
    let clear = (!snap.query.trim().is_empty()).then(|| {
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
    list_page::results(s::result_count(snap.visible, snap.total), clear, cx)
}

#[cfg(test)]
mod tests {
    #[test]
    fn list_filters_keep_their_query_meaning() {
        assert_eq!(super::list::FILTERS.len(), 3);
    }
}
