//! The `<conversation_handoff>` block: writing it and reading it back.
//!
//! ```text
//! <conversation_handoff from-agent=".." from-account=".." to-agent=".." to-account="..">
//! {preamble addressed to the agent}
//!
//! <handoff_record>
//! {transcript}
//! </handoff_record>
//! </conversation_handoff>
//! ```
//!
//! Recorded text is neutralized so it can open or close neither tag; the first
//! literal closing tag of each is therefore the block's own.

use super::{HandoffItem, HandoffMark, HandoffParty};

const OPEN_TAG: &str = "<conversation_handoff";
const CLOSE_TAG: &str = "</conversation_handoff>";
const RECORD_OPEN: &str = "<handoff_record>";
const RECORD_CLOSE: &str = "</handoff_record>";
/// Every tag name recorded text must not be able to write.
const TAG_NAMES: [&str; 2] = ["conversation_handoff", "handoff_record"];

/// The block carrying `transcript`, introduced to the agent by `preamble`.
pub(super) fn wrap(mark: &HandoffMark, preamble: &str, transcript: &str) -> String {
    format!(
        "{open}>\n{preamble}\n\n{RECORD_OPEN}\n{transcript}\n{RECORD_CLOSE}\n{CLOSE_TAG}",
        open = open_tag(mark),
        preamble = neutralize_tags(preamble),
        transcript = neutralize_tags(transcript),
    )
}

/// Split a prompt that starts with a recap block into the divider it stands
/// for — carrying the block's record — and the text after it, or `None` when
/// `text` does not start with one.
///
/// The separator the host puts between a briefing and the prompt (`---`) is
/// dropped with the block.
pub fn split_handoff(text: &str) -> Option<(HandoffItem, &str)> {
    let body = text.trim_start().strip_prefix(OPEN_TAG)?;
    let tag_end = body.find('>')?;
    let mark = parse_mark(&body[..tag_end])?;
    let after_open = &body[tag_end + 1..];
    let close = after_open.find(CLOSE_TAG)?;
    let record = record_of(&after_open[..close]);
    let rest = after_open[close + CLOSE_TAG.len()..].trim_start();
    let rest = rest.strip_prefix("---").map_or(rest, str::trim_start);
    Some((HandoffItem { mark, record }, rest))
}

/// The text between the record tags, or `None` when the block has none.
fn record_of(block: &str) -> Option<String> {
    let start = block.find(RECORD_OPEN)? + RECORD_OPEN.len();
    let end = start + block[start..].find(RECORD_CLOSE)?;
    let record = block[start..end].trim();
    (!record.is_empty()).then(|| record.to_owned())
}

/// `text` with every opening or closing tag of the block's own names
/// disarmed, case-insensitively.
fn neutralize_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after = &rest[lt + 1..];
        let name_at = usize::from(after.starts_with('/'));
        let is_tag = TAG_NAMES.iter().any(|name| {
            after
                .get(name_at..name_at + name.len())
                .is_some_and(|found| found.eq_ignore_ascii_case(name))
        });
        out.push_str(if is_tag { "&lt;" } else { "<" });
        rest = after;
    }
    out.push_str(rest);
    out
}

fn open_tag(mark: &HandoffMark) -> String {
    let mut tag = String::from(OPEN_TAG);
    let attrs = [
        ("from-agent", Some(&mark.from.agent)),
        ("from-account", mark.from.account.as_ref()),
        ("to-agent", Some(&mark.to.agent)),
        ("to-account", mark.to.account.as_ref()),
    ];
    for (name, value) in attrs {
        if let Some(value) = value {
            tag.push_str(&format!(" {name}=\"{}\"", escape_attr(value)));
        }
    }
    tag
}

fn parse_mark(attrs: &str) -> Option<HandoffMark> {
    let mut values: [Option<String>; 4] = Default::default();
    let names = ["from-agent", "from-account", "to-agent", "to-account"];
    let mut rest = attrs.trim_start();
    while !rest.is_empty() {
        let eq = rest.find("=\"")?;
        let name = rest[..eq].trim();
        let value_start = &rest[eq + 2..];
        let close = value_start.find('"')?;
        // An attribute a later build added is skipped, not fatal, so that
        // build's dividers still fold here.
        if let Some(slot) = names.iter().position(|known| *known == name) {
            values[slot] = Some(unescape_attr(&value_start[..close]));
        }
        rest = value_start[close + 1..].trim_start();
    }
    let [from_agent, from_account, to_agent, to_account] = values;
    Some(HandoffMark {
        from: HandoffParty {
            agent: from_agent?,
            account: from_account,
        },
        to: HandoffParty {
            agent: to_agent?,
            account: to_account,
        },
    })
}

fn escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn unescape_attr(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests;
