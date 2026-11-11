//! Shared transport vocabulary and bounded HTTP/WebSocket I/O.

use super::bridge::{InlineKeyboard, MessageTail, PreparedPing};
use daruda_config::remote::{ChannelKind, RemoteRecipient};

pub(crate) mod discord;
mod format;
pub(crate) mod http;
pub(crate) mod slack;
pub(crate) mod socket;

/// Deliberately not Debug: interaction receipts and credentials are secrets.
#[derive(Clone)]
pub struct Credentials {
    pub bot: String,
    pub app: Option<String>,
}

pub struct Incoming {
    pub event_id: String,
    pub sender: RemoteRecipient,
    pub kind: IncomingKind,
}

pub enum IncomingKind {
    Message {
        text: String,
        reply_to: Option<String>,
    },
    Callback {
        data: String,
        /// The tapped message, so a button that names nothing live can be
        /// taken off it.
        message_id: String,
        original: String,
    },
}

/// Something the person did on the app's own page rather than in the paired
/// conversation. Names the person, never a conversation.
pub struct HomeEvent {
    pub event_id: String,
    pub user_id: String,
    pub team_id: String,
    pub kind: HomeEventKind,
}

pub enum HomeEventKind {
    /// The person opened the page: show it the panes as they are now.
    Opened,
    /// The person tapped a button on the page that carries `data`.
    Tap { data: String },
}

pub type Message = PreparedPing;

/// One message a send posted: its id, the text it shows, and whether it
/// carries buttons. Settling a card rewrites exactly the parts with buttons,
/// each with its own text, so a long card split over several messages does
/// not have every part overwritten with the first one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentPart {
    pub id: String,
    pub text: String,
    pub buttons: bool,
}

impl PreparedPing {
    pub fn plain(text: String, keyboard: Option<InlineKeyboard>) -> Self {
        Self {
            header: String::new(),
            tail: MessageTail::Plain(text),
            keyboard,
        }
    }
}

#[derive(Debug)]
pub enum TransportError {
    Message(String),
    Closed(u16),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(message) => f.write_str(message),
            Self::Closed(code) => write!(f, "Gateway closed with code {code}"),
        }
    }
}

impl std::error::Error for TransportError {}

pub type Result<T> = std::result::Result<T, TransportError>;

pub fn send(
    kind: ChannelKind,
    credentials: &Credentials,
    recipient: &RemoteRecipient,
    message: Message,
) -> Result<Vec<String>> {
    send_recording(kind, credentials, recipient, message, |_| {})
}

pub fn send_recording(
    kind: ChannelKind,
    credentials: &Credentials,
    recipient: &RemoteRecipient,
    message: Message,
    on_sent: impl FnMut(SentPart),
) -> Result<Vec<String>> {
    match kind {
        ChannelKind::Slack => slack::send(credentials, recipient, message, on_sent),
        ChannelKind::Discord => discord::send(credentials, recipient, message, on_sent),
    }
}

/// Whether `kind` gives each pane a thread of its own. The one question asked
/// before using a transport's thread API; other channels post at the top.
pub(crate) fn pane_threads(kind: ChannelKind) -> bool {
    matches!(kind, ChannelKind::Slack)
}

/// The page of the app's own on which daruda shows its panes, on channels
/// that have one: how much it holds and how to write to it.
#[derive(Clone, Copy)]
pub(crate) struct AppHome {
    /// What one page's panes may take.
    pub budget: PageBudget,
    /// Show the view as person `user_id`'s page, replacing what it showed.
    pub publish: fn(&Credentials, &str, &PageView<'_>) -> PageVerdict,
    /// The link that opens message `message_id` of `conversation_id`: a
    /// thread's root opens its thread.
    pub link: fn(&Credentials, &str, &str) -> std::result::Result<String, LinkFailure>,
    /// What turns the page back on once the channel says it is off.
    pub off_hint: &'static str,
}

/// The room a page has for its [`PagePart`]s, as the transport draws them:
/// a heading or a label takes one block, and a run of `n` rows takes
/// `ceil(n / cards_per_block)`. The page's frame (header, counts, result,
/// dividers, the line counting what is left out, help) is not counted here:
/// the transport keeps its own room for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PageBudget {
    /// Blocks the parts may take.
    pub blocks: usize,
    /// Rows one block holds side by side.
    pub cards_per_block: usize,
    /// Rows the page shows in all, a pane shown twice counted twice.
    pub cards: usize,
}

impl PageBudget {
    /// The blocks a run of `rows` rows takes.
    pub(crate) fn rows_blocks(&self, rows: usize) -> usize {
        rows.div_ceil(self.cards_per_block.max(1))
    }
}

/// What a page shows, in the order it shows it.
pub(crate) struct PageView<'a> {
    pub header: &'a str,
    /// How many panes are open and how many run, when any are.
    pub summary: Option<&'a str>,
    /// What the person's last tap came to, under the header.
    pub outcome: Option<&'a str>,
    /// Said in place of the parts when there are no panes.
    pub empty: &'a str,
    /// The panes under their headings, top to bottom.
    pub parts: Vec<PagePart<'a>>,
    /// The line that counts the panes left out, when some were.
    pub more: Option<&'a str>,
    /// How to use the page and the conversation, at its foot.
    pub help: PageHelp<'a>,
}

/// One piece of a page's listing. Names are shown as written, never read as
/// markup or emoji.
pub(crate) enum PagePart<'a> {
    /// Heads what follows: the panes needing the person, a window, a group,
    /// or a project outside any group.
    Heading(&'a str),
    /// A quieter line naming what follows: a project in a group, or a lane.
    Label(&'a str),
    /// Panes shown side by side.
    Rows(Vec<PageRow<'a>>),
}

/// The usage help at the foot of a page.
pub(crate) struct PageHelp<'a> {
    pub title: &'a str,
    /// One instruction each, in Markdown: commands in code spans.
    pub lines: Vec<&'a str>,
    /// Each kind of status: its emoji, by short name, and its label.
    pub legend: Vec<(&'a str, &'a str)>,
}

/// One pane on the page: what its conversation is called, which agent or
/// project it belongs to, its status, and its buttons.
pub(crate) struct PageRow<'a> {
    pub title: &'a str,
    /// What the title leaves unsaid: the agent, and under no project heading
    /// the project too.
    pub subtitle: Option<&'a str>,
    /// The status's emoji, by short name, when one is known.
    pub emoji: Option<&'a str>,
    /// The status's words, marked when the pane is the one commands address.
    pub status: &'a str,
    /// The button's label and the link to the pane's thread.
    pub thread: Option<(&'a str, &'a str)>,
    /// The select button's label and value.
    pub select: (&'a str, &'a str),
    /// The stop button's label and value, when the pane can be stopped.
    pub stop: Option<(&'a str, &'a str)>,
}

/// How publishing a page went.
#[derive(Debug)]
pub(crate) enum PageVerdict {
    Published,
    /// The channel has the page turned off: none takes a view until it is on.
    Off,
    /// Rate limits, timeouts, server trouble: may pass.
    Transient(String),
    /// The channel refused this view: credentials, scopes, or the payload.
    Refused(String),
}

/// Why the link to a message could not be had.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LinkFailure {
    /// Rate limits, timeouts, server trouble: may pass.
    Transient(String),
    /// The channel refused to link this message: credentials, scopes, or a
    /// message it does not know. Asking again meets the same answer.
    Refused(String),
}

impl std::fmt::Display for LinkFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient(reason) | Self::Refused(reason) => f.write_str(reason),
        }
    }
}

/// The page of the app's own `kind` has, if any. The one question asked
/// before showing panes there.
pub(crate) fn app_home(kind: ChannelKind) -> Option<AppHome> {
    match kind {
        ChannelKind::Slack => Some(slack::home::APP_HOME),
        ChannelKind::Discord => None,
    }
}

pub fn edit(
    kind: ChannelKind,
    credentials: &Credentials,
    recipient: &RemoteRecipient,
    message_id: &str,
    original: &str,
    label: &str,
) -> std::result::Result<(), crate::remote_channel::cards::EditFailure> {
    match kind {
        ChannelKind::Slack => slack::edit(credentials, recipient, message_id, original, label),
        ChannelKind::Discord => discord::edit(credentials, recipient, message_id, original, label),
    }
}

/// Split on scalar boundaries while measuring UTF-16, the stricter platform unit.
pub(crate) fn feedback(original: &str, label: &str, limit: usize) -> String {
    // A part that carried only buttons has no text to keep above the label.
    if original.is_empty() {
        return chunks(label, limit).into_iter().next().unwrap_or_default();
    }
    let suffix = format!("\n\n{label}");
    let reserve = suffix.encode_utf16().count();
    let prefix = chunks(original, limit.saturating_sub(reserve).max(2))
        .into_iter()
        .next()
        .unwrap_or_default();
    format!("{prefix}{suffix}")
}

pub(crate) fn chunks(text: &str, limit: usize) -> Vec<String> {
    let mut result = Vec::new();
    let mut part = String::new();
    let mut size = 0;
    for ch in text.chars() {
        if size + ch.len_utf16() > limit && !part.is_empty() {
            result.push(std::mem::take(&mut part));
            size = 0;
        }
        part.push(ch);
        size += ch.len_utf16();
    }
    if !part.is_empty() {
        result.push(part);
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_full_length_prompt_keeps_its_decision_after_editing() {
        let body = super::feedback(&"x".repeat(2000), "Allowed", 2000);
        assert!(body.ends_with("Allowed"));
        assert!(body.encode_utf16().count() <= 2000);
    }
    #[test]
    fn a_buttons_only_part_becomes_just_its_decision() {
        assert_eq!(super::feedback("", "Allowed", 2000), "Allowed");
    }

    #[test]
    fn unicode_messages_are_lossless_and_bounded() {
        let text = "A\u{1f600}\u{d55c}".repeat(1000);
        let chunks = super::chunks(&text, 2000);
        assert_eq!(chunks.concat(), text);
        assert!(
            chunks
                .iter()
                .all(|part| part.encode_utf16().count() <= 2000)
        );
    }
}
