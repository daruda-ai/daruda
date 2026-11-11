//! A pane as its thread root shows it: the name in bold, what it is working
//! on below, then its status.

use super::SECTION_LIMIT;
use crate::remote_channel::transport::chunks;
use serde_json::{Value, json};

pub(super) struct PaneLines<'a> {
    pub title: &'a str,
    /// What the pane's conversation is about, under its name.
    pub subject: Option<&'a str>,
    pub status: Option<StatusLine<'a>>,
}

/// The last line: the pane's state.
pub(super) struct StatusLine<'a> {
    /// Slack emoji name, when the state has one.
    pub emoji: Option<&'a str>,
    pub label: &'a str,
}

impl PaneLines<'_> {
    /// The elements of one rich_text section, so a pane's name is shown
    /// literally: a `_` or `*` in it stays itself.
    pub(super) fn elements(&self) -> Vec<Value> {
        let mut elements =
            vec![json!({"type": "text", "text": capped(self.title), "style": {"bold": true}})];
        if let Some(subject) = self.subject {
            elements.push(text(&format!("\n{subject}")));
        }
        if let Some(status) = &self.status {
            elements.push(text("\n"));
            match status.emoji {
                Some(emoji) => {
                    elements.push(json!({"type": "emoji", "name": emoji}));
                    elements.push(text(&format!(" {}", status.label)));
                }
                None => elements.push(text(status.label)),
            }
        }
        elements
    }
}

fn text(text: &str) -> Value {
    json!({"type": "text", "text": capped(text)})
}

fn capped(text: &str) -> String {
    chunks(text, SECTION_LIMIT)
        .into_iter()
        .next()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Name, subject and status each take their own line, in that order; a
    /// pane without a subject or a status reads on fewer lines.
    #[test]
    fn a_pane_reads_name_subject_then_status() {
        let status = |emoji| StatusLine {
            emoji,
            label: "Working",
        };
        let cases = [
            (
                PaneLines {
                    title: "app · Claude",
                    subject: Some("Fix the bug"),
                    status: Some(status(Some("hourglass"))),
                },
                json!([
                    {"type": "text", "text": "app · Claude", "style": {"bold": true}},
                    {"type": "text", "text": "\nFix the bug"},
                    {"type": "text", "text": "\n"},
                    {"type": "emoji", "name": "hourglass"},
                    {"type": "text", "text": " Working"},
                ]),
                "every line",
            ),
            (
                PaneLines {
                    title: "app · Claude",
                    subject: None,
                    status: Some(status(None)),
                },
                json!([
                    {"type": "text", "text": "app · Claude", "style": {"bold": true}},
                    {"type": "text", "text": "\n"},
                    {"type": "text", "text": "Working"},
                ]),
                "no subject, no emoji",
            ),
            (
                PaneLines {
                    title: "app · Claude",
                    subject: None,
                    status: None,
                },
                json!([{"type": "text", "text": "app · Claude", "style": {"bold": true}}]),
                "the name alone",
            ),
        ];
        for (lines, want, label) in cases {
            assert_eq!(Value::from(lines.elements()), want, "{label}");
        }
    }

    #[test]
    fn long_lines_are_capped() {
        let long = "n".repeat(SECTION_LIMIT + 10);
        let lines = PaneLines {
            title: &long,
            subject: Some(&long),
            status: None,
        };
        let elements = lines.elements();
        let length = |value: &Value| value["text"].as_str().unwrap().chars().count();
        assert_eq!(length(&elements[0]), SECTION_LIMIT);
        assert_eq!(length(&elements[1]), SECTION_LIMIT);
    }
}
