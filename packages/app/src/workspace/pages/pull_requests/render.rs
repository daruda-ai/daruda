//! The Pull requests page's header and body, drawn from its staged snapshot.

use daruda_github::ReviewDecision;
use gpui::{
    AnyElement, App, Div, FontWeight, Hsla, IntoElement, SharedString, div, prelude::*, px,
};

use super::PrPageSnapshot;
use super::list::{
    self, Blocked, GroupBody, PageBody, PrGroup, PrRow, RowLane, RowLaneSlot, Trouble,
};
use crate::surface::strings::{common, git as git_s, pull_requests as s};
use crate::ui::list_table::{self, Column, ListTable};
use crate::ui::theme::list_metrics as metrics;
use crate::ui::{
    Badge, ButtonVariants as _, Disableable as _, Selectable as _, Sizable as _, button,
    button_compact, button_group, button_icon, icons, list_page, theme, tooltip,
};
use crate::workspace::left_dock::git_changes::pr_strip::{checks_label, tone_glyph};

#[derive(Clone, Copy)]
enum PrColumn {
    Tone,
    Title,
    Checks,
    Review,
    Comments,
    Lane,
    Open,
}

fn layout() -> ListTable<PrColumn> {
    ListTable::new([
        (PrColumn::Tone, Column::Fixed(theme::CONTROL_ICON_SIZE)),
        (PrColumn::Title, Column::title()),
        (PrColumn::Checks, Column::Fixed(metrics::CHECKS_W)),
        (PrColumn::Review, Column::Fixed(metrics::REVIEW_W)),
        (PrColumn::Comments, Column::Fixed(metrics::COMMENTS_W)),
        (PrColumn::Lane, Column::FixedUnclipped(metrics::LANE_W)),
        (
            PrColumn::Open,
            Column::FixedUnclipped(theme::CONTROL_TARGET_SIZE),
        ),
    ])
}

pub(in crate::workspace) fn header(
    snap: &PrPageSnapshot,
    close: AnyElement,
    cx: &App,
) -> AnyElement {
    let filters = {
        let workspace = snap.workspace.clone();
        button_group("pr-filter", cx)
            .children(list::FILTERS.iter().enumerate().map(|(ix, filter)| {
                button(("pr-filter", ix), list::filter_label(*filter))
                    .debug_selector(move || format!("pr-filter-{ix}"))
                    .selected(*filter == snap.list.filter)
                    .tab_stop(true)
            }))
            .on_click(move |selected: &Vec<usize>, _, cx| {
                if let Some(filter) = selected.first().and_then(|ix| list::FILTERS.get(*ix))
                    && let Some(ws) = workspace.upgrade()
                {
                    ws.update(cx, |ws, cx| ws.set_pr_page_filter(*filter, cx));
                }
            })
    };
    let refresh = {
        let workspace = snap.workspace.clone();
        button_icon("pr-refresh", icons::REFRESH, cx)
            .debug_selector(|| "pr-refresh".into())
            .tooltip(common::refresh())
            .on_click(move |_, _, cx| {
                if let Some(ws) = workspace.upgrade() {
                    ws.update(cx, |ws, cx| ws.refresh_pr_list(cx));
                }
            })
    };
    list_page::header(s::title(), filters, refresh, close, cx).into_any_element()
}

pub(in crate::workspace) fn render(snap: &PrPageSnapshot, cx: &App) -> AnyElement {
    let groups = match &snap.list.body {
        PageBody::Off => return list_page::empty(s::empty_disabled(), cx).into_any_element(),
        PageBody::NoRepos => {
            return list_page::empty(s::empty_no_repos(), cx).into_any_element();
        }
        PageBody::Groups(groups) => groups,
    };
    let layout = layout();
    let table = layout.body().children(
        groups
            .iter()
            .map(|group| render_group(group, snap, &layout, cx)),
    );
    list_table::scroll("pr-table-scroll", table).into_any_element()
}

fn render_group(
    group: &PrGroup,
    snap: &PrPageSnapshot,
    layout: &ListTable<PrColumn>,
    cx: &App,
) -> AnyElement {
    let t = theme::current(cx);
    let heading = div()
        .flex()
        .items_baseline()
        .gap(px(theme::GAP_LG))
        .px(px(theme::PAD_LG))
        .pt(px(theme::PAD_LG))
        .pb(px(theme::PAD_XS))
        .child(
            div()
                .text_size(px(theme::FONT_SIZE_SM))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text_muted)
                .child(group.label.clone()),
        )
        .child(
            div()
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_subtle)
                .child(group.meta.clone()),
        );
    let body: Vec<AnyElement> = match &group.body {
        GroupBody::Loading => vec![note(common::loading(), cx).into_any_element()],
        GroupBody::Blocked(blocked) => vec![blocked_note(group, *blocked, snap, cx)],
        GroupBody::Listed {
            rows,
            truncated,
            trouble,
        } => {
            let mut body: Vec<AnyElement> = Vec::new();
            if let Some(trouble) = trouble {
                body.push(trouble_note(group, *trouble, snap, cx));
            }
            let stale = trouble.is_some();
            body.extend(
                rows.iter()
                    .map(|row| render_row(row, stale, snap, layout, cx)),
            );
            if rows.is_empty() && trouble.is_none() {
                body.push(note(s::empty_repo(), cx).into_any_element());
            }
            if *truncated {
                body.push(note(s::truncated(), cx).into_any_element());
            }
            body
        }
    };
    div()
        .flex()
        .flex_col()
        .child(heading)
        .children(body)
        .into_any_element()
}

/// A quiet line inside a group, aligned with the rows' text.
fn note(message: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .px(px(theme::PAD_LG))
        .py(px(theme::PAD_SM))
        .text_size(px(theme::FONT_SIZE_SM))
        .text_color(theme::current(cx).text_subtle)
        .child(message.into())
}

fn trouble_label(trouble: Trouble) -> String {
    match trouble {
        Trouble::RateLimited => s::rate_limited(),
        Trouble::Unreachable => s::unreachable(),
        Trouble::SearchFailed => s::search_failed(),
    }
}

/// Why the rows may be out of date, with Try again when a search can be
/// sent now.
fn trouble_note(group: &PrGroup, trouble: Trouble, snap: &PrPageSnapshot, cx: &App) -> AnyElement {
    let line = note(trouble_label(trouble), cx);
    if !trouble.retryable() {
        return line.into_any_element();
    }
    let workspace = snap.workspace.clone();
    let selector = format!("pr-retry-{}", group.repo.qualified());
    let retry = button_compact(SharedString::from(selector.clone()), s::retry())
        .debug_selector(move || selector.clone())
        .on_click(move |_, _, cx| {
            if let Some(ws) = workspace.upgrade() {
                ws.update(cx, |ws, cx| ws.refresh_pr_list(cx));
            }
        });
    line.flex()
        .flex_wrap()
        .items_center()
        .gap(px(theme::GAP_LG))
        .child(retry)
        .into_any_element()
}

/// Why the host cannot be asked, with the Git panel's recheck beside it.
fn blocked_note(group: &PrGroup, blocked: Blocked, snap: &PrPageSnapshot, cx: &App) -> AnyElement {
    let host = group.repo.host().to_owned();
    let message = match blocked {
        Blocked::Missing => s::gh_missing(),
        Blocked::LoggedOut => s::gh_logged_out(&host),
    };
    let workspace = snap.workspace.clone();
    let recheck = button_compact(
        SharedString::from(format!("pr-recheck-{}", group.label)),
        git_s::pr_recheck(),
    )
    .on_click(move |_, _, cx| {
        if let Some(ws) = workspace.upgrade() {
            ws.update(cx, |ws, cx| ws.recheck_gh(&host, cx));
        }
    });
    note(message, cx)
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(theme::GAP_LG))
        .child(recheck)
        .into_any_element()
}

/// One pull request. A `stale` row is the last answer of a host that could
/// not be asked since: every colour steps down to the subtle tier.
fn render_row(
    row: &PrRow,
    stale: bool,
    snap: &PrPageSnapshot,
    layout: &ListTable<PrColumn>,
    cx: &App,
) -> AnyElement {
    let t = theme::current(cx);
    let ink = |color: Hsla| if stale { t.text_subtle } else { color };
    let glyph = tone_glyph(row.tone, cx).map(|(path, color)| (path, ink(color)));
    layout
        .row(|column, cell| match column {
            PrColumn::Tone => cell
                .flex()
                .items_center()
                .when_some(glyph, |cell, (path, color)| {
                    cell.child(
                        icons::icon(path)
                            .with_size(px(theme::CONTROL_ICON_SIZE))
                            .text_color(color),
                    )
                })
                .into_any_element(),
            PrColumn::Title => title(row, stale, cell, cx),
            PrColumn::Checks => cell
                .when_some(glyph, |cell, (_, color)| cell.text_color(color))
                .children(checks_label(row.checks))
                .into_any_element(),
            PrColumn::Review => cell
                .when_some(row.review, |cell, review| {
                    let color = match review {
                        ReviewDecision::Approved => t.banner_success_text,
                        ReviewDecision::ChangesRequested => t.banner_warning_text,
                        ReviewDecision::ReviewRequired => t.text_subtle,
                    };
                    cell.text_color(ink(color))
                        .child(list::review_label(review))
                })
                .into_any_element(),
            PrColumn::Comments => cell
                .id(SharedString::from(format!("pr-comments-{}", row.key)))
                .flex()
                .items_center()
                .gap(px(theme::GAP_SM))
                .text_color(if row.comments == 0 {
                    t.text_subtle
                } else {
                    ink(t.text_muted)
                })
                .tooltip(tooltip::text(s::comments(row.comments)))
                .child(icons::icon(icons::SESSION).with_size(px(theme::CONTROL_ICON_SIZE)))
                .child(row.comments.to_string())
                .into_any_element(),
            PrColumn::Lane => cell.child(lane_cell(row, snap)).into_any_element(),
            PrColumn::Open => cell
                .children(row.url.clone().map(|url| {
                    button_icon(
                        SharedString::from(format!("pr-open-{}", row.key)),
                        icons::OPEN_IN_NEW,
                        cx,
                    )
                    .tooltip(git_s::pr_open_on_github())
                    .on_click(move |_, _, cx| cx.open_url(&url))
                }))
                .into_any_element(),
        })
        .id(SharedString::from(format!("pr-row-{}", row.key)))
        .debug_selector({
            let key = row.key.clone();
            move || format!("pr-row-{key}")
        })
        .cursor_pointer()
        .hover(|style| style.bg(t.overlay_hover))
        .on_click({
            let workspace = snap.workspace.clone();
            let item = row.item.as_ref().clone();
            move |_, window, cx| {
                if let Some(ws) = workspace.upgrade() {
                    ws.update(cx, |ws, cx| ws.open_pr_detail(item.clone(), window, cx));
                }
            }
        })
        .into_any_element()
}

/// The row's worktree to go to, or the button that makes one, inert while
/// it is being made.
fn lane_cell(row: &PrRow, snap: &PrPageSnapshot) -> AnyElement {
    let item = match &row.lane {
        RowLaneSlot::Linked(lane) => return linked_lane(row, lane, snap),
        RowLaneSlot::Creating => None,
        RowLaneSlot::Absent(item) => Some(item.as_ref().clone()),
    };
    let label = if row.fork {
        s::create_review_lane()
    } else {
        s::create_lane()
    };
    let key = row.key.clone();
    let workspace = snap.workspace.clone();
    button_compact(
        SharedString::from(format!("pr-create-lane-{}", row.key)),
        label,
    )
    .debug_selector(move || format!("pr-create-lane-{key}"))
    .disabled(item.is_none())
    .on_click(move |_, window, cx| {
        if let (Some(item), Some(ws)) = (item.clone(), workspace.upgrade()) {
            ws.update(cx, |ws, cx| ws.create_lane_from_pr(item, window, cx));
        }
    })
    .into_any_element()
}

fn linked_lane(row: &PrRow, lane: &RowLane, snap: &PrPageSnapshot) -> AnyElement {
    let workspace = snap.workspace.clone();
    let target = lane.lane;
    let key = row.key.clone();
    button_compact(
        SharedString::from(format!("pr-lane-{}", row.key)),
        lane.name.clone(),
    )
    .ghost()
    .max_w_full()
    .justify_start()
    .debug_selector(move || format!("pr-lane-{key}"))
    .tooltip(s::go_to_lane())
    .on_click(move |_, window, cx| {
        if let Some(ws) = workspace.upgrade() {
            ws.update(cx, |ws, cx| ws.activate_lane(target, window, cx));
        }
    })
    .into_any_element()
}

fn title(row: &PrRow, stale: bool, cell: Div, cx: &App) -> AnyElement {
    let t = theme::current(cx);
    let ink = |color: Hsla| if stale { t.text_subtle } else { color };
    let chip = |label: String| Badge::new(label).text_color(ink(t.text_muted));
    cell.flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(theme::GAP_SM))
                .min_w_0()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(ink(t.text_body))
                        .child(row.title.clone()),
                )
                .when(row.draft, |line| line.child(chip(git_s::pr_state_draft())))
                .when(row.fork, |line| line.child(chip(s::fork()))),
        )
        .child(
            div()
                .w_full()
                .truncate()
                .font_family(theme::FONT_FAMILY_MONOSPACE)
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_subtle)
                .child(row.meta.clone()),
        )
        .into_any_element()
}
