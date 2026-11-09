//! Telegram's own bridge types. The vocabulary every channel shares lives in
//! [`crate::remote_channel::bridge`].

use crate::remote_channel::bridge::{InlineKeyboard, MessageTail};

mod core;
pub use core::BridgeCore;

/// What the caller should actually send to Telegram. Telegram-only: it names
/// its recipient by `chat_id`, where the other channels carry a
/// `RemoteRecipient` alongside a `PreparedPing`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundMsg {
    pub chat_id: i64,
    pub header: String,
    pub tail: MessageTail,
    pub keyboard: Option<InlineKeyboard>,
}
