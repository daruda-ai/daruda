//! Queue ownership and the off-foreground Telegram send loop.

use std::cell::RefMut;
use std::sync::Arc;

use futures::StreamExt as _;
use gpui::App;

use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;

use crate::remote_channel::cards::quit::QuitEdit;
use crate::remote_channel::cards::{
    Begun, CardId, CardScope, EditOutcome, SendClaim, SendOutcome, Settlement,
};
use crate::settings_store::SettingsStore;
use crate::telegram::bridge::{Outbound, OutboundMsg, PaneRef};
use crate::telegram::{client, keychain, trace};

use super::bot::ReadyBot;
use super::{TelegramBridge, html_body, plain_body};

pub(super) struct QueuedOutbound {
    pub bot: Arc<ReadyBot>,
    pub chat_id: i64,
    pub outbound: Outbound,
    pub card: Option<SendClaim>,
}

/// A sent card's message, editable by whichever bot serves its identity.
#[derive(Clone)]
pub(super) struct Receipt {
    owner: CardOwner,
    message_id: i64,
    original: String,
}

/// The identity whose messages Telegram cards are: the bot, by its token, and
/// the paired chat. Not the `ReadyBot` instance — a token revision or inbox
/// reopen replaces that while the same bot keeps serving the same chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CardOwner {
    bot: String,
    chat_id: i64,
}

impl CardOwner {
    pub(super) fn of(bot: &ReadyBot, chat_id: i64) -> Self {
        Self {
            bot: crate::remote_channel::lock::digest_of(&bot.token.token),
            chat_id,
        }
    }
}

pub(super) type Desk = crate::remote_channel::cards::CardDesk<i64, CardOwner, Receipt>;

#[cfg(test)]
pub(crate) struct TestOutbox {
    pub(super) receiver: futures::channel::mpsc::UnboundedReceiver<QueuedOutbound>,
}

#[cfg(test)]
impl futures::Stream for TestOutbox {
    type Item = Outbound;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Outbound>> {
        self.get_mut()
            .receiver
            .poll_next_unpin(cx)
            .map(|queued| queued.map(|queued| queued.outbound))
    }
}

#[cfg(test)]
impl TestOutbox {
    pub(crate) fn try_recv(&mut self) -> Result<Outbound, futures::channel::mpsc::TryRecvError> {
        self.receiver.try_recv().map(|queued| queued.outbound)
    }

    pub(crate) fn begin_send(
        &mut self,
        message_id: i64,
        cx: &mut App,
    ) -> Option<impl FnOnce(&mut App) + use<>> {
        self.begin_send_with(SendOutcome::Delivered(message_id), cx)
    }

    pub(crate) fn begin_send_with(
        &mut self,
        outcome: SendOutcome<i64>,
        cx: &mut App,
    ) -> Option<impl FnOnce(&mut App) + use<>> {
        let queued = self.receiver.try_recv().ok()?;
        let prepared = prepare(queued, cx)?;
        Some(move |cx: &mut App| {
            finish(
                &prepared.bot,
                prepared.chat_id,
                prepared.pane,
                prepared.card,
                outcome,
                prepared.original,
                cx,
            )
        })
    }
}

enum Body {
    Ping(OutboundMsg),
    Plain {
        text: String,
        keyboard: Option<client::InlineKeyboard>,
    },
}

struct Prepared {
    bot: Arc<ReadyBot>,
    chat_id: i64,
    pane: Option<PaneRef>,
    body: Body,
    card: Option<SendClaim>,
    original: String,
}

fn live(queued: &QueuedOutbound, cx: &App) -> bool {
    super::inbound::live_bot(&queued.bot, Some(queued.chat_id), keychain::revision(), cx)
}

fn prepare(queued: QueuedOutbound, cx: &mut App) -> Option<Prepared> {
    if !live(&queued, cx) {
        discard(&queued, cx);
        return None;
    }
    let bridge = cx.global_mut::<TelegramBridge>();
    let begun = bridge
        .desk
        .get_mut()
        .begin(queued.card.as_ref(), queued.outbound)?;
    let (pane, body) = match begun {
        Begun::Ping { pane, message } => {
            let message = bridge.core.address(message);
            let buttons: usize = message
                .keyboard
                .as_ref()
                .map_or(0, |keyboard| keyboard.rows.iter().map(Vec::len).sum());
            if buttons > 0 {
                trace::state("permission.tokens", || {
                    format!("pane={} registered={buttons}", trace::pane(pane))
                });
            }
            trace::delivery("send.ping", || {
                format!(
                    "chat_id={} pane={} buttons={buttons} text={}",
                    queued.chat_id,
                    trace::pane(pane),
                    trace::tail_digest(&message.tail)
                )
            });
            (Some(pane), Body::Ping(message))
        }
        Begun::Notice(text) => (
            None,
            Body::Plain {
                text,
                keyboard: None,
            },
        ),
        Begun::FlowAsk(message) => (
            None,
            Body::Plain {
                text: match message.tail {
                    crate::remote_channel::bridge::MessageTail::Plain(text)
                    | crate::remote_channel::bridge::MessageTail::Markdown(text) => text,
                },
                keyboard: message.keyboard,
            },
        ),
        Begun::Approval(prompt) => (
            None,
            Body::Plain {
                text: prompt.summary,
                keyboard: Some(client::InlineKeyboard {
                    rows: vec![prompt.buttons.to_vec()],
                }),
            },
        ),
    };
    let original = match &body {
        Body::Ping(message) => plain_body(&message.header, &message.tail),
        Body::Plain { text, .. } => text.clone(),
    };
    Some(Prepared {
        bot: queued.bot,
        chat_id: queued.chat_id,
        pane,
        body,
        card: queued.card,
        original,
    })
}

fn discard(queued: &QueuedOutbound, cx: &mut App) {
    if let Some(card) = &queued.card {
        release(card, cx);
    }
}

/// Give up a claim nothing was sent for, with the tokens minted for it.
fn release(card: &SendClaim, cx: &mut App) {
    cx.global_mut::<TelegramBridge>()
        .desk
        .get_mut()
        .abandon(card);
}

/// The bot that may edit `receipt`'s message now: the one serving its
/// identity, with the token the keychain still holds. A rewritten token waits
/// for the bridge to serve it; the same bot can still edit afterwards.
fn editor(receipt: &Receipt, cx: &App) -> Option<Arc<ReadyBot>> {
    current_editor(&receipt.owner, cx)
}

fn current_editor(owner: &CardOwner, cx: &App) -> Option<Arc<ReadyBot>> {
    serving(owner, cx).filter(|bot| bot.token.revision == keychain::revision())
}

/// The bot serving `owner` now, if any.
fn serving(owner: &CardOwner, cx: &App) -> Option<Arc<ReadyBot>> {
    let bridge = cx.global::<TelegramBridge>();
    let bot = bridge.bot.ready().filter(|_| bridge.bot.serves())?;
    let config = &SettingsStore::global(cx).user().telegram;
    let serves = config.enabled
        && config.authorized_chat_id == Some(owner.chat_id)
        && CardOwner::of(bot, owner.chat_id) == *owner
        && bridge.desk.borrow().owner() == Some(owner);
    serves.then(|| bot.clone())
}

/// One quit edit, carrying the bot's token so it runs without the app.
fn quit_edit(bot: &ReadyBot, receipt: Receipt, label: String) -> QuitEdit {
    let token = bot.token.token.clone();
    let body =
        crate::remote_channel::dispatch::target::compose_edit_body(&receipt.original, &label);
    QuitEdit::new(QUIT_EDIT_FAILED, QUIT_EDIT_DEDUP, move || {
        client::edit_message_text(&token, receipt.owner.chat_id, receipt.message_id, &body)
    })
}

const QUIT_EDIT_FAILED: &str = "Telegram card failed to edit at quit";
const QUIT_EDIT_DEDUP: &str = "telegram.card.quit_edit";

pub(super) fn edit(receipt: Receipt, label: String, cx: &mut App) {
    let desk = cx.global_mut::<TelegramBridge>().desk.get_mut();
    if desk.owner() != Some(&receipt.owner) {
        return;
    }
    desk.note_settled(receipt.message_id);
    desk.edits_mut().push(receipt, label);
    flush_edits(cx);
}

pub(super) fn flush_edits(cx: &mut App) {
    let bridge = cx.global_mut::<TelegramBridge>();
    if !bridge.bot.serves() {
        return;
    }
    let pending = bridge.desk.get_mut().edits_mut().claim();
    for (attempt, receipt, label) in pending {
        if editor(&receipt, cx).is_none() {
            finish_edit(attempt, EditOutcome::Transient, cx);
            continue;
        }
        #[cfg(test)]
        if let Some(result) = crate::remote_channel::cards::tests::capture(
            "telegram",
            vec![receipt.message_id.to_string()],
            &receipt.original,
            &label,
            cx,
        ) {
            let outcome = EditOutcome::logged(result, EDIT_FAILED, EDIT_DEDUP);
            finish_edit(attempt, outcome, cx);
            continue;
        }
        cx.spawn(async move |cx| {
            let bot = cx.update(|cx| {
                if !cx
                    .global::<TelegramBridge>()
                    .desk
                    .borrow()
                    .edits()
                    .contains(attempt)
                {
                    return None;
                }
                editor(&receipt, cx)
            });
            let Some(bot) = bot else {
                cx.update(|cx| finish_edit(attempt, EditOutcome::Transient, cx));
                return;
            };
            let body = crate::remote_channel::dispatch::target::compose_edit_body(
                &receipt.original,
                &label,
            );
            let result = cx
                .background_executor()
                .spawn(async move {
                    // Asked again off the foreground: the keychain can be
                    // rewritten between the check above and this send.
                    if bot.token.revision != keychain::revision() {
                        return None;
                    }
                    Some(client::edit_message_text(
                        &bot.token.token,
                        receipt.owner.chat_id,
                        receipt.message_id,
                        &body,
                    ))
                })
                .await;
            let outcome = match result {
                Some(result) => EditOutcome::logged(result, EDIT_FAILED, EDIT_DEDUP),
                None => EditOutcome::Transient,
            };
            cx.update(|cx| finish_edit(attempt, outcome, cx));
        })
        .detach();
    }
}

const EDIT_FAILED: &str = "Telegram settled card failed to edit";
const EDIT_DEDUP: &str = "telegram.card.edit";

fn finish_edit(attempt: uuid::Uuid, outcome: EditOutcome, cx: &mut App) {
    cx.global_mut::<TelegramBridge>()
        .desk
        .get_mut()
        .edits_mut()
        .finish(attempt, outcome);
}

impl TelegramBridge {
    /// The bot serving now and the paired chat, when a message can go out.
    pub(super) fn addressee(&self, cx: &App) -> Option<(&Arc<ReadyBot>, i64)> {
        let bot = self.bot.ready().filter(|_| self.bot.serves())?;
        let config = SettingsStore::global(cx).user_arc();
        let chat_id = config
            .telegram
            .enabled
            .then_some(config.telegram.authorized_chat_id)
            .flatten()?;
        Some((bot, chat_id))
    }

    /// The card desk, only while it is bound to `bot` and `chat_id`. The desk
    /// follows the serving identity through `sync_card_owner`; a card for
    /// another identity is refused rather than recorded where its receipt
    /// could not be edited.
    pub(super) fn desk_for(&self, bot: &ReadyBot, chat_id: i64) -> Option<RefMut<'_, Desk>> {
        let desk = self.desk.borrow_mut();
        (desk.owner() == Some(&CardOwner::of(bot, chat_id))).then_some(desk)
    }

    pub(super) fn queue(
        &self,
        bot: &Arc<ReadyBot>,
        chat_id: i64,
        outbound: Outbound,
        card: Option<SendClaim>,
    ) -> Result<(), ()> {
        let result = self
            .outbound_tx
            .unbounded_send(QueuedOutbound {
                bot: bot.clone(),
                chat_id,
                outbound,
                card: card.clone(),
            })
            .map_err(|_| ());
        if result.is_err()
            && let Some(claim) = card
        {
            self.desk.borrow_mut().abandon(&claim);
        }
        result
    }

    /// The edits this bot owes as the app quits, closing their cards. A bot
    /// not serving its identity right now keeps its cards for tap-time
    /// cleanup, as it would for a settlement.
    pub(crate) fn quit_edits(cancelled: &str, cx: &mut App) -> Vec<QuitEdit> {
        let Some(owner) = cx
            .try_global::<Self>()
            .and_then(|bridge| bridge.desk.borrow().owner().cloned())
        else {
            return Vec::new();
        };
        let Some(bot) = current_editor(&owner, cx) else {
            return Vec::new();
        };
        let desk = cx.global_mut::<Self>().desk.get_mut();
        let owed = desk.close_for_quit(cancelled);
        for (receipt, _) in &owed {
            desk.note_settled(receipt.message_id);
        }
        owed.into_iter()
            .map(|(receipt, label)| {
                #[cfg(test)]
                if let Some(result) = crate::remote_channel::cards::tests::capture(
                    "telegram",
                    vec![receipt.message_id.to_string()],
                    &receipt.original,
                    &label,
                    cx,
                ) {
                    return QuitEdit::new(QUIT_EDIT_FAILED, QUIT_EDIT_DEDUP, move || result);
                }
                quit_edit(&bot, receipt, label)
            })
            .collect()
    }

    /// Bind the card ledger to the identity now serving. The same bot and chat
    /// keep every record; another identity can neither edit the old messages
    /// nor receive their taps, so their callback tokens are forgotten with
    /// them. Switching Telegram off or unpairing ends the identity the same
    /// way. A bot that is only between claims keeps its records for its
    /// return.
    pub(super) fn sync_card_owner(cx: &mut App) {
        let Some(bridge) = cx.try_global::<Self>() else {
            return;
        };
        let config = &SettingsStore::global(cx).user().telegram;
        let paired = config.authorized_chat_id.filter(|_| config.enabled);
        let owner = paired
            .zip(bridge.bot.ready())
            .map(|(chat_id, bot)| CardOwner::of(bot, chat_id));
        let desk = cx.global_mut::<Self>().desk.get_mut();
        match (paired, owner) {
            (None, _) => desk.retire(),
            (Some(_), Some(owner)) => desk.bind(owner),
            (Some(_), None) => {}
        }
    }

    pub(crate) fn settle_card(card: &CardId, label: &str, cx: &mut App) {
        if !cx.has_global::<Self>() {
            return;
        }
        if let Settlement::Edit { receipt, label } = cx
            .global_mut::<Self>()
            .desk
            .get_mut()
            .settle(card, label.to_owned())
        {
            edit(receipt, label, cx);
        }
    }

    pub(crate) fn settle_permissions(pane: PaneRef, session: u64, label: &str, cx: &mut App) {
        Self::settle_open(CardScope::Permissions { pane, session }, label, cx);
    }

    pub(crate) fn settle_questions(
        pane: PaneRef,
        session: u64,
        question_id: Option<u64>,
        label: &str,
        cx: &mut App,
    ) {
        let scope = CardScope::Questions {
            pane,
            session,
            question_id,
        };
        Self::settle_open(scope, label, cx);
    }

    fn settle_open(scope: CardScope, label: &str, cx: &mut App) {
        let cards = cx
            .try_global::<Self>()
            .map(|bridge| bridge.desk.borrow().open(scope))
            .unwrap_or_default();
        for card in cards {
            Self::settle_card(&card, label, cx);
        }
    }
}

/// Record what a send came to. A card that may have reached the chat is kept
/// even if its `ReadyBot` was replaced meanwhile: the ledger's claim decides
/// whether it still belongs to this identity, and a same-bot replacement can
/// still edit it or receive its taps, so it must not be offered again.
fn finish(
    bot: &Arc<ReadyBot>,
    chat_id: i64,
    pane: Option<PaneRef>,
    card: Option<SendClaim>,
    outcome: SendOutcome<i64>,
    original: String,
    cx: &mut App,
) {
    let live = super::inbound::live_bot(bot, Some(chat_id), keychain::revision(), cx);
    let orchestrator = crate::orchestrator::pane(cx);
    let bridge = cx.global_mut::<TelegramBridge>();
    let message_id = match &outcome {
        SendOutcome::Delivered(id) => Some(*id),
        SendOutcome::Unconfirmed | SendOutcome::NotSent => None,
    };
    // Only the live bot's message takes a reply.
    let mut replies = None;
    if live && let Some((pane, id)) = pane.zip(message_id) {
        bridge.core.record_delivered(id, pane, orchestrator);
        replies = Some(id);
    }
    if let Some(card) = card {
        let outcome = outcome.map(|message_id| Receipt {
            owner: CardOwner::of(bot, chat_id),
            message_id,
            original,
        });
        let completion = bridge.desk.get_mut().complete(&card, outcome, replies);
        crate::remote_channel::apply_card_completion(&card.card, completion, edit, cx);
    }
}

/// A failed `sendMessage` is unconfirmed, as a Slack or Discord send is: only
/// a send that never started is known not to have posted.
fn send(prepared: Prepared) -> Result<SendOutcome<i64>, client::ClientError> {
    if prepared.bot.token.revision != keychain::revision() {
        return Ok(SendOutcome::NotSent);
    }
    let token = &prepared.bot.token.token;
    let sent = match prepared.body {
        Body::Plain { text, keyboard } => {
            let event = if keyboard.is_some() {
                "send.approval"
            } else {
                "send.notice"
            };
            let sent = client::send_message(token, prepared.chat_id, &text, None, keyboard);
            trace::delivery(event, || {
                format!(
                    "chat_id={} ok={} text={}",
                    prepared.chat_id,
                    sent.is_ok(),
                    trace::digest(&text)
                )
            });
            sent
        }
        Body::Ping(message) => {
            let chat_id = message.chat_id;
            post_ping(message, |text, parse_mode, keyboard| {
                client::send_message(token, chat_id, text, parse_mode, keyboard)
            })
        }
    };
    sent.map(SendOutcome::Delivered)
}

/// Post a ping as Telegram HTML, resending it as plain text only when Telegram
/// refused the markup. Any other failure may have posted the message — a
/// timeout can land after Telegram took it — and a second copy would offer the
/// same buttons while only one copy is ever edited when the card settles.
fn post_ping(
    message: OutboundMsg,
    mut post: impl FnMut(
        &str,
        Option<&str>,
        Option<client::InlineKeyboard>,
    ) -> Result<i64, client::ClientError>,
) -> Result<i64, client::ClientError> {
    let OutboundMsg {
        chat_id,
        header,
        tail,
        keyboard,
    } = message;
    let html = html_body(&header, &tail);
    match post(&html, Some("HTML"), keyboard.clone()) {
        Err(error) if error.is_markup_refusal() => {
            trace::delivery("send.html_rejected", || {
                format!("chat_id={chat_id} error={error}")
            });
            post(&plain_body(&header, &tail), None, keyboard)
        }
        sent => sent,
    }
}

pub(super) fn spawn_send_task(
    mut outbound_rx: futures::channel::mpsc::UnboundedReceiver<QueuedOutbound>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(crate::remote_channel::cards::RETRY_INTERVAL)
                .await;
            cx.update(flush_edits);
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Some(queued) = outbound_rx.next().await {
            trace::delivery("queue.drained", || {
                format!(
                    "{} kind={}",
                    trace::drain_slot(),
                    trace::outbound_kind(&queued.outbound)
                )
            });
            let Some(prepared) = cx.update(|cx| prepare(queued, cx)) else {
                trace::delivery("send.dropped", || {
                    "reason=owner_or_recipient_changed".into()
                });
                continue;
            };
            let bot = prepared.bot.clone();
            let chat_id = prepared.chat_id;
            let pane = prepared.pane;
            let card = prepared.card.clone();
            let original = prepared.original.clone();
            let result = cx
                .background_executor()
                .spawn(async move { send(prepared) })
                .await;
            cx.update(|cx| {
                finish(
                    &bot,
                    chat_id,
                    pane,
                    card,
                    result
                        .as_ref()
                        .map_or(SendOutcome::Unconfirmed, Clone::clone),
                    original,
                    cx,
                )
            });
            if let Err(error) = result {
                LogWriter::log(
                    ErrorReport::new("Telegram message failed to send")
                        .severity(ErrorSeverity::Warning)
                        .from_error(&error)
                        .at(file!(), line!())
                        .dedup("telegram.send_message")
                        .build(),
                );
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests;
