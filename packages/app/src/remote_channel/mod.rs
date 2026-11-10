//! Remote agent control through Telegram, Slack, and Discord.

pub(crate) mod backoff;
pub mod bridge;
pub(crate) mod cards;
pub(crate) mod command;
pub(crate) mod delivery;
pub(crate) mod dispatch;
pub mod global;
pub mod keychain;
pub(crate) mod lock;
mod pairing;
pub(crate) mod runtime;
pub(crate) mod settings;
pub(crate) mod threads;
pub(crate) mod trace;
pub(crate) mod transport;

pub(crate) fn send_approval_card(
    id: crate::control::approval::ApprovalId,
    summary: String,
    delivery: global::Delivery,
    cx: &mut gpui::App,
) -> bool {
    let telegram = crate::telegram::global::TelegramBridge::send_approval_card(
        id,
        summary.clone(),
        delivery,
        cx,
    );
    global::RemoteChannels::send_approval(id, summary, delivery, cx) || telegram
}

pub(crate) fn send_flow_ask(
    prompt: bridge::FlowAskPrompt,
    delivery: global::Delivery,
    cx: &gpui::App,
) -> bool {
    let telegram =
        crate::telegram::global::TelegramBridge::send_flow_ask(prompt.clone(), delivery, cx);
    global::RemoteChannels::send_flow_ask(prompt, delivery, cx) || telegram
}

pub(crate) fn settle_card(card: cards::CardId, label: &str, cx: &mut gpui::App) {
    crate::telegram::global::TelegramBridge::settle_card(&card, label, cx);
    global::RemoteChannels::settle_card(&card, label, cx);
}

pub(crate) fn apply_card_completion<Receipt>(
    card: &cards::CardId,
    completion: cards::CardCompletion<Receipt>,
    edit: impl FnOnce(Receipt, String, &mut gpui::App),
    cx: &mut gpui::App,
) {
    match completion {
        cards::CardCompletion::Delivered => {
            if let cards::CardId::Approval(id) = card {
                crate::control::approval::remote_card_delivered(*id, cx);
            }
        }
        cards::CardCompletion::Edit { receipt, label } => edit(receipt, label, cx),
        cards::CardCompletion::Ignored
        | cards::CardCompletion::Unconfirmed
        | cards::CardCompletion::Released => {}
    }
}

pub(crate) fn settle_permissions(pane: bridge::PaneRef, session: u64, cx: &mut gpui::App) {
    let label = crate::surface::strings::notification::remote_permission_cancelled();
    crate::telegram::global::TelegramBridge::settle_permissions(pane, session, &label, cx);
    global::RemoteChannels::settle_permissions(pane, session, &label, cx);
}

/// Settle every phone card still offering question `question_id` of `pane`'s
/// session (every question when `None`), on every channel, with `label`.
pub(crate) fn settle_questions(
    pane: bridge::PaneRef,
    session: u64,
    question_id: Option<u64>,
    label: &str,
    cx: &mut gpui::App,
) {
    crate::telegram::global::TelegramBridge::settle_questions(
        pane,
        session,
        question_id,
        label,
        cx,
    );
    global::RemoteChannels::settle_questions(pane, session, question_id, label, cx);
}

/// Introduce a chat on eligible channels. Explicit, solicited delivery selects
/// the chat and records its ping; presence-based delivery is only a Notice,
/// preserving selection and reply routing. Best effort by design: the request has
/// already succeeded, so a channel that went away between the tap and now
/// costs a log line, not an outcome.
pub(crate) fn announce_chat_created(
    pane: bridge::PaneRef,
    header: String,
    agent: &str,
    mode: &str,
    delivery: global::Delivery,
    cx: &mut gpui::App,
) {
    use daruda_store::observability::{
        error_report::{ErrorReport, ErrorSeverity},
        log_writer::LogWriter,
    };
    let tail = match delivery {
        global::Delivery::Explicit => crate::surface::strings::control::chat_announced(agent, mode),
        global::Delivery::Presence { .. } => {
            crate::surface::strings::control::chat_announced_only(agent, mode)
        }
    };
    let telegram = crate::telegram::global::TelegramBridge::announce_chat(
        pane,
        header.clone(),
        tail.clone(),
        delivery,
        cx,
    );
    let remote = global::RemoteChannels::announce_chat(pane, header, tail, delivery, cx);
    if !telegram && !remote && matches!(delivery, global::Delivery::Explicit) {
        LogWriter::log(
            ErrorReport::new("Chat announcement not sent: no remote channel can deliver it")
                .severity(ErrorSeverity::Warning)
                .at(file!(), line!())
                .with_context("pane", crate::remote_channel::trace::pane(pane).to_string())
                .dedup("announce.undeliverable")
                .build(),
        );
    }
}

/// A channel's cards now belong to another identity: a new recipient, bot or
/// connection, or none. Every chat forgets which of its waiting requests the
/// phone was shown, so the next sweep offers them again; a channel that still
/// holds a request's card refuses the repeat. Deferred, because the change is
/// seen while a bridge global is being updated.
pub(crate) fn offer_unseen_cards_again(cx: &mut gpui::App) {
    cx.defer(|cx| {
        crate::window_registry::WindowRegistry::for_each_workspace(cx, |ws, _window, cx| {
            ws.forget_told_to_phone(cx);
        });
    });
}

/// Send a pane's ping to every channel `delivery` reaches, each under its own
/// `only_when_away`. Whether any channel took it.
pub(crate) fn send_ping(
    ping: bridge::BridgePing,
    delivery: global::Delivery,
    cx: &gpui::App,
) -> bool {
    let telegram = crate::telegram::global::TelegramBridge::send_ping(ping.clone(), delivery, cx);
    global::RemoteChannels::send_ping(ping, delivery, cx) || telegram
}

/// Send a pane-less notice to every channel `delivery` reaches. Whether any
/// channel took it.
pub(crate) fn send_notice(text: String, delivery: global::Delivery, cx: &gpui::App) -> bool {
    let telegram =
        crate::telegram::global::TelegramBridge::notify_with_delivery(text.clone(), delivery, cx);
    global::RemoteChannels::send_notice_with_delivery(text, delivery, cx) || telegram
}

/// Tell every channel that can hear it something owed to a command the phone
/// sent earlier. Best effort, like [`announce_chat_created`].
pub(crate) fn send_notice_everywhere(text: String, cx: &gpui::App) {
    use daruda_store::observability::{
        error_report::{ErrorReport, ErrorSeverity},
        log_writer::LogWriter,
    };
    if !send_notice(text, global::Delivery::Explicit, cx) {
        LogWriter::log(
            ErrorReport::new("Notice not sent: no remote channel can deliver it")
                .severity(ErrorSeverity::Warning)
                .at(file!(), line!())
                .dedup("notice.undeliverable")
                .build(),
        );
    }
}

/// Report one remote-channel failure. `#[track_caller]` puts the call site in
/// the log instead of this helper, and `dedup` must be unique per site: a
/// shared key merges a different failure into a live toast's repeat count.
#[track_caller]
pub(crate) fn log_error(message: &str, error: &dyn std::fmt::Display, dedup: &str) {
    use daruda_store::observability::{
        error_report::{ErrorReport, ErrorSeverity},
        log_writer::LogWriter,
    };
    let at = std::panic::Location::caller();
    LogWriter::log(
        ErrorReport::new(message.to_owned())
            .severity(ErrorSeverity::Warning)
            .message(error.to_string())
            .at(at.file(), at.line())
            .dedup(dedup)
            .build(),
    );
}

#[cfg(test)]
mod tests;
