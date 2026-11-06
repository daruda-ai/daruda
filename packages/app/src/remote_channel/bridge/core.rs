//! Channel-independent reply routing and target selection. Card buttons and
//! replies to cards are [`crate::remote_channel::cards::CardDesk`]'s.

use super::{InboundAction, PaneRef, QuestionReply, Routed, Unaimed};
use crate::remote_channel::cards::QuestionCard;
use std::collections::{HashMap, VecDeque};

/// Upper bound on `(message_id -> PaneRef)` entries in `sent_pings`.
/// Oldest entries are evicted first; a reply-to lookup for an evicted
/// message_id falls back to `last_pinged`.
pub(crate) const SENT_PINGS_CAP: usize = 64;

/// One recipient's routing state. Native message IDs remain lossless.
pub struct RoutingCore<M = i64> {
    pub(crate) last_pinged: Option<PaneRef>,
    pub(crate) sent_pings: HashMap<M, PaneRef>,
    pub(crate) sent_pings_order: VecDeque<M>,
    command_state: crate::remote_channel::command::CommandState,
}

impl<M: Eq + std::hash::Hash + Clone> Default for RoutingCore<M> {
    fn default() -> Self {
        Self {
            last_pinged: None,
            sent_pings: HashMap::new(),
            sent_pings_order: VecDeque::new(),
            command_state: crate::remote_channel::command::CommandState::default(),
        }
    }
}

impl<M: Eq + std::hash::Hash + Clone> RoutingCore<M> {
    pub fn command_state_mut(&mut self) -> &mut crate::remote_channel::command::CommandState {
        &mut self.command_state
    }

    /// Stop sending plain text to `target`, a pane found gone, including via
    /// `last_pinged`. Says how the plain-text target moved so the reply can.
    pub fn forget(&mut self, target: PaneRef) -> crate::remote_channel::command::Absorbed {
        use crate::remote_channel::command::Absorbed;
        let picked = self.command_state.selected() == Some(target);
        let before = self.command_state.plain_text_target(None, self.last_pinged);
        self.command_state.forget(target);
        if self.last_pinged == Some(target) {
            self.last_pinged = None;
        }
        let after = self.command_state.plain_text_target(None, self.last_pinged);
        match after {
            _ if picked => Absorbed::SelectionDropped,
            _ if after == before => Absorbed::Nothing,
            None => Absorbed::SelectionDropped,
            Some(_) => Absorbed::SelectionRestored,
        }
    }

    /// Called only after the adapter authenticates both sender and conversation.
    /// `card` is the question card `reply_to_message_id` names, if any.
    pub(crate) fn route_text(
        &mut self,
        text: String,
        reply_to_message_id: Option<M>,
        card: Option<QuestionCard>,
    ) -> Routed {
        // Parsing sits behind the gate on purpose: a listing names projects,
        // lanes, and session titles, so an unauthorized chat must not reach it.
        // A name we do not own is held back rather than answered: the agent's
        // slash namespace is open and ours is closed, so only the pane can say
        // whether `/usage` is its command or a typo of `/use`.
        let unknown = match crate::control::spec::parse(&text) {
            Ok(command) => return Routed::Ready(InboundAction::RunCommand { command }),
            Err(crate::control::spec::ParseError::NotACommand) => None,
            Err(crate::control::spec::ParseError::Unknown { input, suggestion }) => {
                Some((input, suggestion))
            }
            // A command we *do* own, used wrongly. Ours to answer.
            Err(error) => return Routed::Ready(InboundAction::ReportParseError { error }),
        };

        // A reply to a question card answers it, slash or not: only a
        // command daruda owns, parsed above, outranks the card.
        match card {
            Some(QuestionCard::Settled) => return Routed::Ready(InboundAction::StaleQuestion),
            Some(QuestionCard::Live { pane, question }) => {
                return Routed::Ready(InboundAction::RespondQuestion {
                    pane,
                    question,
                    reply: QuestionReply::Text(text),
                });
            }
            None => {}
        }

        let reply_to = reply_to_message_id
            .and_then(|id| self.sent_pings.get(&id))
            .copied();

        match self
            .command_state
            .plain_text_target(reply_to, self.last_pinged)
        {
            Some(pane) => Routed::Ready(match unknown {
                Some((name, suggestion)) => InboundAction::UnknownSlash {
                    pane,
                    name,
                    text,
                    suggestion,
                },
                None => InboundAction::InjectPrompt { pane, text },
            }),
            // Nothing here names a target, and this layer cannot ask the app
            // for one. Handed on rather than answered — both the target and,
            // for a slash, whose command it is are still open questions.
            None => Routed::NeedsTarget(match unknown {
                Some((name, suggestion)) => Unaimed::Slash {
                    name,
                    text,
                    suggestion,
                },
                None => Unaimed::Text { text },
            }),
        }
    }

    /// A listing button tap. Never consumed: tapping the same row twice is
    /// ordinary use, and a superseded listing answers as stale.
    pub(crate) fn route_listing(&self, data: &str) -> InboundAction {
        match self.command_state.resolve_token(data) {
            Some(pane) => InboundAction::SelectTarget { pane },
            None => InboundAction::StaleListing,
        }
    }

    /// Record a delivered ping from `pane`, letting it take the plain-text
    /// target unless it is the orchestrator's — see [`Self::record_bystander_sent`].
    pub fn record_delivered(
        &mut self,
        message_id: M,
        pane: PaneRef,
        orchestrator: Option<PaneRef>,
    ) {
        if orchestrator == Some(pane) {
            self.record_bystander_sent(message_id, pane);
        } else {
            self.record_sent(message_id, pane);
        }
    }

    /// Record a delivered ping, making `pane` where the next plain message goes.
    pub fn record_sent(&mut self, message_id: M, pane: PaneRef) {
        self.command_state.note_ping(pane);
        self.record_bystander_sent(message_id, pane);
    }

    /// Record a ping that stays reply-able but does not take the plain-text
    /// target — the orchestrator reporting on a chat it just pointed the
    /// phone at must not pull the phone back to itself.
    pub fn record_bystander_sent(&mut self, message_id: M, pane: PaneRef) {
        self.last_pinged = Some(pane);
        self.sent_pings.insert(message_id.clone(), pane);
        self.sent_pings_order.push_back(message_id);

        if self.sent_pings_order.len() > SENT_PINGS_CAP
            && let Some(oldest) = self.sent_pings_order.pop_front()
        {
            self.sent_pings.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests;
