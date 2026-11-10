use super::RemoteChannels;
use crate::remote_channel::{
    bridge::{InlineKeyboard, MessageTail, Outbound, PaneRef},
    cards::{
        Begun, CardId, CardScope, EditOutcome, SendClaim, SendOutcome, Settlement, quit::QuitEdit,
    },
    runtime::BotSession,
    threads::driver::{self, ThreadPlan},
    transport::{self, Message},
};
use daruda_config::remote::{ChannelKind, RemoteRecipient};
use futures::{
    StreamExt,
    channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
};
use gpui::App;
use std::{collections::HashMap, sync::Arc};

pub(crate) struct Pending {
    pub id: String,
    pub generation: uuid::Uuid,
    pub bot: Arc<BotSession>,
    pub recipient: RemoteRecipient,
    pub outbound: Outbound,
    pub card: Option<SendClaim>,
}

/// A sent card's messages, editable by whichever session serves the identity
/// that sent them.
#[derive(Clone)]
pub(super) struct Receipt {
    id: String,
    owner: String,
    recipient: RemoteRecipient,
    ids: Vec<String>,
    original: String,
}

pub(super) struct Prepared {
    kind: ChannelKind,
    bot: Arc<BotSession>,
    recipient: RemoteRecipient,
    message: Message,
    pane: Option<PaneRef>,
    original: String,
    /// The pane's thread, on a channel that gives panes their own.
    thread: Option<ThreadPlan>,
}

pub(super) fn prepare(pending: &Pending, cx: &mut App) -> Option<Prepared> {
    if !current(pending, cx) {
        discard(pending, cx);
        return None;
    }
    let connection = cx
        .global_mut::<RemoteChannels>()
        .connections
        .get_mut(&pending.id)?;
    let kind = connection.config.kind;
    let begun = connection
        .desk
        .get_mut()
        .begin(pending.card.as_ref(), pending.outbound.clone())?;
    let (message, pane) = match begun {
        Begun::Ping { pane, message } => (message, Some(pane)),
        Begun::Notice(text) => (Message::plain(text, None), None),
        Begun::FlowAsk(message) => (message, None),
        Begun::Approval(prompt) => (
            Message::plain(
                prompt.summary,
                Some(InlineKeyboard::single_row(prompt.buttons.to_vec())),
            ),
            None,
        ),
    };
    let tail = match &message.tail {
        MessageTail::Plain(text) | MessageTail::Markdown(text) => text,
    };
    let original = if message.header.is_empty() {
        tail.clone()
    } else {
        format!("{}\n{tail}", message.header)
    };
    let thread = pane.and_then(|pane| {
        driver::plan(
            &pending.id,
            kind,
            &pending.bot,
            &pending.recipient,
            pane,
            cx,
        )
    });
    Some(Prepared {
        kind,
        bot: pending.bot.clone(),
        recipient: pending.recipient.clone(),
        message,
        pane,
        original,
        thread,
    })
}

fn discard(pending: &Pending, cx: &mut App) {
    if let Some(card) = &pending.card
        && let Some(connection) = cx
            .global_mut::<RemoteChannels>()
            .connections
            .get_mut(&pending.id)
    {
        connection.desk.get_mut().abandon(card);
    }
}

/// Whether `pending` was sent by the session still serving its connection.
fn current(pending: &Pending, cx: &App) -> bool {
    cx.global::<RemoteChannels>()
        .live(&pending.id, cx)
        .is_some_and(|connection| {
            connection.serves(pending.generation, &pending.bot, &pending.recipient)
        })
}

/// The session that may edit `receipt`'s messages: any live one serving the
/// identity and recipient that sent them.
fn editor(receipt: &Receipt, cx: &App) -> Option<(ChannelKind, Arc<BotSession>)> {
    let connection = cx.global::<RemoteChannels>().live(&receipt.id, cx)?;
    let serves = connection.can_send()
        && connection.config.recipient.as_ref() == Some(&receipt.recipient)
        && connection.desk.borrow().owner() == Some(&receipt.owner);
    if !serves {
        return None;
    }
    Some((connection.config.kind, connection.bot.clone()?))
}

pub(super) fn edit(receipt: Receipt, label: String, cx: &mut App) {
    let Some(connection) = cx
        .global_mut::<RemoteChannels>()
        .connections
        .get_mut(&receipt.id)
    else {
        return;
    };
    let desk = connection.desk.get_mut();
    if desk.owner() != Some(&receipt.owner) {
        return;
    }
    for id in &receipt.ids {
        desk.note_settled(id.clone());
    }
    let id = receipt.id.clone();
    desk.edits_mut().push(receipt, label);
    flush_connection(&id, cx);
}

pub(super) fn flush_edits(cx: &mut App) {
    let ids: Vec<_> = cx
        .global::<RemoteChannels>()
        .connections
        .keys()
        .cloned()
        .collect();
    for id in ids {
        flush_connection(&id, cx);
    }
}

fn flush_connection(id: &str, cx: &mut App) {
    let Some(connection) = cx.global_mut::<RemoteChannels>().connections.get_mut(id) else {
        return;
    };
    if !connection.can_send() {
        return;
    }
    let pending = connection.desk.get_mut().edits_mut().claim();
    for (attempt, receipt, label) in pending {
        if editor(&receipt, cx).is_none() {
            finish_edit(&receipt, attempt, EditOutcome::Transient, cx);
            continue;
        }
        #[cfg(test)]
        if let Some(result) = crate::remote_channel::cards::tests::capture(
            &receipt.id,
            receipt.ids.clone(),
            &receipt.original,
            &label,
            cx,
        ) {
            let outcome = EditOutcome::logged(result, EDIT_FAILED, EDIT_DEDUP);
            finish_edit(&receipt, attempt, outcome, cx);
            continue;
        }
        cx.spawn(async move |cx| {
            // A gone message does not stop the rest of a split card from
            // losing its buttons; a transient failure retries all of them.
            let mut outcome = EditOutcome::Edited;
            for message_id in &receipt.ids {
                let serving = cx.update(|cx| {
                    let connection = cx.global::<RemoteChannels>().connections.get(&receipt.id)?;
                    if !connection.desk.borrow().edits().contains(attempt) {
                        return None;
                    }
                    editor(&receipt, cx)
                });
                let Some((kind, bot)) = serving else {
                    outcome = EditOutcome::Transient;
                    break;
                };
                let recipient = receipt.recipient.clone();
                let original = receipt.original.clone();
                let label = label.clone();
                let message_id = message_id.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        if !bot.is_live() {
                            return None;
                        }
                        Some(transport::edit(
                            kind,
                            &bot.credentials,
                            &recipient,
                            &message_id,
                            &original,
                            &label,
                        ))
                    })
                    .await;
                let Some(result) = result else {
                    outcome = EditOutcome::Transient;
                    break;
                };
                match EditOutcome::logged(result, EDIT_FAILED, EDIT_DEDUP) {
                    EditOutcome::Edited => {}
                    EditOutcome::Gone => outcome = EditOutcome::Gone,
                    EditOutcome::Transient => {
                        outcome = EditOutcome::Transient;
                        break;
                    }
                }
            }
            cx.update(|cx| finish_edit(&receipt, attempt, outcome, cx));
        })
        .detach();
    }
}

/// One quit edit, carrying the session's credentials so it runs without the
/// app. With no retry after quit, a failed message does not stop the rest
/// of a split card.
fn quit_edit(kind: ChannelKind, bot: Arc<BotSession>, receipt: Receipt, label: String) -> QuitEdit {
    QuitEdit::new(QUIT_EDIT_FAILED, QUIT_EDIT_DEDUP, move || {
        let mut result = Ok(());
        for message_id in &receipt.ids {
            let edited = transport::edit(
                kind,
                &bot.credentials,
                &receipt.recipient,
                message_id,
                &receipt.original,
                &label,
            );
            if let Err(failure) = edited {
                result = Err(failure);
            }
        }
        result
    })
}

const QUIT_EDIT_FAILED: &str = "Remote card failed to edit at quit";
const QUIT_EDIT_DEDUP: &str = "remote.card.quit_edit";

const EDIT_FAILED: &str = "Remote settled card failed to edit";
const EDIT_DEDUP: &str = "remote.card.edit";

fn finish_edit(receipt: &Receipt, attempt: uuid::Uuid, outcome: EditOutcome, cx: &mut App) {
    if let Some(connection) = cx
        .global_mut::<RemoteChannels>()
        .connections
        .get_mut(&receipt.id)
    {
        connection
            .desk
            .get_mut()
            .edits_mut()
            .finish(attempt, outcome);
    }
}

impl RemoteChannels {
    /// The edits every serving connection owes as the app quits, closing
    /// their cards. A connection not serving right now keeps its cards for
    /// tap-time cleanup, as it would for a settlement.
    pub(crate) fn quit_edits(cancelled: &str, cx: &mut App) -> Vec<QuitEdit> {
        let Some(channels) = cx.try_global::<Self>() else {
            return Vec::new();
        };
        let serving: Vec<_> = channels
            .connections
            .keys()
            .filter(|id| {
                channels
                    .live(id, cx)
                    .is_some_and(|connection| connection.can_send())
            })
            .cloned()
            .collect();
        let mut owed = Vec::new();
        for id in serving {
            let Some(connection) = cx.global_mut::<Self>().connections.get_mut(&id) else {
                continue;
            };
            let desk = connection.desk.get_mut();
            for (receipt, label) in desk.close_for_quit(cancelled) {
                for message in &receipt.ids {
                    desk.note_settled(message.clone());
                }
                owed.push((receipt, label));
            }
        }
        let mut edits = Vec::new();
        for (receipt, label) in owed {
            let Some((kind, bot)) = editor(&receipt, cx) else {
                continue;
            };
            #[cfg(test)]
            if let Some(result) = crate::remote_channel::cards::tests::capture(
                &receipt.id,
                receipt.ids.clone(),
                &receipt.original,
                &label,
                cx,
            ) {
                edits.push(QuitEdit::new(
                    QUIT_EDIT_FAILED,
                    QUIT_EDIT_DEDUP,
                    move || result,
                ));
                continue;
            }
            edits.push(quit_edit(kind, bot, receipt, label));
        }
        edits
    }

    pub(crate) fn settle_card(card: &CardId, label: &str, cx: &mut App) {
        if !cx.has_global::<Self>() {
            return;
        }
        let mut edits = Vec::new();
        for connection in cx.global_mut::<Self>().connections.values_mut() {
            if let Settlement::Edit { receipt, label } =
                connection.desk.get_mut().settle(card, label.to_owned())
            {
                edits.push((receipt, label));
            }
        }
        for (receipt, label) in edits {
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
        let cards: Vec<_> = cx
            .try_global::<Self>()
            .into_iter()
            .flat_map(|bridge| bridge.connections.values())
            .flat_map(|connection| connection.desk.borrow().open(scope))
            .collect();
        for card in cards {
            Self::settle_card(&card, label, cx);
        }
    }
}

/// Record what a send came to. A card that may have reached the person is
/// kept even if its session was replaced meanwhile: the ledger's claim decides
/// whether it still belongs to this identity, and a same-bot replacement can
/// still edit it or receive its taps, so the pump must not offer it again.
pub(super) fn finish(
    pending: &Pending,
    pane: Option<PaneRef>,
    outcome: SendOutcome<Vec<String>>,
    original: String,
    cx: &mut App,
) {
    let live = current(pending, cx);
    let orchestrator = crate::orchestrator::pane(cx);
    let Some(connection) = cx
        .global_mut::<RemoteChannels>()
        .connections
        .get_mut(&pending.id)
    else {
        return;
    };
    let ids: &[String] = match &outcome {
        SendOutcome::Delivered(ids) => ids,
        SendOutcome::Unconfirmed | SendOutcome::NotSent => &[],
    };
    // Only the live session's messages take a reply.
    let replies = match (live, pane) {
        (true, Some(pane)) => {
            for id in ids {
                connection
                    .core
                    .record_delivered(id.clone(), pane, orchestrator);
            }
            ids.to_vec()
        }
        _ => Vec::new(),
    };
    if let Some(card) = &pending.card {
        let desk = connection.desk.get_mut();
        let outcome = match (outcome, desk.owner()) {
            (SendOutcome::Delivered(ids), Some(owner)) => SendOutcome::Delivered(Receipt {
                id: pending.id.clone(),
                owner: owner.clone(),
                recipient: pending.recipient.clone(),
                ids,
                original,
            }),
            // Without a bound identity no edit can own the messages.
            (SendOutcome::Delivered(_), None) => SendOutcome::Unconfirmed,
            (SendOutcome::Unconfirmed, _) => SendOutcome::Unconfirmed,
            (SendOutcome::NotSent, _) => SendOutcome::NotSent,
        };
        let completion = desk.complete(card, outcome, replies);
        crate::remote_channel::apply_card_completion(&card.card, completion, edit, cx);
    }
}

/// Each connection drains in order; a throttled channel does not block another.
pub(super) fn spawn(mut receiver: UnboundedReceiver<Pending>, cx: &mut App) {
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
        let mut queues: HashMap<String, UnboundedSender<Pending>> = HashMap::new();
        while let Some(pending) = receiver.next().await {
            let sender = queues.entry(pending.id.clone()).or_insert_with(|| {
                let (sender, receiver) = unbounded();
                cx.update(|cx| spawn_connection(receiver, cx));
                sender
            });
            if let Err(error) = sender.unbounded_send(pending) {
                crate::remote_channel::log_error(
                    "Remote connection queue closed",
                    &error,
                    "remote.queue.connection",
                );
                cx.update(|cx| discard(&error.into_inner(), cx));
            }
        }
    })
    .detach();
}

/// The transports report every failure as one message-only error, so a send
/// that posted no chunk is unconfirmed: the platform may have taken it before
/// the reply was lost. Any posted chunk is a receipt settlement can edit.
fn sent_outcome(sent: Vec<String>) -> SendOutcome<Vec<String>> {
    if sent.is_empty() {
        SendOutcome::Unconfirmed
    } else {
        SendOutcome::Delivered(sent)
    }
}

fn spawn_connection(mut receiver: UnboundedReceiver<Pending>, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Some(pending) = receiver.next().await {
            let Some(prepared) = cx.update(|cx| prepare(&pending, cx)) else {
                continue;
            };
            let pane = prepared.pane;
            let original = prepared.original.clone();
            let (outcome, result, thread) = cx
                .background_executor()
                .spawn(async move {
                    if !prepared.bot.is_live() {
                        return (SendOutcome::NotSent, Ok(Vec::new()), None);
                    }
                    let mut sent = Vec::new();
                    let record = |id: &str| sent.push(id.to_owned());
                    let (result, started) = match &prepared.thread {
                        Some(plan) => {
                            let posted = driver::deliver(
                                plan,
                                &prepared.bot.credentials,
                                &prepared.recipient,
                                prepared.message,
                                record,
                            );
                            (posted.result, posted.root)
                        }
                        None => (
                            transport::send_recording(
                                prepared.kind,
                                &prepared.bot.credentials,
                                &prepared.recipient,
                                prepared.message,
                                record,
                            ),
                            None,
                        ),
                    };
                    let started = prepared.thread.zip(started);
                    (sent_outcome(sent), result, started)
                })
                .await;
            cx.update(|cx| {
                if let Some((plan, root)) = &thread {
                    driver::record_root(plan, root.clone(), cx);
                }
                finish(&pending, pane, outcome, original, cx);
            });
            if let Err(error) = result {
                crate::remote_channel::log_error(
                    "Remote message failed to send",
                    &error,
                    "remote.message.send",
                );
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests;
