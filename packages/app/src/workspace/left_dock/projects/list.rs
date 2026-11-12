//! Lanes view — list of lanes plus a section header. Rendered
//! into the left dock when `left_dock_view == Lanes`.
//!
//! Top-level rows follow [`project_order::arrange`]. A group's member
//! projects render only when the group is expanded.

use crate::ui::theme;
use gpui::{AnyElement, Context, IntoElement, div, prelude::*, px};

use crate::surface::strings as surface_strings;
use crate::workspace::layout::{Dock, LeftDockSnapshot, ProjectSnapshot};
use crate::workspace::project_order::{self, GroupKey, ProjectKey, TopRow};

use super::rows::{
    git_badge_for, group_header_row, non_git_placeholder, pr_badge_for, project_header_row,
    section_header, worktree_row,
};

/// Render the Lanes view body.
pub(in crate::workspace) fn render(snap: &LeftDockSnapshot, cx: &mut Context<Dock>) -> AnyElement {
    // `_any_git` is unused inside `section_header` — pass `false` unconditionally.
    let header = section_header(false, snap, cx);

    // The header stays outside the scroll region so the section
    // actions remain reachable no matter where the tree is scrolled.
    let mut body = crate::workspace::left_dock::left_panel_body()
        .gap(px(theme::LANE_ROOT_GAP))
        .child(header);

    if snap.projects.is_empty() {
        body = body.child(empty_state(cx));
        return body.into_any_element();
    }

    let any_git = snap
        .projects
        .iter()
        .flat_map(|p| &p.lanes)
        .any(|w| w.is_git());

    let active_project = snap.active.project;
    let active_lane = snap.active.lane;

    let top_rows = top_rows(snap);

    let scroll_handle = snap.lanes_scroll_handle.clone();
    let mut roots = div()
        .id("lanes-scroll")
        .flex()
        .flex_col()
        .size_full()
        .gap(px(theme::LANE_ROOT_GAP))
        .overflow_y_scroll()
        .track_scroll(&scroll_handle);

    for row in top_rows {
        match row {
            TopRow::Group { group, members } => {
                let group = &snap.groups[group];
                let header = group_header_row(group, snap, cx).into_any_element();
                let members_block = {
                    let mut inner = div().flex().flex_col().w_full();
                    if !group.is_collapsed {
                        for project in members {
                            inner = inner.child(grouped_project_block(
                                &snap.projects[project],
                                active_project,
                                active_lane,
                                snap,
                                cx,
                            ));
                        }
                    }
                    inner.into_any_element()
                };
                roots = roots.child(super::tree::group(
                    header,
                    members_block,
                    theme::current(cx).border,
                ));
            }
            TopRow::UngroupedProject(project) => {
                let inner = ungrouped_project_block(
                    &snap.projects[project],
                    active_project,
                    active_lane,
                    snap,
                    cx,
                )
                .into_any_element();
                roots = roots.child(super::tree::project(inner));
            }
        }
    }

    // Non-git project hint — surfaces the `git init` path.
    if !any_git {
        roots = roots.child(non_git_placeholder(snap.metrics, cx));
    }

    // Thumb overlay needs a `.relative()` parent; the tree's own
    // `LANE_TREE_MARGIN_X` gutter already clears the thumb (see
    // `lanes_scrollbar`).
    body = body.child(
        div()
            .flex_1()
            .relative()
            .overflow_hidden()
            .child(roots)
            .children(lanes_scrollbar(&scroll_handle, cx)),
    );

    body.into_any_element()
}

/// The tree's top-level rows, by index into `snap.groups` and
/// `snap.projects`, in the order every project-arranging surface shares:
/// the expanded tree and the collapsed rail.
pub(in crate::workspace::left_dock) fn top_rows(snap: &LeftDockSnapshot) -> Vec<TopRow> {
    let groups: Vec<GroupKey> = snap
        .groups
        .iter()
        .map(|g| GroupKey {
            id: g.id,
            tab_order: g.tab_order,
        })
        .collect();
    let projects: Vec<ProjectKey> = snap
        .projects
        .iter()
        .map(|p| ProjectKey {
            group: p.group_id,
            tab_order: p.tab_order,
        })
        .collect();
    project_order::arrange(&groups, &projects)
}

/// Scrollbar thumb for the project tree; a track press or a thumb drag
/// scrolls `handle`, as in the Git Changes / Files dock views.
///
/// Sits in the gutter the tree leaves free: the press strip spans the right
/// `SCROLLBAR_HIT_W` (8 px) while a tree root stops at `LANE_TREE_MARGIN_X`
/// (8 px), so it never overlaps a row.
fn lanes_scrollbar(
    handle: &gpui::ScrollHandle,
    cx: &gpui::App,
) -> Option<crate::ui::scrollbar::Thumb> {
    let viewport_h = handle.bounds().size.height;
    let max_offset = handle.max_offset().y;
    let t = theme::current(cx);
    crate::ui::scrollbar::vertical_thumb(
        "lanes-scrollbar-thumb",
        handle.clone(),
        viewport_h,
        viewport_h + max_offset,
        handle.offset().y,
        px(0.),
        t.scrollbar_thumb,
        t.dock_scrollbar_thumb_hover,
    )
}

/// Project header + lane rows rendered flush against the dock edge.
fn ungrouped_project_block(
    project: &ProjectSnapshot,
    active_project: daruda_store::project::ProjectId,
    active_lane: daruda_store::project::LaneId,
    snap: &LeftDockSnapshot,
    cx: &mut Context<Dock>,
) -> impl IntoElement + use<> {
    // Same vertical gap as between adjacent lane rows — keeps the
    // project header from sitting flush against its first lane, so
    // header / list reads as the same rhythm the lane list uses
    // internally.
    let mut block = div()
        .flex()
        .flex_col()
        .w_full()
        .gap(px(theme::LANE_LIST_GAP_Y));
    let is_active_project = project.id == active_project;
    let project_is_git = project
        .lanes
        .iter()
        .any(|w| matches!(&w.kind, daruda_store::project::LaneKind::Git { .. }));
    block = block.child(project_header_row(
        super::rows::ProjectHeaderArgs {
            project_id: project.id,
            name: project.name.clone(),
            display_alias: project.display_alias.clone(),
            is_ungrouped: project.group_id.is_none(),
            is_active: is_active_project,
            is_git: project_is_git,
            is_collapsed: project.is_collapsed,
            last_active_lane_id: project.last_active_lane_id,
            availability: project.availability,
            default_branch: project.default_branch.clone(),
        },
        snap,
        cx,
    ));
    if !project.is_collapsed {
        let mut list = div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(theme::LANE_LIST_GAP_Y));
        for wt in &project.lanes {
            let is_active = project.id == active_project && wt.id == active_lane;
            let badges = super::rows::LaneBadges {
                git: git_badge_for(snap, project.id, wt.id),
                pr: pr_badge_for(snap, project.id, wt.id),
            };
            list = list.child(worktree_row(wt, project.id, is_active, badges, snap, cx));
        }
        block = block.child(list);
    }
    block
}

/// Group members indent one step; lanes indent by their own row inset.
fn grouped_project_block(
    project: &ProjectSnapshot,
    active_project: daruda_store::project::ProjectId,
    active_lane: daruda_store::project::LaneId,
    snap: &LeftDockSnapshot,
    cx: &mut Context<Dock>,
) -> impl IntoElement + use<> {
    div()
        .flex()
        .flex_col()
        .w_full()
        .pl(px(theme::LANE_INDENT_STEP))
        .child(ungrouped_project_block(
            project,
            active_project,
            active_lane,
            snap,
            cx,
        ))
}

fn empty_state(cx: &mut Context<Dock>) -> impl IntoElement {
    let cta = crate::ui::button_primary(
        "projects-empty-add",
        surface_strings::projects::empty_state_cta(),
    )
    .on_click(cx.listener(|_dock, _: &gpui::ClickEvent, _window, app_cx| {
        // prompt_and_open_folder_with_policy takes only &mut App — _window unused.
        let config = crate::settings_store::SettingsStore::global(app_cx).user_arc();
        crate::windows::prompt_and_open_folder_with_policy(config, app_cx);
    }));

    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(theme::MAIN_EMPTY_STATE_GAP))
        .text_size(px(theme::DOCK_PLACEHOLDER_FONT_SIZE))
        .text_color(theme::current(cx).text_subtle)
        .child(crate::ui::placeholder_text(
            surface_strings::projects::empty_state(),
        ))
        .child(cta)
}

#[cfg(test)]
mod tests {
    use super::{TopRow, top_rows};

    /// The tree's top rows, read from the staged snapshot, are the
    /// workspace's shared order: groups and ungrouped projects interleaved
    /// by `tab_order`, a group's members by their own, a collapsed group
    /// still listing its members and an empty one keeping its row.
    #[gpui::test]
    fn the_tree_arranges_the_snapshot_in_the_shared_order(cx: &mut gpui::TestAppContext) {
        let root = std::env::temp_dir().join("daruda-left-dock-top-rows");
        let (_, workspace) = crate::workspace::tests::build_workspace_with(
            cx,
            &daruda_config::Config::default(),
            Some(daruda_store::project::Project::from_path(root.join("a"))),
        );
        workspace.update(cx, |ws, cx| {
            for name in ["b", "c", "d"] {
                let id = ws.next_project_id;
                ws.next_project_id += 1;
                let mut project = crate::project::Project::bootstrap(
                    id,
                    root.join(name),
                    &crate::lane::git::FakeGit::new(),
                );
                project.tab_order = ws.projects.len() as u32;
                ws.projects.push(project);
            }
            let ids: Vec<_> = ws.projects.iter().map(|p| p.id).collect();
            // Top pool: a(0) c(2) tools(4) apps(5); tools holds d(3), b(1).
            let tools = ws.add_group("tools".into(), None, cx);
            ws.add_group("apps".into(), None, cx);
            ws.move_project_to_group(ids[3], Some(tools), cx);
            ws.move_project_to_group(ids[1], Some(tools), cx);
            ws.toggle_group_collapse(tools, cx);

            let snap = ws.prepare_left_dock_snapshot(cx);
            let tree = named(
                top_rows(&snap),
                |g| snap.groups[g].name.to_string(),
                |p| snap.projects[p].name.to_string(),
            );
            assert_eq!(tree, ["a", "c", "tools[b,d]", "apps[]"]);
            let (groups, projects) = ws.project_order_keys();
            let model = named(
                crate::workspace::project_order::arrange(&groups, &projects),
                |g| ws.groups[g].name.clone(),
                |p| ws.projects[p].name.clone(),
            );
            assert_eq!(tree, model, "the tree and the workspace agree");
        });
    }

    /// Each row by name; a group with its members in brackets.
    fn named(
        rows: Vec<TopRow>,
        group: impl Fn(usize) -> String,
        project: impl Fn(usize) -> String,
    ) -> Vec<String> {
        rows.into_iter()
            .map(|row| match row {
                TopRow::Group { group: g, members } => {
                    let members: Vec<String> = members.into_iter().map(&project).collect();
                    format!("{}[{}]", group(g), members.join(","))
                }
                TopRow::UngroupedProject(p) => project(p),
            })
            .collect()
    }
}
