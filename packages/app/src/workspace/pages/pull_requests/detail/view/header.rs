//! The header: the pull request's state, what can be asked of GitHub, and
//! each action's slot.

use super::*;

/// Whether Send to agent has somewhere to go and something to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum SendSlot {
    /// No worktree works on the pull request, so no agent can take it.
    NoLane,
    /// Nothing failing, or no open thread.
    Nothing,
    /// Closed or merged: its failing checks and open threads are no
    /// longer anyone's to fix.
    NotOpen,
    Sending,
    Ready,
}

impl SendSlot {
    pub(in crate::workspace) fn enabled(self) -> bool {
        self == Self::Ready
    }
}

/// Whether an action on the pull request can be pressed, and why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum ActionSlot {
    Ready,
    /// This action is running.
    Running,
    /// Another action on the same pull request is running.
    Waiting,
    /// Said in its tooltip.
    Blocked(String),
}

impl ActionSlot {
    /// `mine` running, another running, or neither and `blocked` or not.
    pub(super) fn of(state: &ActionState, mine: PrAction, blocked: Option<String>) -> Self {
        match state.running() {
            None => blocked.map_or(Self::Ready, Self::Blocked),
            Some(running) if running == mine => Self::Running,
            Some(_) => Self::Waiting,
        }
    }

    pub(in crate::workspace) fn enabled(&self) -> bool {
        *self == Self::Ready
    }

    pub(in crate::workspace) fn reason(&self) -> Option<String> {
        match self {
            Self::Blocked(reason) => Some(reason.clone()),
            Self::Ready | Self::Running | Self::Waiting => None,
        }
    }
}

/// Merge methods and the labels their menu items carry.
pub(in crate::workspace) type MethodLabels = Vec<(MergeMethod, String)>;

/// The merge button and its menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct MergeView {
    /// The preferred method's label, or a plain one when none is allowed.
    pub label: String,
    /// The allowed methods, preferred first, and their labels.
    pub methods: Vec<(MergeMethod, String)>,
    /// The methods auto-merge can be turned on with, and their labels.
    pub auto: Vec<(MergeMethod, String)>,
    /// In a stack with open pull requests below it: those it would take
    /// with it, bottom first, and the methods to do so with.
    pub auto_stack: Option<(Vec<u64>, MethodLabels)>,
    pub slot: ActionSlot,
}

/// What the header offers to do to the pull request, by its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum HeaderControls {
    /// Not read yet, or merged: nothing.
    None,
    /// Open or a draft. Boxed: its slots dwarf the other variants.
    Active(Box<HeaderActions>),
    /// Closed unmerged.
    Reopen(ActionSlot),
}

#[cfg(test)]
impl HeaderControls {
    pub(in crate::workspace) fn actions(&self) -> Option<&HeaderActions> {
        match self {
            Self::Active(actions) => Some(actions.as_ref()),
            Self::None | Self::Reopen(_) => None,
        }
    }

    pub(in crate::workspace) fn reopen(&self) -> Option<&ActionSlot> {
        match self {
            Self::Reopen(slot) => Some(slot),
            Self::None | Self::Active(_) => None,
        }
    }
}

/// The header's actions on an open or draft pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct HeaderActions {
    pub merge: MergeView,
    pub close: ActionSlot,
    /// Only on a draft.
    pub ready: Option<ActionSlot>,
    /// Only while auto-merge is on and the viewer may turn it off.
    pub auto_merge_off: Option<ActionSlot>,
    /// Only while it is in the merge queue.
    pub dequeue: Option<ActionSlot>,
}

/// The open pull requests of the shown one's stack up to it, when there
/// are others below it.
pub(super) fn stack_range(state: &PrDetailState) -> Option<Vec<u64>> {
    state.stack()?.range_below(state.item.pr.number)
}

/// Merge is offered unless it cannot go at all (see [`MergeBlock`]); then
/// its tooltip says why, in the words the summary column uses.
pub(super) fn header_controls(
    loaded: &PrDetail,
    running: &ActionState,
    stack: Option<Vec<u64>>,
) -> HeaderControls {
    match loaded.summary.state {
        PrState::Open | PrState::Draft => {
            HeaderControls::Active(Box::new(header_actions(loaded, running, stack)))
        }
        PrState::Closed => {
            let refused = (!loaded.can_reopen_now()).then(s::reopen_not_allowed);
            HeaderControls::Reopen(ActionSlot::of(running, PrAction::Reopening, refused))
        }
        PrState::Merged => HeaderControls::None,
    }
}

pub(super) fn header_actions(
    loaded: &PrDetail,
    running: &ActionState,
    stack: Option<Vec<u64>>,
) -> HeaderActions {
    let draft = loaded.is_draft();
    let methods: Vec<(MergeMethod, String)> = loaded
        .merge_menu()
        .into_iter()
        .map(|method| (method, actions::merge_action_label(method)))
        .collect();
    let blocked = loaded.merge_block().map(|block| match block {
        MergeBlock::NoMethods => s::merge_no_methods(),
        MergeBlock::Queued => s::merge_blocked(s::merge_in_queue()),
        MergeBlock::Draft => s::merge_blocked(s::blocked_draft()),
        MergeBlock::Conflicting => s::merge_blocked(s::merge_conflicting()),
    });
    HeaderActions {
        merge: MergeView {
            label: methods
                .first()
                .map_or_else(s::merge_button, |(_, label)| label.clone()),
            methods,
            auto: loaded
                .auto_merge_menu()
                .into_iter()
                .map(|method| {
                    (
                        method,
                        s::auto_merge_item(actions::merge_action_label(method)),
                    )
                })
                .collect(),
            // The same methods, for the stack below it as well.
            auto_stack: stack
                .map(|range| {
                    let methods = loaded
                        .auto_merge_menu()
                        .into_iter()
                        .map(|method| {
                            (
                                method,
                                s::auto_merge_stack_item(actions::merge_action_label(method)),
                            )
                        })
                        .collect::<Vec<_>>();
                    (range, methods)
                })
                .filter(|(_, methods)| !methods.is_empty()),
            slot: ActionSlot::of(running, PrAction::Merging, blocked),
        },
        auto_merge_off: loaded
            .can_disable_auto_merge_now()
            .then(|| ActionSlot::of(running, PrAction::DisablingAutoMerge, None)),
        dequeue: loaded
            .can_dequeue_now()
            .then(|| ActionSlot::of(running, PrAction::Dequeuing, None)),
        close: ActionSlot::of(
            running,
            PrAction::Closing,
            (!loaded.can_close_now()).then(s::close_not_allowed),
        ),
        ready: draft.then(|| ActionSlot::of(running, PrAction::MarkingReady, None)),
    }
}

/// Auto-merge, while on: with which method and who turned it on. Not a
/// merge: GitHub merges (or queues) it once its requirements pass. Closed
/// or merged, there is nothing left for it to do, whatever was read.
pub(super) fn auto_merge_fact(loaded: &PrDetail) -> Option<Fact> {
    let on = loaded
        .auto_merge
        .as_ref()
        .filter(|_| loaded.summary.state.is_active())?;
    let method = on.method.map_or_else(s::method_unknown, method_label);
    Some(Fact {
        label: s::side_auto_merge(),
        value: match &on.by {
            Some(by) => s::auto_merge_on_by(method, by),
            None => s::auto_merge_on(method),
        },
        mood: Mood::Good,
    })
}

/// Its place in the merge queue: accepted, not merged yet; never for one
/// closed or merged, which has left the queue.
pub(super) fn queue_fact(loaded: &PrDetail) -> Option<Fact> {
    let entry = loaded.queue.filter(|_| loaded.summary.state.is_active())?;
    let (state, mood) = match entry.state {
        QueueState::Queued => (s::queue_queued(), Mood::Quiet),
        QueueState::AwaitingChecks => (s::queue_awaiting_checks(), Mood::Quiet),
        QueueState::Mergeable => (s::queue_mergeable(), Mood::Good),
        QueueState::Unmergeable => (s::queue_unmergeable(), Mood::Bad),
        QueueState::Locked => (s::queue_locked(), Mood::Warn),
        QueueState::Other => (s::queue_in_queue(), Mood::Quiet),
    };
    Some(Fact {
        label: s::side_merge_queue(),
        value: match entry.position {
            Some(position) => s::queue_position(position, state),
            None => state,
        },
        mood,
    })
}

pub(super) fn block_label(reason: BlockReason) -> String {
    match reason {
        BlockReason::Requirements => s::blocked_requirements(),
        BlockReason::Behind => s::blocked_behind(),
        BlockReason::Draft => s::blocked_draft(),
    }
}

pub(super) fn method_label(method: MergeMethod) -> String {
    match method {
        MergeMethod::Merge => s::method_merge(),
        MergeMethod::Squash => s::method_squash(),
        MergeMethod::Rebase => s::method_rebase(),
    }
}
