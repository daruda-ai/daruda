//! The summary column: facts, reviewers, the linked worktree and the stack.

use super::*;

/// What the summary column says under the linked worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum LaneNote {
    /// On the pull request's own branch: a push updates it.
    UpdatesPr,
    /// A fork's review worktree: its pushes go nowhere near it.
    ReviewOnly,
}

/// One `label: value` line of the summary column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct Fact {
    pub label: String,
    pub value: String,
    pub mood: Mood,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct SideView {
    pub merge: Vec<Fact>,
    /// Each reviewer and where they stand, then the open threads.
    pub reviews: Vec<Fact>,
    pub lane: Option<RowLane>,
    /// Said under the linked worktree: whether its pushes reach the PR.
    pub lane_note: Option<LaneNote>,
    /// Who is asked to review can be changed: GitHub lets the viewer.
    pub requests: Option<RequestsView>,
    /// Why the worktree is tied to it, and what can be done about that.
    pub link: super::super::links::LinkInputs,
    /// The pull request, for tying a worktree to it.
    pub item: super::super::list::PrKey,
    /// Its native stack, when it is in one.
    pub stack: Option<StackView>,
}

/// A native stack beside the detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct StackView {
    pub number: u64,
    pub base: String,
    pub size: u32,
    /// Bottom first.
    pub entries: Vec<StackEntryView>,
    /// Where the shown one sits; `None` when it is not among those listed.
    pub shown_at: Option<u32>,
    /// Pull requests of the stack not listed here.
    pub more: usize,
}

/// One pull request of a stack beside the detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct StackEntryView {
    pub position: u32,
    pub number: u64,
    pub title: String,
    /// Said beside it when not open.
    pub state: PrState,
    /// The one the detail shows.
    pub shown: bool,
}

/// Changing who is asked to review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct RequestsView {
    /// Asked and not reviewed yet: these can be unasked.
    pub removable: Vec<String>,
    pub input: Option<gpui::Entity<crate::ui::InputState>>,
    pub op: Option<super::super::reviewers::ReviewerOp>,
}

/// Reviewers asked and not reviewed yet, under names `gh` can be given.
pub(super) fn removable(loaded: &PrDetail) -> Vec<String> {
    loaded
        .reviewers
        .iter()
        .filter(|(name, state)| {
            *state == daruda_github::ReviewerState::Requested
                && daruda_github::reviewer_name_ok(name)
        })
        .map(|(name, _)| name.clone())
        .collect()
}

pub(super) fn side(
    item: &PrListItem,
    loaded: &PrDetail,
    lane: Option<RowLane>,
    threads: (usize, bool),
    state: &PrDetailState,
    link: super::super::links::LinkInputs,
) -> SideView {
    let (merge_state, merge_mood) = match loaded.mergeability {
        Mergeability::Ready => (s::merge_ready(), Mood::Good),
        Mergeability::Blocked(reason) => (block_label(reason), Mood::Warn),
        Mergeability::Conflicting => (s::merge_conflicting(), Mood::Bad),
        Mergeability::Unknown => (s::merge_unknown(), Mood::Quiet),
    };
    let (conflicts, conflicts_mood) = match loaded.mergeability {
        Mergeability::Conflicting => (s::conflicts_yes(), Mood::Bad),
        Mergeability::Unknown => (s::merge_unknown(), Mood::Quiet),
        Mergeability::Ready | Mergeability::Blocked(_) => (s::conflicts_none(), Mood::Good),
    };
    let methods = if loaded.methods.is_empty() {
        s::merge_methods_none()
    } else {
        let labels: Vec<String> = loaded.methods.iter().map(|m| method_label(*m)).collect();
        labels.join(", ")
    };
    // With the reviewers editable, those that can be unasked are listed
    // with their control instead.
    let unaskable = if loaded.can_edit() {
        removable(loaded)
    } else {
        Vec::new()
    };
    let mut reviews: Vec<Fact> = loaded
        .reviewers
        .iter()
        .filter(|(name, _)| !unaskable.contains(name))
        .map(|(name, state)| {
            let (value, mood) = reviewer_label(*state);
            Fact {
                label: name.clone(),
                value,
                mood,
            }
        })
        .collect();
    if reviews.is_empty() && unaskable.is_empty() {
        reviews.push(Fact {
            label: s::side_reviewers(),
            value: s::reviewers_none(),
            mood: Mood::Quiet,
        });
    }
    reviews.push(Fact {
        label: s::side_open_threads(),
        value: match threads {
            (count, false) => count.to_string(),
            (count, true) => s::count_capped(count),
        },
        mood: if threads == (0, false) {
            Mood::Quiet
        } else {
            Mood::Warn
        },
    });
    let lane_note = lane.as_ref().map(|_| {
        if item.pr.cross_repo {
            LaneNote::ReviewOnly
        } else {
            LaneNote::UpdatesPr
        }
    });
    SideView {
        merge: vec![
            Fact {
                label: s::side_state(),
                value: merge_state,
                mood: merge_mood,
            },
            Fact {
                label: s::side_methods(),
                value: methods,
                mood: Mood::Quiet,
            },
            Fact {
                label: s::side_conflicts(),
                value: conflicts,
                mood: conflicts_mood,
            },
        ]
        .into_iter()
        .chain(auto_merge_fact(loaded))
        .chain(queue_fact(loaded))
        .collect(),
        reviews,
        lane,
        lane_note,
        requests: loaded.can_edit().then(|| {
            let requests = state.reviewer_requests();
            RequestsView {
                removable: removable(loaded),
                input: requests.input.clone(),
                op: requests.op.clone(),
            }
        }),
        link,
        item: (item.repo.clone(), item.pr.number),
        stack: match state.stack() {
            Some(daruda_github::StackRead::Stacked(stack)) => Some(StackView {
                number: stack.number,
                base: stack.base.clone(),
                size: stack.size,
                entries: stack
                    .entries
                    .iter()
                    .map(|entry| StackEntryView {
                        position: entry.position,
                        number: entry.number,
                        title: entry.title.clone(),
                        state: entry.state,
                        shown: entry.number == item.pr.number,
                    })
                    .collect(),
                shown_at: stack
                    .entries
                    .iter()
                    .find(|entry| entry.number == item.pr.number)
                    .map(|entry| entry.position),
                more: stack.more(),
            }),
            _ => None,
        },
    }
}

pub(super) fn reviewer_label(state: ReviewerState) -> (String, Mood) {
    match state {
        ReviewerState::Requested => (s::reviewer_requested(), Mood::Quiet),
        ReviewerState::Approved => (s::review_approved(), Mood::Good),
        ReviewerState::ChangesRequested => (s::review_changes_requested(), Mood::Warn),
        ReviewerState::Commented => (s::reviewer_commented(), Mood::Quiet),
        ReviewerState::Dismissed => (s::reviewer_dismissed(), Mood::Quiet),
        ReviewerState::Pending => (s::reviewer_pending(), Mood::Quiet),
        ReviewerState::Other => (s::reviewer_other(), Mood::Quiet),
    }
}
