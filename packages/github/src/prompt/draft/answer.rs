//! Reading the title and the description out of what an agent printed.

use serde::Deserialize;

use super::DraftText;
use crate::prompt::{one_line, strip_control};

/// GitHub refuses a longer title.
const TITLE_CHARS: usize = 256;
/// GitHub refuses a description past 65,536 characters; cut short of it.
const BODY_CHARS: usize = 60_000;

#[derive(Deserialize)]
struct Answer {
    title: String,
    #[serde(default)]
    body: String,
}

/// The title and description in what an agent printed, or `None` when it
/// holds no JSON object with a title. Text around the object (a banner, a
/// code fence) is skipped: the first object that has a title wins. Objects
/// are taken whole, so one inside another's string (a code sample in the
/// body) is never read as the answer. The prompt's own example, echoed
/// back with its `...` title, is not one.
pub fn parse_pr_draft(output: &str) -> Option<DraftText> {
    let mut rest = 0;
    while let Some(at) = output[rest..].find('{').map(|found| rest + found) {
        let Some(object) = object_at(&output[at..]) else {
            // Never closed, so not an object: a stray brace in a banner.
            rest = at + 1;
            continue;
        };
        if let Some(text) = draft_in(&object.json) {
            return Some(text);
        }
        rest = at + object.len;
    }
    None
}

/// One `{…}` read from the start of a text.
struct Object {
    /// Bytes of the text it spans.
    len: usize,
    /// Its text, with the line breaks and tabs an agent left raw inside a
    /// string escaped as JSON requires.
    json: String,
}

/// The object `text` starts with, up to the brace that closes it; braces
/// inside its strings do not count. `None` when it is never closed.
fn object_at(text: &str) -> Option<Object> {
    let mut json = String::new();
    let (mut depth, mut in_string, mut escaped) = (0_usize, false, false);
    for (at, c) in text.char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                '\n' => {
                    json.push_str("\\n");
                    continue;
                }
                '\r' => {
                    json.push_str("\\r");
                    continue;
                }
                '\t' => {
                    json.push_str("\\t");
                    continue;
                }
                _ => {}
            }
        } else {
            match c {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        json.push(c);
                        return Some(Object {
                            len: at + c.len_utf8(),
                            json,
                        });
                    }
                }
                _ => {}
            }
        }
        json.push(c);
    }
    None
}

/// The draft `json`, one object, holds: its title on one line and both
/// within what GitHub takes.
fn draft_in(json: &str) -> Option<DraftText> {
    let answer: Answer = serde_json::from_str(json).ok()?;
    let title: String = one_line(&answer.title)
        .trim()
        .chars()
        .take(TITLE_CHARS)
        .collect();
    let placeholder = title.chars().all(|c| c == '.' || c == '…');
    (!title.is_empty() && !placeholder).then(|| DraftText {
        title,
        body: strip_control(&answer.body)
            .trim()
            .chars()
            .take(BODY_CHARS)
            .collect(),
    })
}

#[cfg(test)]
mod tests;
