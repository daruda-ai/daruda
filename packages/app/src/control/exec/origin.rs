//! Control-entry provenance converted at each domain boundary.

/// Where a control command entered the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandOrigin {
    Phone,
    Orchestrator,
}

impl CommandOrigin {
    /// The caller this command answers to. Read as the command enters,
    /// before any card or async work: an orchestrator command is solicited
    /// while a phone turn is open, and that turn may close before it ends.
    pub(crate) fn capture(self, cx: &gpui::App) -> Caller {
        match self {
            Self::Phone => Caller::Phone,
            Self::Orchestrator => Caller::Orchestrator {
                solicited: crate::window_registry::WindowRegistry::orchestrator(cx)
                    .and_then(|(_, workspace)| workspace.upgrade())
                    .is_some_and(|workspace| {
                        let workspace = workspace.read(cx);
                        workspace
                            .orchestrator_chat_pane()
                            .is_some_and(|pane| workspace.phone_turn_open(pane.pane, cx))
                    }),
            },
        }
    }
}

/// Who a control command answers to: the phone, which always asked, or the
/// orchestrator, solicited when the phone asked it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Caller {
    Phone,
    Orchestrator { solicited: bool },
}

impl Caller {
    pub(crate) fn origin(self) -> CommandOrigin {
        match self {
            Self::Phone => CommandOrigin::Phone,
            Self::Orchestrator { .. } => CommandOrigin::Orchestrator,
        }
    }

    pub(crate) fn solicited(self) -> bool {
        match self {
            Self::Phone => true,
            Self::Orchestrator { solicited } => solicited,
        }
    }

    pub(crate) fn flow_submitter(self) -> crate::runs::FlowSubmitter {
        match self {
            Self::Phone => crate::runs::FlowSubmitter::Phone,
            Self::Orchestrator { solicited } => {
                crate::runs::FlowSubmitter::Orchestrator { solicited }
            }
        }
    }

    /// How a card or announcement for this caller reaches remote channels.
    pub(crate) fn delivery(self, away: bool) -> crate::remote_channel::global::Delivery {
        crate::remote_channel::delivery::remote_delivery(self.solicited(), away)
    }

    /// Put the run `key` names in the orchestrator's scope, owed with this
    /// caller's solicitation. The phone keeps no scope.
    pub(crate) fn scope_run(self, key: crate::runs::result::RunKey, cx: &mut gpui::App) {
        if let Self::Orchestrator { solicited } = self {
            crate::runs::RunsHost::register_run_solicited(key, solicited, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Caller;

    /// The caller and the submitter its runs answer to are separate enums
    /// with matching variants: a swapped arm would compile, so it is pinned.
    #[test]
    fn every_caller_submits_as_itself_with_its_solicitation() {
        use crate::runs::FlowSubmitter;
        let orchestrator = |solicited| Caller::Orchestrator { solicited };
        assert_eq!(Caller::Phone.flow_submitter(), FlowSubmitter::Phone);
        for solicited in [true, false] {
            assert_eq!(
                orchestrator(solicited).flow_submitter(),
                FlowSubmitter::Orchestrator { solicited }
            );
            assert_eq!(orchestrator(solicited).solicited(), solicited);
        }
        assert!(Caller::Phone.solicited());
    }
}
