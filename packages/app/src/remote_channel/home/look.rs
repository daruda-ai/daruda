//! How a board reads on the page: every text in the person's language, held
//! so the transport can borrow it. One builder for every publish.

use super::board::Board;
use super::token::Act;
use crate::remote_channel::threads::status::ShownStatus;
use crate::remote_channel::transport::{PageGroup, PageHelp, PageRow, PageView};
use crate::surface::strings as s;

pub(super) struct HomeLook {
    header: String,
    summary: Option<String>,
    outcome: Option<String>,
    empty: String,
    running_title: String,
    others_title: String,
    rows: Vec<RowLook>,
    more: Option<String>,
    open_thread: String,
    select: String,
    stop: String,
    help_title: String,
    help_lines: Vec<String>,
    legend: Vec<(&'static str, String)>,
}

struct RowLook {
    name: String,
    subject: Option<String>,
    emoji: Option<&'static str>,
    status: String,
    link: Option<String>,
    select: String,
    /// Only a running pane has something to stop, and only it is grouped
    /// under the running heading.
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
            empty: s::control::listing_empty(),
            running_title: s::remote_channel::home_group_running(),
            others_title: s::remote_channel::home_group_others(),
            rows: board
                .rows()
                .iter()
                .enumerate()
                .map(|(index, row)| {
                    let (emoji, status) = match row.shown.map(ShownStatus::view) {
                        Some((emoji, label)) => (Some(emoji), label),
                        None => (None, s::remote_channel::home_status_idle()),
                    };
                    RowLook {
                        name: row.name.clone(),
                        subject: row.subject.clone(),
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

    pub(super) fn view(&self) -> PageView<'_> {
        let (running, others): (Vec<_>, Vec<_>) =
            self.rows.iter().partition(|row| row.stop.is_some());
        let groups = [
            (self.running_title.as_str(), running),
            (self.others_title.as_str(), others),
        ]
        .into_iter()
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(title, rows)| PageGroup {
            title,
            rows: rows.into_iter().map(|row| self.row(row)).collect(),
        })
        .collect();
        PageView {
            header: &self.header,
            summary: self.summary.as_deref(),
            outcome: self.outcome.as_deref(),
            empty: &self.empty,
            groups,
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

    fn row<'a>(&'a self, row: &'a RowLook) -> PageRow<'a> {
        PageRow {
            name: &row.name,
            subject: row.subject.as_deref(),
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
