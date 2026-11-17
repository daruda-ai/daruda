//! Whether the Pull requests page shows its list and an open detail side by
//! side, or one at a time. Side by side keeps the list's search, selection
//! and place while pull requests open one after another.

use crate::ui::theme;

/// What the page draws for the width between the docks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workspace) enum PageLayout {
    /// No detail is open.
    List,
    /// A detail is open and the width holds only it.
    Detail,
    /// A detail is open beside the list.
    Split,
}

/// Whether the list sits beside an open detail where the width allows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workspace) enum ListColumn {
    #[default]
    Beside,
    /// The person chose to see the detail alone.
    Hidden,
}

/// The list column's width for `content`: the `preferred` width, narrowed
/// while the window cannot hold it beside the narrowest detail. Narrowing
/// is for this frame only; the preference stays.
pub(in crate::workspace) fn list_width(content: f32, preferred: f32) -> f32 {
    let room = (content - theme::PR_SPLIT_DETAIL_MIN_W).max(theme::PR_SPLIT_LIST_MIN_W);
    preferred.clamp(theme::PR_SPLIT_LIST_MIN_W, room)
}

impl PageLayout {
    /// Whether `content`, the width between the docks, holds the list
    /// column at its narrowest beside a detail at its narrowest.
    pub(in crate::workspace) fn split_fits(content: f32) -> bool {
        content >= theme::PR_SPLIT_LIST_MIN_W + theme::PR_SPLIT_DETAIL_MIN_W
    }

    pub(in crate::workspace) fn of(detail_shown: bool, split_fits: bool) -> Self {
        match (detail_shown, split_fits) {
            (false, _) => Self::List,
            (true, false) => Self::Detail,
            (true, true) => Self::Split,
        }
    }

    /// Whether the list is drawn, and so can hold the keyboard.
    pub(in crate::workspace) fn shows_list(self) -> bool {
        self != Self::Detail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_detail_sits_beside_the_list_only_where_both_fit() {
        let fits = theme::PR_SPLIT_LIST_MIN_W + theme::PR_SPLIT_DETAIL_MIN_W;
        // (label, detail shown, content width, layout)
        let cases = [
            ("no detail", false, fits, PageLayout::List),
            ("no detail, narrow", false, 0.0, PageLayout::List),
            ("room for both", true, fits, PageLayout::Split),
            ("one pixel short", true, fits - 1.0, PageLayout::Detail),
        ];
        for (label, detail, width, layout) in cases {
            assert_eq!(
                PageLayout::of(detail, PageLayout::split_fits(width)),
                layout,
                "{label}"
            );
        }
        assert!(!PageLayout::Detail.shows_list());
        assert!(PageLayout::Split.shows_list() && PageLayout::List.shows_list());
    }

    #[test]
    fn the_list_narrows_for_the_window_without_going_below_its_minimum() {
        let detail = theme::PR_SPLIT_DETAIL_MIN_W;
        let min = theme::PR_SPLIT_LIST_MIN_W;
        // (label, content, preferred, width)
        let cases = [
            ("room for the preference", 1600.0, 500.0, 500.0),
            (
                "narrowed to leave the detail its minimum",
                detail + 400.0,
                500.0,
                400.0,
            ),
            ("never below the minimum", detail + 100.0, 500.0, min),
            (
                "a preference under the minimum is raised",
                1600.0,
                100.0,
                min,
            ),
        ];
        for (label, content, preferred, width) in cases {
            assert_eq!(list_width(content, preferred), width, "{label}");
        }
    }
}
