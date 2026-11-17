//! Request progress projected from the discovery slots and shared scheduler.

use super::list::{GroupBody, PageBody, PageInputs};
use crate::surface::strings::pull_requests as s;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum Activity {
    Idle,
    Waiting,
    Discovering,
    Loading,
    Refreshing,
}

impl Activity {
    pub(super) fn of(inputs: &PageInputs<'_>, body: &PageBody) -> Self {
        if !inputs.enabled {
            return Self::Idle;
        }
        if inputs.discovering {
            return Self::Discovering;
        }
        let PageBody::Groups(groups) = body else {
            return Self::Idle;
        };
        if inputs.refreshing {
            if groups.iter().any(|group| group.refreshed.is_some()) {
                Self::Refreshing
            } else {
                Self::Loading
            }
        } else if groups
            .iter()
            .any(|group| matches!(group.body, GroupBody::Loading))
        {
            Self::Waiting
        } else {
            Self::Idle
        }
    }

    pub(super) fn busy(self) -> bool {
        matches!(self, Self::Discovering | Self::Loading | Self::Refreshing)
    }

    pub(super) fn label(self) -> Option<String> {
        match self {
            Self::Idle => None,
            Self::Waiting => Some(s::waiting()),
            Self::Discovering => Some(s::discovering()),
            Self::Loading => Some(s::loading()),
            Self::Refreshing => Some(s::refreshing()),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) struct Sources {
    loaded: usize,
    total: usize,
    unavailable: usize,
}

impl Sources {
    pub(super) fn of(body: &PageBody) -> Self {
        let PageBody::Groups(groups) = body else {
            return Self::default();
        };
        Self {
            loaded: groups
                .iter()
                .filter(|group| group.refreshed.is_some())
                .count(),
            total: groups.len(),
            unavailable: groups
                .iter()
                .filter(|group| {
                    matches!(
                        group.body,
                        GroupBody::Blocked(_)
                            | GroupBody::Listed {
                                trouble: Some(_),
                                ..
                            }
                    )
                })
                .count(),
        }
    }

    pub(super) fn label(self) -> Option<String> {
        (self.total > 0).then(|| {
            if self.unavailable > 0 {
                s::sources_unavailable(self.loaded, self.total, self.unavailable)
            } else {
                s::sources_loaded(self.loaded, self.total)
            }
        })
    }
}

#[cfg(test)]
mod tests;
