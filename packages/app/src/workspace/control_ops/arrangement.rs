//! A window's agent panes arranged as the project sidebar arranges their
//! projects: groups and ungrouped projects in its order, each project's
//! lanes in tab order. Only what holds a pane is kept. GPUI-free.

/// One top-level entry: a group with its projects, or a project in none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ArrangedSection<T> {
    Group {
        name: String,
        projects: Vec<ArrangedProject<T>>,
    },
    Project(ArrangedProject<T>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArrangedProject<T> {
    pub name: String,
    pub lanes: Vec<ArrangedLane<T>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArrangedLane<T> {
    pub name: String,
    pub panes: Vec<T>,
}

impl<T> ArrangedSection<T> {
    /// The section's projects, in order: one for an ungrouped project.
    pub(crate) fn projects(&self) -> &[ArrangedProject<T>] {
        match self {
            ArrangedSection::Group { projects, .. } => projects,
            ArrangedSection::Project(project) => std::slice::from_ref(project),
        }
    }

    /// The same arrangement with every pane converted, in order, by `f`,
    /// which is given the names of the pane's project and lane too.
    pub(crate) fn map<U>(self, f: &mut impl FnMut(&str, &str, T) -> U) -> ArrangedSection<U> {
        match self {
            ArrangedSection::Group { name, projects } => ArrangedSection::Group {
                name,
                projects: projects.into_iter().map(|p| p.map(f)).collect(),
            },
            ArrangedSection::Project(project) => ArrangedSection::Project(project.map(f)),
        }
    }
}

impl<T> ArrangedProject<T> {
    fn map<U>(self, f: &mut impl FnMut(&str, &str, T) -> U) -> ArrangedProject<U> {
        let lanes = self
            .lanes
            .into_iter()
            .map(|lane| ArrangedLane {
                panes: lane
                    .panes
                    .into_iter()
                    .map(|pane| f(&self.name, &lane.name, pane))
                    .collect(),
                name: lane.name,
            })
            .collect();
        ArrangedProject {
            name: self.name,
            lanes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str, lanes: &[(&str, &[u32])]) -> ArrangedProject<u32> {
        ArrangedProject {
            name: name.into(),
            lanes: lanes
                .iter()
                .map(|(name, panes)| ArrangedLane {
                    name: (*name).into(),
                    panes: panes.to_vec(),
                })
                .collect(),
        }
    }

    /// Mapping keeps every name and the order of every pane; a section's
    /// projects are its members, or the one project it is.
    #[test]
    fn mapping_keeps_the_arrangement() {
        let group = ArrangedSection::Group {
            name: "g".into(),
            projects: vec![
                project("a", &[("main", &[1, 2])]),
                project("b", &[("x", &[3])]),
            ],
        };
        let mut seen = Vec::new();
        let mapped = group.map(&mut |project, lane, pane| {
            seen.push(format!("{project}/{lane}/{pane}"));
            pane * 10
        });
        assert_eq!(seen, ["a/main/1", "a/main/2", "b/x/3"]);
        assert_eq!(
            mapped,
            ArrangedSection::Group {
                name: "g".into(),
                projects: vec![
                    project("a", &[("main", &[10, 20])]),
                    project("b", &[("x", &[30])])
                ],
            }
        );
        let alone = ArrangedSection::Project(project("c", &[]));
        assert_eq!(alone.projects().len(), 1);
        assert_eq!(mapped.projects().len(), 2);
    }
}
