//! One identity's cards: the ledger record of each and the callback tokens on
//! its buttons, changed together. Released or unreceipted cards forget their
//! tokens, settled ones leave tombstones, and an identity change drops both.
//! Transports keep receipts and edits; nothing outside reaches the tokens.

use super::tokens::CardTokens;
use super::{CardCompletion, CardId, EditQueue, OwnedCards, SendClaim, SendOutcome, Settlement};
use crate::control::approval::ApprovalId;
use crate::remote_channel::bridge::{
    ApprovalPrompt, InboundAction, Outbound, PaneRef, PreparedPing,
};

pub(crate) use super::tokens::QuestionCard;

pub(crate) struct CardDesk<M, Owner, Receipt> {
    cards: OwnedCards<Owner, Receipt, M>,
    tokens: CardTokens<M>,
}

impl<M, Owner, Receipt> Default for CardDesk<M, Owner, Receipt> {
    fn default() -> Self {
        Self {
            cards: OwnedCards::default(),
            tokens: CardTokens::default(),
        }
    }
}

/// An outbound whose card, if any, is now sending, with its buttons' tokens
/// live.
pub(crate) enum Begun {
    Ping {
        pane: PaneRef,
        message: PreparedPing,
    },
    FlowAsk(PreparedPing),
    Approval(ApprovalPrompt),
    Notice(String),
}

/// The open cards a pane-side event settles at once.
#[derive(Clone, Copy, Debug)]
pub(crate) enum CardScope {
    Permissions {
        pane: PaneRef,
        session: u64,
    },
    /// Question `question_id` of the session, any field, or every question
    /// when `None`.
    Questions {
        pane: PaneRef,
        session: u64,
        question_id: Option<u64>,
    },
}

impl<M, Owner, Receipt> CardDesk<M, Owner, Receipt>
where
    M: Eq + std::hash::Hash + Clone,
    Owner: PartialEq,
{
    pub(crate) fn owner(&self) -> Option<&Owner> {
        self.cards.owner()
    }

    pub(crate) fn contains(&self, card: &CardId) -> bool {
        self.cards.ledger().contains(card)
    }

    /// Serve cards as `owner`. The same identity keeps every record and token.
    /// Another can neither edit the old messages nor receive their taps, so
    /// their records, tokens and tombstones go, and a still-waiting request
    /// can be offered once to the new identity.
    pub(crate) fn bind(&mut self, owner: Owner) {
        if self.cards.owner() != Some(&owner) {
            self.tokens.clear_settled();
        }
        for card in self.cards.bind(owner) {
            self.tokens.forget(&card);
        }
    }

    /// End the identity: nobody serves these cards any more.
    pub(crate) fn retire(&mut self) {
        self.tokens.clear_settled();
        for card in self.cards.retire() {
            self.tokens.forget(&card);
        }
    }

    /// Claim `card` for one send. Its tokens are minted by [`Self::begin`].
    pub(crate) fn claim(&mut self, card: CardId) -> Option<SendClaim> {
        self.cards.ledger_mut().claim(card)
    }

    /// Claim an approval card and mint its `(approve, refuse)` tokens, so they
    /// are live before the message that redeems them can be sent.
    pub(crate) fn claim_approval(
        &mut self,
        id: ApprovalId,
    ) -> Option<(SendClaim, (String, String))> {
        let claim = self.cards.ledger_mut().claim(CardId::Approval(id))?;
        Some((claim, self.tokens.mint_approval(id)))
    }

    /// Give up a claim nothing was sent for, with any tokens minted for it.
    /// An old claim cannot release the current one.
    pub(crate) fn abandon(&mut self, claim: &SendClaim) -> bool {
        let released = self.cards.ledger_mut().release(claim);
        if released {
            self.tokens.forget(&claim.card);
        }
        released
    }

    /// Start sending `outbound`: its claim, if any, must still be queued.
    /// Mints the tokens its buttons carry.
    pub(crate) fn begin(&mut self, claim: Option<&SendClaim>, outbound: Outbound) -> Option<Begun> {
        if let Some(claim) = claim
            && !self.cards.ledger_mut().begin(claim)
        {
            return None;
        }
        Some(match outbound {
            Outbound::Ping(ping) => Begun::Ping {
                pane: ping.pane,
                message: self.tokens.mint_ping(ping),
            },
            Outbound::FlowAsk(prompt) => Begun::FlowAsk(self.tokens.mint_flow_ask(prompt)),
            Outbound::Approval(prompt) => Begun::Approval(prompt),
            Outbound::Notice(text) => Begun::Notice(text),
        })
    }

    /// Record what a send came to. `replies` are the message ids a text reply
    /// may answer the card through: a question delivered by the live session.
    pub(crate) fn complete(
        &mut self,
        claim: &SendClaim,
        outcome: SendOutcome<Receipt>,
        replies: impl IntoIterator<Item = M>,
    ) -> CardCompletion<Receipt> {
        if let CardId::Question {
            pane,
            question,
            step,
        } = &claim.card
        {
            for message in replies {
                self.tokens
                    .note_question_sent(message, *pane, *question, *step);
            }
        }
        let completion = self.cards.ledger_mut().complete(claim, outcome);
        if matches!(completion, CardCompletion::Released) {
            self.tokens.forget(&claim.card);
        }
        completion
    }

    /// Settle `card` with `label`. A card with nothing to edit still retires
    /// its tokens, so a late tap cannot answer it again.
    pub(crate) fn settle(&mut self, card: &CardId, label: String) -> Settlement<Receipt> {
        let settlement = self.cards.ledger_mut().settle(card, label);
        match &settlement {
            Settlement::Edit { .. } | Settlement::Recorded => self.tokens.tombstone(card),
            Settlement::Unreceipted => self.tokens.forget(card),
        }
        settlement
    }

    pub(crate) fn open(&self, scope: CardScope) -> Vec<CardId> {
        let ledger = self.cards.ledger();
        match scope {
            CardScope::Permissions { pane, session } => ledger.permissions(pane, session),
            CardScope::Questions {
                pane,
                session,
                question_id,
            } => ledger.questions(pane, session, question_id),
        }
    }

    /// What a tap on `data` answers, or `None` for a listing button.
    pub(crate) fn route_tap(&mut self, data: &str) -> Option<InboundAction> {
        self.tokens.route_tap(data)
    }

    pub(crate) fn question_reply(&self, reply_to: Option<&M>) -> Option<QuestionCard> {
        self.tokens.question_reply(reply_to)
    }

    /// Record that `message` was tapped. A managed card's settlement, or a
    /// retired token's, owns the message before its send response arrives.
    pub(crate) fn note_tap(&mut self, data: &str, message: M, managed: bool) {
        if managed || self.tokens.is_settled(data) {
            self.cards.note_settled(message);
        }
    }

    /// Whether a tap on `message` may strip its buttons.
    pub(crate) fn untracked(&self, message: &M) -> bool {
        self.cards.untracked(message)
    }

    /// Record that a settlement edit owns `message`.
    pub(crate) fn note_settled(&mut self, message: M) {
        self.cards.note_settled(message);
    }

    pub(crate) fn edits(&self) -> &EditQueue<Receipt> {
        &self.cards.edits
    }

    pub(crate) fn edits_mut(&mut self) -> &mut EditQueue<Receipt> {
        &mut self.cards.edits
    }

    #[cfg(test)]
    pub(crate) fn owns(&self, claim: &SendClaim) -> bool {
        self.cards.ledger().owns(claim)
    }

    #[cfg(test)]
    pub(crate) fn is_sending(&self, claim: &SendClaim) -> bool {
        self.cards.ledger().is_sending(claim)
    }

    #[cfg(test)]
    pub(crate) fn approval_token_count(&self) -> usize {
        self.tokens.approval_count()
    }

    #[cfg(test)]
    pub(crate) fn live_token_count(&self) -> usize {
        self.tokens.live_count()
    }
}

impl<M, Owner, Receipt: Clone> CardDesk<M, Owner, Receipt> {
    /// The edits owed as the app quits; see [`OwnedCards::close_for_quit`].
    pub(crate) fn close_for_quit(&mut self, cancelled: &str) -> Vec<(Receipt, String)> {
        self.cards.close_for_quit(cancelled)
    }
}

#[cfg(test)]
mod tests;
