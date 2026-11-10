//! What a pane thread's root says about the pane: the host reports the pane's
//! activity in this module's own terms, and the root shows one status for it.

use crate::surface::strings as s;

/// What the host reports about a pane. Owned here so the remote channel does
/// not depend on chat or workspace types; the workspace converts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaneActivity {
    /// A turn or its background work is running.
    Working,
    /// Running, and stopped on a permission or a question for the person.
    AwaitingYou,
    /// The pane's last turn ended this way.
    Ended(Ending),
    /// The pane is gone.
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ending {
    Completed,
    Failed,
    Stopped,
    /// daruda stopped the turn at the plan's usage limit.
    OverageStopped,
}

/// The status a root shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShownStatus {
    /// Unix seconds when the work began.
    Working {
        since: i64,
    },
    /// Paused on the person; `since` is when the work began.
    NeedsYou {
        since: i64,
    },
    Ended(Ending),
    Closed,
}

impl ShownStatus {
    /// The status for `activity`, given what the root showed before. Work keeps
    /// the time it began across a pause for the person's answer.
    pub(crate) fn next(previous: Option<ShownStatus>, activity: PaneActivity, now: i64) -> Self {
        match activity {
            PaneActivity::Working => Self::Working {
                since: Self::began(previous).unwrap_or(now),
            },
            PaneActivity::AwaitingYou => Self::NeedsYou {
                since: Self::began(previous).unwrap_or(now),
            },
            PaneActivity::Ended(ending) => Self::Ended(ending),
            PaneActivity::Closed => Self::Closed,
        }
    }

    /// Whether a stop can still act on the pane.
    pub(crate) fn is_running(self) -> bool {
        matches!(self, Self::Working { .. } | Self::NeedsYou { .. })
    }

    fn began(previous: Option<ShownStatus>) -> Option<i64> {
        match previous? {
            Self::Working { since } | Self::NeedsYou { since } => Some(since),
            Self::Ended(_) | Self::Closed => None,
        }
    }

    /// The Slack emoji name and the words the root shows. Emoji names, not
    /// characters: a character such as U+23F9 renders as a plain glyph
    /// without a variation selector, while a name renders as Slack's art.
    pub(crate) fn view(self) -> (&'static str, String) {
        match self {
            Self::Working { since } => (
                "hourglass_flowing_sand",
                s::remote_channel::thread_status_working(
                    chrono::DateTime::from_timestamp(since, 0).unwrap_or_default(),
                ),
            ),
            Self::NeedsYou { .. } => ("bell", s::remote_channel::thread_status_needs_you()),
            Self::Ended(Ending::Completed) => (
                "white_check_mark",
                s::remote_channel::thread_status_completed(),
            ),
            Self::Ended(Ending::Failed) => ("x", s::remote_channel::thread_status_failed()),
            Self::Ended(Ending::Stopped) => {
                ("octagonal_sign", s::remote_channel::thread_status_stopped())
            }
            Self::Ended(Ending::OverageStopped) => {
                ("credit_card", s::remote_channel::thread_status_overage())
            }
            Self::Closed => ("door", s::remote_channel::thread_status_closed()),
        }
    }
}
