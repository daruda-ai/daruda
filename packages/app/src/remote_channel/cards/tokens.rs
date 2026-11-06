//! Callback tokens for one identity's cards, and what a tap or reply on a card
//! routes to. Only [`super::CardDesk`] reaches this table, so a card's tokens
//! cannot change apart from its ledger record.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use uuid::Uuid;

use super::CardId;
use crate::control::approval::{ApprovalChoice, ApprovalId};
use crate::remote_channel::bridge::core::SENT_PINGS_CAP;
use crate::remote_channel::bridge::{
    BridgePing, FlowAskPrompt, InboundAction, InlineKeyboard, MessageTail, PaneRef,
    PermissionDecision, PingPrompt, PreparedPing, QuestionButton, QuestionPromptRef, QuestionReply,
};

/// Callback-data prefix for an approval button. Distinct from the listing
/// prefix and from a permission token (bare `Uuid::simple`, no `:`), so the
/// namespaces cannot collide structurally.
pub(crate) const APPROVAL_TOKEN_PREFIX: &str = "apv";
const FLOW_ASK_TOKEN_PREFIX: &str = "faq";
const QUESTION_TOKEN_PREFIX: &str = "qst";

/// Live tokens kept per kind before the oldest is evicted. A token whose card
/// is settled in-app is forgotten then; this bounds a run of cards nobody
/// answers. An evicted token answers a tap like any unknown one.
pub(crate) const TOKENS_PER_KIND_CAP: usize = 64;
pub(crate) const SETTLED_CALLBACKS_CAP: usize = 64;

/// What one button on a live card means.
#[derive(Clone, Debug)]
enum Token {
    /// Not consumed on a tap: the card stays on screen, and tapping the same
    /// button twice is ordinary use. The approval store discards the repeat.
    Approval {
        id: ApprovalId,
        choice: ApprovalChoice,
    },
    /// Not consumed either: the flow refuses a second answer itself.
    FlowAsk {
        worktree: PathBuf,
        run_dir: PathBuf,
        ask_id: u64,
        decision: PermissionDecision,
    },
    Permission {
        pane: PaneRef,
        session: u64,
        request: u64,
        decision: PermissionDecision,
    },
    /// Consumed, as a permission token is: the card is settled the moment its
    /// field moves on, and a second tap must not toggle or answer twice.
    Question {
        pane: PaneRef,
        question: QuestionPromptRef,
        step: u64,
        button: QuestionButton,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Approval,
    FlowAsk,
    Permission,
    Question,
}

impl Kind {
    fn of(card: &CardId) -> Self {
        match card {
            CardId::Approval(_) => Self::Approval,
            CardId::FlowAsk { .. } => Self::FlowAsk,
            CardId::Permission { .. } => Self::Permission,
            CardId::Question { .. } => Self::Question,
        }
    }
}

impl Token {
    fn kind(&self) -> Kind {
        match self {
            Self::Approval { .. } => Kind::Approval,
            Self::FlowAsk { .. } => Kind::FlowAsk,
            Self::Permission { .. } => Kind::Permission,
            Self::Question { .. } => Kind::Question,
        }
    }

    fn card(&self) -> CardId {
        match self {
            Self::Approval { id, .. } => CardId::Approval(*id),
            Self::FlowAsk {
                run_dir, ask_id, ..
            } => CardId::FlowAsk {
                run_dir: run_dir.clone(),
                ask_id: *ask_id,
            },
            Self::Permission {
                pane,
                session,
                request,
                ..
            } => CardId::Permission {
                pane: *pane,
                session: *session,
                request: *request,
            },
            Self::Question {
                pane,
                question,
                step,
                ..
            } => CardId::Question {
                pane: *pane,
                question: *question,
                step: *step,
            },
        }
    }

    /// What a tap on the live button does, and whether it spends the token.
    fn on_tap(&self) -> (InboundAction, bool) {
        match self {
            Self::Approval { id, choice } => (
                InboundAction::ResolveApproval {
                    id: *id,
                    choice: *choice,
                },
                false,
            ),
            Self::FlowAsk {
                worktree,
                run_dir,
                ask_id,
                decision,
            } => (
                InboundAction::ResolveFlowAsk {
                    worktree: worktree.clone(),
                    run_dir: run_dir.clone(),
                    ask_id: *ask_id,
                    decision: decision.clone(),
                },
                false,
            ),
            Self::Permission {
                pane,
                session,
                request,
                decision,
            } => (
                InboundAction::RespondPermission {
                    pane: *pane,
                    session: *session,
                    perm_id: *request,
                    decision: decision.clone(),
                },
                true,
            ),
            Self::Question {
                pane,
                question,
                button,
                ..
            } => (
                InboundAction::RespondQuestion {
                    pane: *pane,
                    question: *question,
                    reply: QuestionReply::Button(*button),
                },
                true,
            ),
        }
    }

    /// What a tap answers once the card settled. A flow tap still names its
    /// original question so the flow can say it was already answered.
    fn tombstone(&self) -> InboundAction {
        match self {
            Self::Approval { .. } => InboundAction::StaleApproval,
            Self::FlowAsk { .. } => self.on_tap().0,
            Self::Permission { .. } | Self::Question { .. } => InboundAction::Ignore,
        }
    }
}

/// What a text reply to a question card answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuestionCard {
    Live {
        pane: PaneRef,
        question: QuestionPromptRef,
    },
    /// Settled or superseded; a reply is refused as stale.
    Settled,
}

pub(super) struct CardTokens<M> {
    live: HashMap<String, Token>,
    /// Insertion order per kind. A consumed token keeps its slot until
    /// evicted, so a tap never makes room for a newer card early.
    order: HashMap<Kind, VecDeque<String>>,
    /// Retired tokens and what a late tap on each still answers. Identifies a
    /// tap racing a settlement even before the send returns its message id.
    settled: VecDeque<(String, InboundAction)>,
    /// Question cards a text reply answers, with the step each carried. Kept
    /// after the card settles, so a late reply is refused rather than sent as
    /// a prompt.
    sent_questions: HashMap<M, (PaneRef, QuestionPromptRef, u64)>,
    sent_questions_order: VecDeque<M>,
    settled_questions: VecDeque<(PaneRef, QuestionPromptRef, u64)>,
}

impl<M> Default for CardTokens<M> {
    fn default() -> Self {
        Self {
            live: HashMap::new(),
            order: HashMap::new(),
            settled: VecDeque::new(),
            sent_questions: HashMap::new(),
            sent_questions_order: VecDeque::new(),
            settled_questions: VecDeque::new(),
        }
    }
}

impl<M: Eq + std::hash::Hash + Clone> CardTokens<M> {
    fn insert(&mut self, token: String, value: Token) {
        let order = self.order.entry(value.kind()).or_default();
        self.live.insert(token.clone(), value);
        order.push_back(token);
        if order.len() > TOKENS_PER_KIND_CAP
            && let Some(oldest) = order.pop_front()
        {
            self.live.remove(&oldest);
        }
    }

    /// Mint the two tokens one approval card needs, `(approve, refuse)`.
    pub(super) fn mint_approval(&mut self, id: ApprovalId) -> (String, String) {
        // Random rather than derived from `id`: a card survives a restart in
        // the user's chat history while this table does not, and a derived
        // token would let a stale button resolve a new request reusing it.
        let mut mint = |choice| {
            let token = format!("{APPROVAL_TOKEN_PREFIX}:{}", Uuid::new_v4().as_simple());
            self.insert(token.clone(), Token::Approval { id, choice });
            token
        };
        (
            mint(ApprovalChoice::Approved),
            mint(ApprovalChoice::Refused),
        )
    }

    pub(super) fn mint_ping(&mut self, ping: BridgePing) -> PreparedPing {
        let pane = ping.pane;
        let keyboard = ping.prompt.map(|prompt| match prompt {
            PingPrompt::Permission(prompt) => InlineKeyboard::single_row(
                prompt
                    .buttons
                    .into_iter()
                    .map(|(label, decision)| {
                        let token = Uuid::new_v4().simple().to_string();
                        let value = Token::Permission {
                            pane,
                            session: prompt.session,
                            request: prompt.perm_id,
                            decision,
                        };
                        self.insert(token.clone(), value);
                        (label, token)
                    })
                    .collect(),
            ),
            PingPrompt::Question(prompt) => InlineKeyboard {
                rows: prompt
                    .rows
                    .into_iter()
                    .map(|row| {
                        row.into_iter()
                            .map(|(label, button)| {
                                let token =
                                    format!("{QUESTION_TOKEN_PREFIX}:{}", Uuid::new_v4().simple());
                                let value = Token::Question {
                                    pane,
                                    question: prompt.question,
                                    step: prompt.step,
                                    button,
                                };
                                self.insert(token.clone(), value);
                                (label, token)
                            })
                            .collect()
                    })
                    .collect(),
            },
        });
        PreparedPing {
            header: ping.header,
            tail: ping.tail,
            keyboard,
        }
    }

    pub(super) fn mint_flow_ask(&mut self, prompt: FlowAskPrompt) -> PreparedPing {
        let buttons = prompt
            .buttons
            .into_iter()
            .map(|(label, decision)| {
                let token = format!("{FLOW_ASK_TOKEN_PREFIX}:{}", Uuid::new_v4().simple());
                let value = Token::FlowAsk {
                    worktree: prompt.worktree.clone(),
                    run_dir: prompt.run_dir.clone(),
                    ask_id: prompt.ask_id,
                    decision,
                };
                self.insert(token.clone(), value);
                (label, token)
            })
            .collect();
        PreparedPing {
            header: String::new(),
            tail: MessageTail::Plain(prompt.summary),
            keyboard: Some(InlineKeyboard::single_row(buttons)),
        }
    }

    /// What a tap on `data` answers, or `None` for a listing button, which
    /// only the routing core can resolve.
    pub(super) fn route_tap(&mut self, data: &str) -> Option<InboundAction> {
        if let Some((_, action)) = self.settled.iter().find(|(token, _)| token == data) {
            return Some(action.clone());
        }
        if let Some(token) = self.live.get(data) {
            let (action, consumed) = token.on_tap();
            if consumed {
                self.live.remove(data);
            }
            return Some(action);
        }
        if data.starts_with(crate::remote_channel::command::LISTING_TOKEN_PREFIX) {
            return None;
        }
        // An approval button outlives this table in the chat history; saying
        // the request is gone beats a tap that seems to do nothing.
        Some(if data.starts_with(APPROVAL_TOKEN_PREFIX) {
            InboundAction::StaleApproval
        } else {
            InboundAction::Ignore
        })
    }

    /// Record that `message_id` is card `step` for `question`, so a text reply
    /// to it answers that field while the card is live.
    pub(super) fn note_question_sent(
        &mut self,
        message_id: M,
        pane: PaneRef,
        question: QuestionPromptRef,
        step: u64,
    ) {
        self.sent_questions
            .insert(message_id.clone(), (pane, question, step));
        self.sent_questions_order.push_back(message_id);
        if self.sent_questions_order.len() > SENT_PINGS_CAP
            && let Some(oldest) = self.sent_questions_order.pop_front()
        {
            self.sent_questions.remove(&oldest);
        }
    }

    pub(super) fn question_reply(&self, reply_to: Option<&M>) -> Option<QuestionCard> {
        let card = self.sent_questions.get(reply_to?)?;
        Some(if self.settled_questions.contains(card) {
            QuestionCard::Settled
        } else {
            let (pane, question, _) = *card;
            QuestionCard::Live { pane, question }
        })
    }

    /// Retire `card`'s tokens into bounded tombstones: a late flow tap still
    /// names its original question, and other settled taps cannot compete
    /// with the settlement edit.
    pub(super) fn tombstone(&mut self, card: &CardId) {
        if let CardId::Question {
            pane,
            question,
            step,
        } = card
        {
            self.settled_questions.push_back((*pane, *question, *step));
            if self.settled_questions.len() > SETTLED_CALLBACKS_CAP {
                self.settled_questions.pop_front();
            }
        }
        let retired: Vec<_> = self
            .live
            .iter()
            .filter(|(_, token)| token.card() == *card)
            .map(|(data, token)| (data.clone(), token.tombstone()))
            .collect();
        self.forget(card);
        for callback in retired {
            self.settled.push_back(callback);
            if self.settled.len() > SETTLED_CALLBACKS_CAP {
                self.settled.pop_front();
            }
        }
    }

    pub(super) fn forget(&mut self, card: &CardId) {
        self.live.retain(|_, token| token.card() != *card);
        let live = &self.live;
        if let Some(order) = self.order.get_mut(&Kind::of(card)) {
            order.retain(|token| live.contains_key(token));
        }
    }

    pub(super) fn is_settled(&self, data: &str) -> bool {
        self.settled.iter().any(|(token, _)| token == data)
    }

    pub(super) fn clear_settled(&mut self) {
        self.settled.clear();
    }

    #[cfg(test)]
    pub(super) fn approval_count(&self) -> usize {
        self.live
            .values()
            .filter(|token| token.kind() == Kind::Approval)
            .count()
    }

    #[cfg(test)]
    pub(super) fn live_count(&self) -> usize {
        self.live.len()
    }
}

#[cfg(test)]
mod tests;
