//! The order the project sidebar shows groups and projects in, shared by
//! every surface that arranges projects as the sidebar does: its tree, the
//! collapsed rail, drag-and-drop renumbering, and the remote app home.
//! GPUI-free.
//!
//! Top-level rows interleave groups and ungrouped projects by their shared
//! `tab_order` pool; a group's member projects follow their own `tab_order`.

use daruda_store::project::GroupId;

/// What ordering reads of one group.
#[derive(Clone, Copy, Debug)]
pub(in crate::workspace) struct GroupKey {
    pub id: GroupId,
    pub tab_order: u32,
}

/// What ordering reads of one project.
#[derive(Clone, Copy, Debug)]
pub(in crate::workspace) struct ProjectKey {
    pub group: Option<GroupId>,
    pub tab_order: u32,
}

/// One top-level row, by index into the slices [`arrange`] was given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::workspace) enum TopRow {
    /// A group and its member projects, in their order.
    Group {
        group: usize,
        members: Vec<usize>,
    },
    UngroupedProject(usize),
}

impl TopRow {
    fn tab_order(&self, groups: &[GroupKey], projects: &[ProjectKey]) -> u32 {
        match self {
            TopRow::Group { group, .. } => groups[*group].tab_order,
            TopRow::UngroupedProject(project) => projects[*project].tab_order,
        }
    }
}

/// The sidebar's top-level rows. Sorts are stable: on a `tab_order` tie a
/// group comes before an ungrouped project, and otherwise the input order
/// decides. A group without members still has its row; a project naming a
/// group that is not in `groups` has none, as the sidebar shows it nowhere.
pub(in crate::workspace) fn arrange(groups: &[GroupKey], projects: &[ProjectKey]) -> Vec<TopRow> {
    let mut rows: Vec<TopRow> = Vec::with_capacity(groups.len() + projects.len());
    for (index, group) in groups.iter().enumerate() {
        rows.push(TopRow::Group {
            group: index,
            members: members(group.id, projects),
        });
    }
    rows.extend(
        (0..projects.len())
            .filter(|&p| projects[p].group.is_none())
            .map(TopRow::UngroupedProject),
    );
    rows.sort_by_key(|row| row.tab_order(groups, projects));
    rows
}

/// The projects of `group`, by index into `projects`, in their order; ties
/// keep input order.
pub(in crate::workspace) fn members(group: GroupId, projects: &[ProjectKey]) -> Vec<usize> {
    let mut members: Vec<usize> = (0..projects.len())
        .filter(|&p| projects[p].group == Some(group))
        .collect();
    members.sort_by_key(|&p| projects[p].tab_order);
    members
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: u64, tab_order: u32) -> GroupKey {
        GroupKey { id, tab_order }
    }

    fn project(group: Option<u64>, tab_order: u32) -> ProjectKey {
        ProjectKey { group, tab_order }
    }

    use TopRow::{Group as G, UngroupedProject as P};

    /// A label, the groups and projects given, and the rows wanted.
    type Case = (&'static str, Vec<GroupKey>, Vec<ProjectKey>, Vec<TopRow>);

    #[test]
    fn rows_follow_the_sidebar_order() {
        let cases: Vec<Case> = vec![
            ("nothing", vec![], vec![], vec![]),
            (
                "ungrouped projects by tab order, not input order",
                vec![],
                vec![project(None, 2), project(None, 0), project(None, 1)],
                vec![P(1), P(2), P(0)],
            ),
            (
                "groups and ungrouped projects interleave in one pool",
                vec![group(7, 1), group(8, 3)],
                vec![project(None, 0), project(None, 2), project(None, 4)],
                vec![
                    P(0),
                    G {
                        group: 0,
                        members: vec![],
                    },
                    P(1),
                    G {
                        group: 1,
                        members: vec![],
                    },
                    P(2),
                ],
            ),
            (
                "a group's members follow their own tab order",
                vec![group(7, 0)],
                vec![
                    project(Some(7), 5),
                    project(Some(7), 1),
                    project(Some(7), 3),
                ],
                vec![G {
                    group: 0,
                    members: vec![1, 2, 0],
                }],
            ),
            (
                "a member's tab order does not place it among the top rows",
                vec![group(7, 5)],
                vec![project(Some(7), 0), project(None, 1)],
                vec![
                    P(1),
                    G {
                        group: 0,
                        members: vec![0],
                    },
                ],
            ),
            (
                "on a tie a group leads an ungrouped project, then input order",
                vec![group(8, 1), group(7, 1)],
                vec![project(None, 1), project(None, 1)],
                vec![
                    G {
                        group: 0,
                        members: vec![],
                    },
                    G {
                        group: 1,
                        members: vec![],
                    },
                    P(0),
                    P(1),
                ],
            ),
            (
                "tied members keep input order",
                vec![group(7, 0)],
                vec![project(Some(7), 2), project(Some(7), 2)],
                vec![G {
                    group: 0,
                    members: vec![0, 1],
                }],
            ),
            (
                "an empty group keeps its row",
                vec![group(7, 0), group(8, 1)],
                vec![project(Some(8), 0)],
                vec![
                    G {
                        group: 0,
                        members: vec![],
                    },
                    G {
                        group: 1,
                        members: vec![0],
                    },
                ],
            ),
            (
                "a project naming no known group is shown nowhere",
                vec![group(7, 0)],
                vec![project(Some(9), 0), project(None, 1)],
                vec![
                    G {
                        group: 0,
                        members: vec![],
                    },
                    P(1),
                ],
            ),
        ];
        for (label, groups, projects, want) in cases {
            assert_eq!(arrange(&groups, &projects), want, "{label}");
        }
    }
}
