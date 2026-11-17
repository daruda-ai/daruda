//! The Pull requests page's header and body, drawn from its staged snapshot.

use daruda_github::ReviewDecision;
use gpui::{
    AnyElement, App, Div, FontWeight, Hsla, IntoElement, SharedString, canvas, div, prelude::*, px,
};

use super::PrPageSnapshot;
use super::list::{self, Blocked, GroupBody, LaneSlot, PageBody, PrGroup, PrRow, RowLane, Trouble};
use super::navigation;
use super::row_layout::{PrColumn, RowLayout, table};
use crate::surface::strings::{common, git as git_s, pull_requests as s};
use crate::ui::list_table::ListTable;
use crate::ui::{
    Badge, ButtonVariants as _, Disableable as _, Sizable as _, button_compact, button_icon, icons,
    list_page, theme, tooltip,
};
use crate::workspace::left_dock::git_changes::pr_strip::{checks_label, tone_glyph};

/// The page header: title, filters, search and result counts, and in the
/// table layout the column headings, so none of them scroll with the rows.
pub(in crate::workspace) fn header(
    snap: &PrPageSnapshot,
    rows: RowLayout,
    close: AnyElement,
    cx: &App,
) -> AnyElement {
    let headings = (rows == RowLayout::Table && matches!(snap.list.body, PageBody::Groups(_)))
        .then(|| headings(cx));
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .child(super::controls::header(snap, close, cx))
        .child(
            div()
                .debug_selector(|| "pr-list-controls".into())
                .flex()
                .flex_col()
                .min_w_0()
                .px(px(theme::DOCK_PAGE_PAD))
                .pb(px(theme::PAD_LG))
                .gap(px(theme::GAP_LG))
                .child(super::controls::filters(snap))
                .child(super::controls::search(snap, cx))
                .child(super::controls::results(snap, cx)),
        )
        .children(headings)
        .into_any_element()
}

/// The column headings, inset like the rows below them.
fn headings(cx: &App) -> AnyElement {
    let table = table();
    div()
        .debug_selector(|| "pr-list-headings".into())
        .px(px(theme::DOCK_PAGE_PAD))
        .child(
            table
                .row(|column, cell| match column {
                    PrColumn::Title => cell.child(s::column_title()).into_any_element(),
                    PrColumn::Checks => cell.child(s::column_checks()).into_any_element(),
                    PrColumn::Review => cell.child(s::column_review()).into_any_element(),
                    PrColumn::Comments => cell.child(s::column_comments()).into_any_element(),
                    PrColumn::Lane => cell.child(common::column_worktree()).into_any_element(),
                    PrColumn::Tone | PrColumn::Open => cell.into_any_element(),
                })
                .text_color(theme::current(cx).text_muted),
        )
        .into_any_element()
}

/// What every row of one frame is drawn with.
struct Draw<'a> {
    snap: &'a PrPageSnapshot,
    /// Whether the list has the keyboard, which the selected row shows.
    focused: bool,
    rows: RowLayout,
    table: ListTable<PrColumn>,
}

/// The list body. It holds the keyboard focus whatever it shows, so the
/// focus the page gives it is never on an element that is not drawn.
pub(in crate::workspace) fn render(
    snap: &PrPageSnapshot,
    rows: RowLayout,
    focused: bool,
    cx: &App,
) -> AnyElement {
    let draw = Draw {
        snap,
        focused,
        rows,
        table: table(),
    };
    div()
        .key_context(KEY_CONTEXT)
        .track_focus(&snap.focus)
        .debug_selector(|| "pr-list-body".into())
        .child(body(&draw, cx))
        .into_any_element()
}

/// The key context the list's browsing keys are bound in.
pub(crate) const KEY_CONTEXT: &str = "PullRequestsList";

fn body(draw: &Draw<'_>, cx: &App) -> AnyElement {
    let groups = match &draw.snap.list.body {
        PageBody::Off => return list_page::empty(s::empty_disabled(), cx).into_any_element(),
        PageBody::Discovering => return list_page::empty(s::discovering(), cx).into_any_element(),
        PageBody::NoRepos => return list_page::empty(s::empty_no_repos(), cx).into_any_element(),
        PageBody::Groups(groups) => groups,
    };
    let groups = groups.iter().map(|group| render_group(group, draw, cx));
    match draw.rows {
        // The columns fit by construction; a pixel lost to rounding is
        // clipped rather than scrolled, keeping the headings aligned.
        RowLayout::Table => draw
            .table
            .body()
            .overflow_hidden()
            .children(groups)
            .into_any_element(),
        RowLayout::Compact => div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .children(groups)
            .into_any_element(),
    }
}

fn render_group(group: &PrGroup, draw: &Draw<'_>, cx: &App) -> AnyElement {
    let snap = draw.snap;
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
        )
        .children(group.refreshed.clone().map(|age| {
            div()
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_subtle)
                .child(age)
        }));
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
            body.extend(rows.iter().map(|row| render_row(row, stale, draw, cx)));
            if rows.is_empty() && trouble.is_none() {
                body.push(
                    note(
                        if snap.query.is_empty() {
                            s::empty_repo()
                        } else {
                            s::empty_search()
                        },
                        cx,
                    )
                    .into_any_element(),
                );
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

/// A readable status line inside a group, aligned with the rows' text.
/// The message may shrink, so it wraps beside a button in a narrow page
/// instead of running past its edge.
fn note(message: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .px(px(theme::PAD_LG))
        .py(px(theme::PAD_SM))
        .text_size(px(theme::FONT_SIZE_MD))
        .text_color(theme::current(cx).text_body)
        .child(div().min_w_0().child(message.into()))
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
    let host = group.repo.host().to_owned();
    let selector = format!("pr-retry-{}", group.repo.qualified());
    let retry = button_compact(SharedString::from(selector.clone()), s::retry())
        .debug_selector(move || selector.clone())
        .on_click(move |_, _, cx| {
            if let Some(ws) = workspace.upgrade() {
                ws.update(cx, |ws, cx| ws.refresh_pr_host(&host, cx));
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
        Blocked::UnconfirmedHost => s::gh_unconfirmed_host(&host),
    };
    let workspace = snap.workspace.clone();
    // The qualified name: two hosts can hold the same `owner/name`.
    let selector = format!("pr-recheck-{}", group.repo.qualified());
    let recheck = button_compact(SharedString::from(selector.clone()), git_s::pr_recheck())
        .debug_selector(move || selector.clone())
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
/// not be asked since: every colour steps down to the subtle tier. The
/// selected row keeps its background and accent bar, which dims while the
/// list does not have the keyboard.
fn render_row(row: &PrRow, stale: bool, draw: &Draw<'_>, cx: &App) -> AnyElement {
    let cells = Cells::new(row, stale, cx);
    let line = match draw.rows {
        RowLayout::Table => draw.table.row(|column, cell| match column {
            PrColumn::Tone => cells.tone(cell),
            PrColumn::Title => title(row, stale, cell, cx),
            PrColumn::Checks => cells.checks(cell),
            PrColumn::Review => cells.review(cell),
            PrColumn::Comments => cells.comments(cell),
            PrColumn::Lane => cell.child(lane_cell(row, draw.snap)).into_any_element(),
            PrColumn::Open => cells.open(cell),
        }),
        RowLayout::Compact => compact_row(row, stale, &cells, draw.snap, cx),
    };
    row_chrome(line, row, draw, cx)
}

/// The stacked row: the title and meta lines, then the side facts, which
/// wrap rather than push the row wider than the page.
fn compact_row(
    row: &PrRow,
    stale: bool,
    cells: &Cells<'_>,
    snap: &PrPageSnapshot,
    cx: &App,
) -> Div {
    let fact = || div().flex_none();
    let facts = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(px(theme::GAP_LG))
        .gap_y(px(theme::GAP_XS))
        .text_size(px(theme::FONT_SIZE_SM))
        .child(cells.checks(fact()))
        .child(cells.review(fact()))
        .child(cells.comments(fact()))
        .child(div().min_w_0().max_w_full().child(lane_cell(row, snap)));
    div()
        .flex()
        .w_full()
        .min_w_0()
        .items_start()
        .gap(px(theme::GAP_LG))
        .px(px(theme::PAD_LG))
        .py(px(theme::PAD_SM))
        .text_size(px(theme::FONT_SIZE_MD))
        .child(cells.tone(div().flex_none().pt(px(theme::PAD_XS))))
        .child(
            div()
                .debug_selector({
                    let key = row.key.clone();
                    move || format!("pr-compact-row-{key}")
                })
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(theme::GAP_XS))
                .child(title(row, stale, div().w_full().min_w_0(), cx))
                .child(facts),
        )
        .child(cells.open(div().flex_none()))
}

/// What both layouts put around a row: its identity, the selection, the
/// geometry keyboard moves scroll by, and opening it on click.
fn row_chrome(line: Div, row: &PrRow, draw: &Draw<'_>, cx: &App) -> AnyElement {
    let t = theme::current(cx);
    let key = navigation::key(row);
    let selected = draw.snap.cursor.as_ref() == Some(&key);
    let bounds = draw.snap.row_bounds.clone();
    let selector = format!("pr-row-selected-{}", row.key);
    let focused = draw.focused;
    line.id(SharedString::from(format!("pr-row-{}", row.key)))
        .debug_selector({
            let key = row.key.clone();
            move || format!("pr-row-{key}")
        })
        .relative()
        .child(
            canvas(
                move |at, _, _| {
                    bounds.borrow_mut().insert(key, at);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
            .size_full(),
        )
        .when(selected, |line| {
            line.bg(t.lane_card_active_bg).child(
                div()
                    .debug_selector(move || selector.clone())
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(theme::LANE_ACTIVE_BORDER_W))
                    .bg(if focused {
                        theme::PRIMARY
                    } else {
                        t.text_subtle
                    }),
            )
        })
        .cursor_pointer()
        .when(!selected, |line| {
            line.hover(|style| style.bg(t.overlay_hover))
        })
        .on_click({
            let workspace = draw.snap.workspace.clone();
            let item = row.item.as_ref().clone();
            move |_, window, cx| {
                if let Some(ws) = workspace.upgrade() {
                    ws.update(cx, |ws, cx| ws.open_pr_detail(item.clone(), window, cx));
                }
            }
        })
        .into_any_element()
}

/// The side facts of one row, each filled into the cell or slot a layout
/// gives it.
struct Cells<'a> {
    row: &'a PrRow,
    stale: bool,
    glyph: Option<(&'static str, Hsla)>,
    cx: &'a App,
}

impl<'a> Cells<'a> {
    fn new(row: &'a PrRow, stale: bool, cx: &'a App) -> Self {
        let mut cells = Self {
            row,
            stale,
            glyph: None,
            cx,
        };
        cells.glyph = tone_glyph(row.tone, cx).map(|(path, color)| (path, cells.ink(color)));
        cells
    }

    /// `color`, or the subtle tier on a stale row.
    fn ink(&self, color: Hsla) -> Hsla {
        if self.stale {
            theme::current(self.cx).text_subtle
        } else {
            color
        }
    }

    fn tone(&self, cell: Div) -> AnyElement {
        cell.flex()
            .items_center()
            .when_some(self.glyph, |cell, (path, color)| {
                cell.child(
                    icons::icon(path)
                        .with_size(px(theme::CONTROL_ICON_SIZE))
                        .text_color(color),
                )
            })
            .into_any_element()
    }

    fn checks(&self, cell: Div) -> AnyElement {
        cell.when_some(self.glyph, |cell, (_, color)| cell.text_color(color))
            .children(checks_label(self.row.checks))
            .into_any_element()
    }

    fn review(&self, cell: Div) -> AnyElement {
        let t = theme::current(self.cx);
        cell.when_some(self.row.review, |cell, review| {
            let color = match review {
                ReviewDecision::Approved => t.banner_success_text,
                ReviewDecision::ChangesRequested => t.banner_warning_text,
                ReviewDecision::ReviewRequired => t.text_subtle,
            };
            cell.text_color(self.ink(color))
                .child(list::review_label(review))
        })
        .into_any_element()
    }

    fn comments(&self, cell: Div) -> AnyElement {
        let row = self.row;
        let t = theme::current(self.cx);
        cell.id(SharedString::from(format!("pr-comments-{}", row.key)))
            .flex()
            .items_center()
            .gap(px(theme::GAP_SM))
            .text_color(if row.comments == 0 {
                t.text_subtle
            } else {
                self.ink(t.text_muted)
            })
            .tooltip(tooltip::text(s::comments(row.comments)))
            .child(icons::icon(icons::SESSION).with_size(px(theme::CONTROL_ICON_SIZE)))
            .child(row.comments.to_string())
            .into_any_element()
    }

    fn open(&self, cell: Div) -> AnyElement {
        let row = self.row;
        cell.children(row.url.clone().map(|url| {
            button_icon(
                SharedString::from(format!("pr-open-{}", row.key)),
                icons::OPEN_IN_NEW,
                self.cx,
            )
            .tooltip(git_s::pr_open_on_github())
            .on_click(move |_, _, cx| cx.open_url(&url))
        }))
        .into_any_element()
    }
}

/// The row's worktree to go to, or the button that makes one, inert while
/// it is being made.
fn lane_cell(row: &PrRow, snap: &PrPageSnapshot) -> AnyElement {
    let (item, review) = match &row.lane {
        LaneSlot::Linked(lane) => return linked_lane(row, lane, snap),
        LaneSlot::Creating { review } => (None, *review),
        LaneSlot::Absent { review } => (Some(row.item.as_ref().clone()), *review),
    };
    let label = list::create_lane_label(review);
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
            ws.update(cx, |ws, cx| ws.create_lane_from_pr(item, None, window, cx));
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
                        .id(SharedString::from(format!("pr-title-{}", row.key)))
                        .min_w_0()
                        .truncate()
                        .tooltip(tooltip::text(row.title.clone()))
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
