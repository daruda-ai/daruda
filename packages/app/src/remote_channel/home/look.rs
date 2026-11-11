//! How a board reads on the page: every text in the person's language, held
//! so the transport can borrow it. One builder for every publish.

use super::board::{Board, Part};
use super::token::Act;
use crate::remote_channel::threads::status::ShownStatus;
use crate::remote_channel::transport::{PageHelp, PagePart, PageRow, PageView};
use crate::surface::strings as s;

pub(super) struct HomeLook {
    header: String,
    summary: Option<String>,
    outcome: Option<String>,
    empty: String,
    parts: Vec<PartLook>,
    rows: Vec<RowLook>,
    more: Option<String>,
    open_thread: String,
    select: String,
    stop: String,
    help_title: String,
    help_lines: Vec<String>,
    legend: Vec<(&'static str, String)>,
}

/// One piece of the listing in words; rows by index into `rows`.
enum PartLook {
    Heading(String),
    Label(String),
    Rows {
        rows: Vec<usize>,
        /// The strip's cards sit under no project heading, so they name
        /// their project themselves.
        names_project: bool,
    },
}

struct RowLook {
    /// The conversation's title, else the tab's name, else the agent's.
    title: String,
    /// The agent and tab name, omitting either if it already is the title.
    subtitle: Option<String>,
    /// The subtitle under no project heading: the project, then the pane's.
    strip_subtitle: String,
    emoji: Option<&'static str>,
    status: String,
    link: Option<String>,
    select: String,
    /// Only a running pane has something to stop.
    stop: Option<String>,
}

impl HomeLook {
    pub(super) fn new(board: &Board) -> Self {
        let counts = board.counts();
        let select = s::remote_channel::home_select();
        Self {
            header: s::remote_channel::home_header(),
            summary: (counts.total > 0).then(|| {
                s::remote_channel::home_summary(counts.working, counts.needs_you, counts.total)
            }),
            outcome: board.outcome().map(|outcome| {
                s::remote_channel::home_result(
                    chrono::DateTime::from_timestamp(outcome.at, 0).unwrap_or_default(),
                    &outcome.text,
                )
            }),
            empty: if counts.total == 0 {
                s::control::listing_empty()
            } else {
                s::remote_channel::home_empty_omitted()
            },
            parts: Self::parts_of(board),
            rows: board
                .rows()
                .iter()
                .enumerate()
                .map(|(index, row)| {
                    let (emoji, status) = match row.shown.map(ShownStatus::view) {
                        Some((emoji, label)) => (Some(emoji), label),
                        None => (None, s::remote_channel::home_status_idle()),
                    };
                    let title = row
                        .subject
                        .clone()
                        .or_else(|| row.tab_name.clone())
                        .unwrap_or_else(|| row.agent.clone());
                    let agent = (title != row.agent).then_some(row.agent.as_str());
                    let tab = row
                        .tab_name
                        .as_deref()
                        .filter(|tab| *tab != title && *tab != row.agent);
                    let subtitle = match (agent, tab) {
                        (Some(agent), Some(tab)) => Some(s::control::agent_with_tab(agent, tab)),
                        (Some(agent), None) => Some(agent.to_owned()),
                        (None, Some(tab)) => Some(tab.to_owned()),
                        (None, None) => None,
                    };
                    RowLook {
                        strip_subtitle: subtitle.iter().fold(row.project.clone(), |text, agent| {
                            format!(
                                "{text}{}{agent}",
                                s::remote_channel::thread_name_separator()
                            )
                        }),
                        subtitle,
                        title,
                        emoji,
                        status: if row.selected {
                            s::remote_channel::home_status_selected(status)
                        } else {
                            status
                        },
                        link: row.link.clone(),
                        select: board.token(index, Act::Select),
                        stop: row
                            .shown
                            .is_some_and(ShownStatus::is_running)
                            .then(|| board.token(index, Act::Stop)),
                    }
                })
                .collect(),
            more: (board.omitted() > 0).then(|| s::remote_channel::home_more(board.omitted())),
            open_thread: s::remote_channel::home_open_thread(),
            help_title: s::remote_channel::home_help_title(),
            help_lines: vec![
                s::remote_channel::home_help_thread(),
                s::remote_channel::home_help_select(&select),
                s::remote_channel::home_help_commands(),
            ],
            legend: ShownStatus::legend(),
            select,
            stop: s::remote_channel::home_stop(),
        }
    }

    fn parts_of(board: &Board) -> Vec<PartLook> {
        let mut strip = false;
        board
            .parts()
            .into_iter()
            .map(|part| match part {
                Part::NeedsYou => {
                    strip = true;
                    PartLook::Heading(s::remote_channel::home_needs_you())
                }
                Part::Window(number) => PartLook::Heading(s::remote_channel::home_window(number)),
                Part::Group(name) | Part::Project(name) => PartLook::Heading(name.to_owned()),
                Part::GroupedProject(name) => PartLook::Label(name.to_owned()),
                Part::Lane { project, lane } => {
                    PartLook::Label(s::control::lane_path(project, lane))
                }
                Part::Rows(rows) => PartLook::Rows {
                    rows: rows.to_vec(),
                    names_project: std::mem::take(&mut strip),
                },
            })
            .collect()
    }

    pub(super) fn view(&self) -> PageView<'_> {
        let parts = self
            .parts
            .iter()
            .map(|part| match part {
                PartLook::Heading(text) => PagePart::Heading(text),
                PartLook::Label(text) => PagePart::Label(text),
                PartLook::Rows {
                    rows,
                    names_project,
                } => PagePart::Rows(
                    rows.iter()
                        .map(|&index| self.row(&self.rows[index], *names_project))
                        .collect(),
                ),
            })
            .collect();
        PageView {
            header: &self.header,
            summary: self.summary.as_deref(),
            outcome: self.outcome.as_deref(),
            empty: &self.empty,
            parts,
            more: self.more.as_deref(),
            help: PageHelp {
                title: &self.help_title,
                lines: self.help_lines.iter().map(String::as_str).collect(),
                legend: self
                    .legend
                    .iter()
                    .map(|(emoji, label)| (*emoji, label.as_str()))
                    .collect(),
            },
        }
    }

    fn row<'a>(&'a self, row: &'a RowLook, names_project: bool) -> PageRow<'a> {
        PageRow {
            title: &row.title,
            subtitle: if names_project {
                Some(&row.strip_subtitle)
            } else {
                row.subtitle.as_deref()
            },
            emoji: row.emoji,
            status: &row.status,
            thread: row
                .link
                .as_deref()
                .map(|link| (self.open_thread.as_str(), link)),
            select: (self.select.as_str(), row.select.as_str()),
            stop: row.stop.as_deref().map(|token| (self.stop.as_str(), token)),
        }
    }
}
