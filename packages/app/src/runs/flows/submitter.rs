//! Who a flow episode, a start or a resume, answers to.

use daruda_flow::request::StartedBy;

/// The submitter of one flow episode. The run's first starter is
/// provenance, kept in its log; this is who the episode's questions and end
/// are owed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlowSubmitter {
    /// The person at this desktop: questions follow their presence, and the
    /// run's key keeps whatever solicitation it already had.
    Desktop,
    /// The phone asked, so it is always owed the answer.
    Phone,
    /// The orchestrator, solicited when a phone turn was open as it asked.
    Orchestrator { solicited: bool },
}

impl FlowSubmitter {
    /// The flow's record of who started it.
    pub(crate) fn started_by(self) -> StartedBy {
        match self {
            Self::Desktop => StartedBy::Desktop,
            Self::Phone => StartedBy::Phone,
            Self::Orchestrator { .. } => StartedBy::Orchestrator,
        }
    }

    /// Whether the episode's questions reach a remote channel regardless of
    /// where the person is.
    pub(crate) fn solicited(self) -> bool {
        match self {
            Self::Desktop => false,
            Self::Phone => true,
            Self::Orchestrator { solicited } => solicited,
        }
    }

    /// What the episode sets the run key's solicitation to; `None` keeps
    /// the key's, since a desktop episode brings none of its own.
    pub(crate) fn solicitation(self) -> Option<bool> {
        match self {
            Self::Desktop => None,
            Self::Phone | Self::Orchestrator { .. } => Some(self.solicited()),
        }
    }

    /// Whether the episode's end is answered on the phone that asked.
    pub(crate) fn owes_remote_answer(self) -> bool {
        matches!(self, Self::Phone)
    }

    /// The submitter of a task's flow, from who started the task and the
    /// solicitation that start captured.
    pub(crate) fn of_task(started_by: daruda_store::tasks::StartedBy, solicited: bool) -> Self {
        match started_by {
            daruda_store::tasks::StartedBy::Desktop => Self::Desktop,
            daruda_store::tasks::StartedBy::Phone => Self::Phone,
            daruda_store::tasks::StartedBy::Orchestrator => Self::Orchestrator { solicited },
        }
    }

    /// The task's record of who started it, for a task started for this
    /// submitter.
    pub(crate) fn task_started_by(self) -> daruda_store::tasks::StartedBy {
        match self {
            Self::Desktop => daruda_store::tasks::StartedBy::Desktop,
            Self::Phone => daruda_store::tasks::StartedBy::Phone,
            Self::Orchestrator { .. } => daruda_store::tasks::StartedBy::Orchestrator,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each submitter's three readings, pinned together: a swapped arm in
    /// one would split questions from results.
    #[test]
    fn each_submitter_reads_its_starter_and_solicitation() {
        let cases = [
            (
                FlowSubmitter::Desktop,
                StartedBy::Desktop,
                false,
                None,
                false,
            ),
            (
                FlowSubmitter::Phone,
                StartedBy::Phone,
                true,
                Some(true),
                true,
            ),
            (
                FlowSubmitter::Orchestrator { solicited: true },
                StartedBy::Orchestrator,
                true,
                Some(true),
                false,
            ),
            (
                FlowSubmitter::Orchestrator { solicited: false },
                StartedBy::Orchestrator,
                false,
                Some(false),
                false,
            ),
        ];
        for (submitter, started_by, solicited, solicitation, owes) in cases {
            assert_eq!(submitter.started_by(), started_by, "{submitter:?}");
            assert_eq!(submitter.solicited(), solicited, "{submitter:?}");
            assert_eq!(submitter.solicitation(), solicitation, "{submitter:?}");
            assert_eq!(submitter.owes_remote_answer(), owes, "{submitter:?}");
        }
    }

    #[test]
    fn a_tasks_flow_answers_to_who_started_the_task() {
        use daruda_store::tasks::StartedBy as Task;
        assert_eq!(
            FlowSubmitter::of_task(Task::Desktop, true),
            FlowSubmitter::Desktop
        );
        assert_eq!(
            FlowSubmitter::of_task(Task::Phone, false),
            FlowSubmitter::Phone
        );
        assert_eq!(
            FlowSubmitter::of_task(Task::Orchestrator, true),
            FlowSubmitter::Orchestrator { solicited: true }
        );
        for started_by in [Task::Desktop, Task::Phone, Task::Orchestrator] {
            for solicited in [true, false] {
                let submitter = FlowSubmitter::of_task(started_by, solicited);
                assert_eq!(submitter.task_started_by(), started_by, "{submitter:?}");
            }
        }
    }
}
