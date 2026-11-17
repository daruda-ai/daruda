//! How the Pull requests list lays out its rows for the width between the
//! docks: a table whose column headings stay above the scrolling rows, or,
//! where the columns do not fit, compact rows that stack the side facts
//! under the title instead of scrolling sideways.

use crate::ui::list_table::{Column, ListTable};
use crate::ui::theme::{self, list_metrics as metrics};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workspace) enum PrColumn {
    Tone,
    Title,
    Checks,
    Review,
    Comments,
    Lane,
    Open,
}

/// The table's columns.
pub(in crate::workspace) fn table() -> ListTable<PrColumn> {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workspace) enum RowLayout {
    /// Every column fits: headings sit in the fixed page header.
    Table,
    /// Too narrow for the columns: one stacked row per pull request.
    Compact,
}

impl RowLayout {
    /// The layout for `content`, the width the page has between the docks.
    /// The rows sit inside the page's padding on both sides.
    pub(in crate::workspace) fn for_width(content: f32) -> Self {
        if content >= table().min_width() + 2.0 * theme::DOCK_PAGE_PAD {
            Self::Table
        } else {
            Self::Compact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_stack_exactly_when_the_columns_no_longer_fit() {
        let fits = table().min_width() + 2.0 * theme::DOCK_PAGE_PAD;
        // (label, content width, layout)
        let cases = [
            (
                "wide content keeps the table",
                fits + 400.0,
                RowLayout::Table,
            ),
            ("the exact fit keeps the table", fits, RowLayout::Table),
            ("one pixel short stacks", fits - 1.0, RowLayout::Compact),
            ("no space stacks", 0.0, RowLayout::Compact),
        ];
        for (label, width, layout) in cases {
            assert_eq!(RowLayout::for_width(width), layout, "{label}");
        }
    }
}
