//! What a page lists: the panes needing the person in a strip on top, then
//! every pane in the project sidebar's arrangement, cut at a project's end
//! to what the page has room for. GPUI-free.

use super::token::{Act, Numbering, Tap};
use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::threads::status::ShownStatus;
use crate::remote_channel::transport::PageBudget;
use crate::workspace::{ArrangedLane, ArrangedProject, ArrangedSection};

/// One pane on the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PaneRow {
    pub pane: PaneRef,
    /// The project the pane's worktree belongs to.
    pub project: String,
    /// The agent running the pane.
    pub agent: String,
    /// The name the person gave the pane's tab.
    pub tab_name: Option<String>,
    /// What the pane is working on, as its activity bar says.
    pub subject: Option<String>,
    /// `None` while the pane has not reported since daruda started.
    pub shown: Option<ShownStatus>,
    /// The root of the pane's thread on this connection, when it has one.
    pub thread: Option<String>,
    /// The link that opens that thread, once known.
    pub link: Option<String>,
    /// The pane the connection's commands address.
    pub selected: bool,
}

/// One window's panes, arranged as its sidebar arranges their projects.
pub(super) struct WindowPanes {
    /// The window's place among every window, from 1.
    pub number: usize,
    pub sections: Vec<ArrangedSection<PaneRow>>,
}

/// One window's shown panes, each by its index into the board's rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BoardWindow {
    /// Named only when more than one window has panes.
    pub number: Option<usize>,
    pub sections: Vec<ArrangedSection<usize>>,
}

/// One piece of the listing, top to bottom, rows by index into the board's
/// rows. What the page draws and what its room is counted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Part<'a> {
    /// Heads the strip of panes needing the person.
    NeedsYou,
    /// Heads a window's sections.
    Window(usize),
    /// Heads a group's projects.
    Group(&'a str),
    /// Heads a project in no group.
    Project(&'a str),
    /// Names a project in a group whose lanes are not named one by one.
    GroupedProject(&'a str),
    /// Names a lane, in a project with panes in more than one.
    Lane {
        project: &'a str,
        lane: &'a str,
    },
    Rows(&'a [usize]),
}

impl Part<'_> {
    fn blocks(&self, budget: &PageBudget) -> usize {
        match self {
            Part::Rows(rows) => budget.rows_blocks(rows.len()),
            _ => 1,
        }
    }

    fn cards(&self) -> usize {
        match self {
            Part::Rows(rows) => rows.len(),
            _ => 0,
        }
    }
}

/// What the person's last tap on the page came to, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Outcome {
    /// Unix seconds.
    pub at: i64,
    pub text: String,
}

/// How many panes are open and how many of them run, the ones a page has no
/// room for included.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Counts {
    pub working: usize,
    pub needs_you: usize,
    pub total: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Board {
    /// Every pane shown, once each: the shown projects' panes in order,
    /// then the strip's panes whose project was left out. A row's index is
    /// what its buttons carry, wherever it shows.
    rows: Vec<PaneRow>,
    /// The panes needing the person, by row index, in sidebar order.
    needs_you: Vec<usize>,
    windows: Vec<BoardWindow>,
    counts: Counts,
    /// Panes shown nowhere for want of room.
    omitted: usize,
    /// What the rows' buttons carry, so a tap names this board's row.
    generation: u64,
    outcome: Option<Outcome>,
}

impl Board {
    /// `windows`' panes, in the sidebar's arrangement, laid out for a page
    /// of `budget`, under what the person's last tap came to.
    ///
    /// The strip takes its room first, cut to what fits when it alone
    /// overflows. Projects follow in order, each with the headings it opens
    /// (its window's, its group's), until one does not fit: it and every
    /// later project are left out whole, so a project never shows in part.
    pub(super) fn new(
        windows: Vec<WindowPanes>,
        budget: PageBudget,
        outcome: Option<Outcome>,
    ) -> Self {
        // Every pane once, in order, and the arrangement by index into them.
        let mut all: Vec<PaneRow> = Vec::new();
        let arranged: Vec<(usize, Vec<ArrangedSection<usize>>)> = windows
            .into_iter()
            .map(|window| {
                let sections: Vec<ArrangedSection<usize>> = window
                    .sections
                    .into_iter()
                    .filter_map(|section| {
                        pruned(section.map(&mut |_, _, row| {
                            all.push(row);
                            all.len() - 1
                        }))
                    })
                    .collect();
                (window.number, sections)
            })
            .filter(|(_, sections)| !sections.is_empty())
            .collect();
        let counts = Counts {
            working: all
                .iter()
                .filter(|row| matches!(row.shown, Some(ShownStatus::Working { .. })))
                .count(),
            needs_you: all
                .iter()
                .filter(|row| matches!(row.shown, Some(ShownStatus::NeedsYou { .. })))
                .count(),
            total: all.len(),
        };
        let numbered = arranged.len() > 1;

        let mut room = Room {
            budget,
            blocks: 0,
            cards: 0,
        };
        let mut needs_you: Vec<usize> = (0..all.len())
            .filter(|&i| matches!(all[i].shown, Some(ShownStatus::NeedsYou { .. })))
            .collect();
        if !needs_you.is_empty() {
            let fit = budget
                .blocks
                .saturating_sub(1)
                .saturating_mul(budget.cards_per_block)
                .min(budget.cards);
            needs_you.truncate(fit);
            if !needs_you.is_empty() {
                room.take(&[Part::NeedsYou, Part::Rows(&needs_you)]);
            }
        }
        let mut shown: Vec<BoardWindow> = Vec::new();
        'windows: for (number, sections) in &arranged {
            let mut window = BoardWindow {
                number: numbered.then_some(*number),
                sections: Vec::new(),
            };
            for section in sections {
                let group = match section {
                    ArrangedSection::Group { name, .. } => Some(name.as_str()),
                    ArrangedSection::Project(_) => None,
                };
                let mut kept: Vec<ArrangedProject<usize>> = Vec::new();
                for project in section.projects() {
                    let mut parts = Vec::new();
                    project_parts(
                        &mut parts,
                        window
                            .number
                            .filter(|_| window.sections.is_empty() && kept.is_empty()),
                        group.filter(|_| kept.is_empty()),
                        project,
                        group.is_some(),
                    );
                    if !room.fits(&parts) {
                        window.sections.extend(section_of(group, kept));
                        if !window.sections.is_empty() {
                            shown.push(window);
                        }
                        break 'windows;
                    }
                    room.take(&parts);
                    kept.push(project.clone());
                }
                window.sections.extend(section_of(group, kept));
            }
            shown.push(window);
        }

        // The rows the page shows, renumbered in the order they first show
        // in the projects, then the strip's panes shown only there.
        let mut order: Vec<usize> = shown
            .iter()
            .flat_map(|window| &window.sections)
            .flat_map(|section| section.projects())
            .flat_map(|project| &project.lanes)
            .flat_map(|lane| lane.panes.iter().copied())
            .collect();
        let mut index_of = vec![None; all.len()];
        for (new, &old) in order.iter().enumerate() {
            index_of[old] = Some(new);
        }
        for &old in &needs_you {
            if index_of[old].is_none() {
                index_of[old] = Some(order.len());
                order.push(old);
            }
        }
        let renumber = |old: usize| index_of[old].expect("every shown pane has a row");
        let needs_you = needs_you.into_iter().map(renumber).collect();
        let windows = shown
            .into_iter()
            .map(|window| BoardWindow {
                number: window.number,
                sections: window
                    .sections
                    .into_iter()
                    .map(|section| section.map(&mut |_, _, old| renumber(old)))
                    .collect(),
            })
            .collect();
        let mut all: Vec<Option<PaneRow>> = all.into_iter().map(Some).collect();
        let rows: Vec<PaneRow> = order
            .iter()
            .map(|&old| all[old].take().expect("each pane once"))
            .collect();
        Self {
            omitted: counts.total - rows.len(),
            rows,
            needs_you,
            windows,
            counts,
            generation: 0,
            outcome,
        }
    }

    /// The listing piece by piece, top to bottom: what the page draws.
    pub(super) fn parts(&self) -> Vec<Part<'_>> {
        let mut parts = Vec::new();
        if !self.needs_you.is_empty() {
            parts.push(Part::NeedsYou);
            parts.push(Part::Rows(&self.needs_you));
        }
        for window in &self.windows {
            let mut window_heading = window.number;
            for section in &window.sections {
                let mut group = match section {
                    ArrangedSection::Group { name, .. } => Some(name.as_str()),
                    ArrangedSection::Project(_) => None,
                };
                let grouped = group.is_some();
                for project in section.projects() {
                    project_parts(
                        &mut parts,
                        window_heading.take(),
                        group.take(),
                        project,
                        grouped,
                    );
                }
            }
        }
        parts
    }

    /// The blocks and cards the listing takes, by `budget`'s count.
    #[cfg(test)]
    pub(super) fn room_taken(&self, budget: PageBudget) -> (usize, usize) {
        let mut room = Room {
            budget,
            blocks: 0,
            cards: 0,
        };
        room.take(&self.parts());
        (room.blocks, room.cards)
    }

    pub(super) fn rows(&self) -> &[PaneRow] {
        &self.rows
    }

    pub(super) fn counts(&self) -> Counts {
        self.counts
    }

    /// Panes left out for want of room.
    pub(super) fn omitted(&self) -> usize {
        self.omitted
    }

    /// What the person's last tap came to, shown atop the rows.
    pub(super) fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    #[cfg(test)]
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// Give the rows' buttons the generation `numbering` has for them.
    pub(super) fn number(&mut self, numbering: &mut Numbering) {
        self.generation = numbering.number(self.rows.iter().map(|row| row.pane).collect());
    }

    /// The value of row `index`'s button for `act`.
    pub(super) fn token(&self, index: usize, act: Act) -> String {
        Tap {
            generation: self.generation,
            index,
            act,
        }
        .token()
    }

    /// The pane `tap` names, when it was made for this board.
    pub(super) fn resolve(&self, tap: Tap) -> Option<PaneRef> {
        (tap.generation == self.generation)
            .then(|| self.rows.get(tap.index).map(|row| row.pane))
            .flatten()
    }

    /// The thread roots shown without a link yet.
    pub(super) fn unlinked(&self) -> impl Iterator<Item = &str> {
        self.rows
            .iter()
            .filter(|row| row.link.is_none())
            .filter_map(|row| row.thread.as_deref())
    }

    /// Give each row the link `link_of` knows for its thread.
    pub(super) fn link(&mut self, link_of: impl Fn(&str) -> Option<String>) {
        for row in &mut self.rows {
            if row.link.is_none() {
                row.link = row.thread.as_deref().and_then(&link_of);
            }
        }
    }
}

/// What a page has taken of its budget so far.
struct Room {
    budget: PageBudget,
    blocks: usize,
    cards: usize,
}

impl Room {
    fn fits(&self, parts: &[Part<'_>]) -> bool {
        let (blocks, cards) = Self::cost(parts, &self.budget);
        self.blocks + blocks <= self.budget.blocks && self.cards + cards <= self.budget.cards
    }

    fn take(&mut self, parts: &[Part<'_>]) {
        let (blocks, cards) = Self::cost(parts, &self.budget);
        self.blocks += blocks;
        self.cards += cards;
    }

    fn cost(parts: &[Part<'_>], budget: &PageBudget) -> (usize, usize) {
        parts.iter().fold((0, 0), |(blocks, cards), part| {
            (blocks + part.blocks(budget), cards + part.cards())
        })
    }
}

/// The parts one project shows: the window and group headings it opens,
/// when it is their first, then its own heading or label, then each lane's
/// rows. A project with panes in more than one lane names each lane; in a
/// group, those lane labels stand in for the project's own.
fn project_parts<'a>(
    parts: &mut Vec<Part<'a>>,
    window: Option<usize>,
    group: Option<&'a str>,
    project: &'a ArrangedProject<usize>,
    grouped: bool,
) {
    parts.extend(window.map(Part::Window));
    parts.extend(group.map(Part::Group));
    let lanes_named = project.lanes.len() > 1;
    if !grouped {
        parts.push(Part::Project(&project.name));
    } else if !lanes_named {
        parts.push(Part::GroupedProject(&project.name));
    }
    for lane in &project.lanes {
        if lanes_named {
            parts.push(Part::Lane {
                project: &project.name,
                lane: &lane.name,
            });
        }
        parts.push(Part::Rows(&lane.panes));
    }
}

/// The section `kept` makes under `group`, when it kept any project.
fn section_of(
    group: Option<&str>,
    mut kept: Vec<ArrangedProject<usize>>,
) -> Option<ArrangedSection<usize>> {
    match group {
        _ if kept.is_empty() => None,
        Some(name) => Some(ArrangedSection::Group {
            name: name.to_owned(),
            projects: kept,
        }),
        None => kept.pop().map(ArrangedSection::Project),
    }
}

/// `section` without its empty lanes and projects; `None` when nothing is
/// left.
fn pruned<T>(section: ArrangedSection<T>) -> Option<ArrangedSection<T>> {
    let keep = |project: ArrangedProject<T>| {
        let lanes: Vec<ArrangedLane<T>> = project
            .lanes
            .into_iter()
            .filter(|lane| !lane.panes.is_empty())
            .collect();
        (!lanes.is_empty()).then_some(ArrangedProject {
            name: project.name,
            lanes,
        })
    };
    match section {
        ArrangedSection::Group { name, projects } => {
            let projects: Vec<_> = projects.into_iter().filter_map(keep).collect();
            (!projects.is_empty()).then_some(ArrangedSection::Group { name, projects })
        }
        ArrangedSection::Project(project) => keep(project).map(ArrangedSection::Project),
    }
}
