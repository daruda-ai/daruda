//! Filling the template variables a flow may use: the five run variables
//! and the flow's declared inputs. Every value is substituted as an absolute
//! path, a rendered line or an input's text; only the shell surface quotes
//! them, because a prompt is prose and a command is not.

use crate::NodeId;
use crate::runner::NodeFailure;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// Where a rendered string is about to go. A prompt is prose an agent
/// reads; a command is a line a shell parses, so only the latter quotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Prompt,
    Shell,
}

/// Everything the variables can resolve to for one render.
pub struct TemplateContext<'a> {
    pub run_dir: &'a Path,
    /// The rendering node's own output, if it has one.
    pub output: Option<&'a Path>,
    /// Every node's output path, for `{{node.<id>.output}}`.
    pub node_outputs: &'a HashMap<NodeId, PathBuf>,
    /// The failure a repair or retry is responding to.
    pub failure: Option<&'a NodeFailure>,
    /// Archived evidence from earlier attempts.
    pub attempts: &'a [PathBuf],
    /// The run's input values, already resolved to text, for
    /// `{{input.<name>}}`.
    pub inputs: &'a BTreeMap<String, String>,
}

/// Substitute the variables. An unknown `{{…}}` is left verbatim —
/// static validation already rejected a bad node reference, and a stray
/// brace pair in prose is not this function's to police.
pub fn render(text: &str, ctx: &TemplateContext<'_>, surface: Surface) -> String {
    let mut out = String::with_capacity(text.len());
    for piece in pieces(text) {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Token(token) => match resolve(token, ctx) {
                Some(value) => out.push_str(&emit(&value, surface)),
                None => {
                    out.push_str("{{");
                    out.push_str(token);
                    out.push_str("}}");
                }
            },
        }
    }
    out
}

/// One stretch of a template: literal text, or what sits between a `{{` and
/// its own nearest `}}`.
enum Piece<'t> {
    Text(&'t str),
    Token(&'t str),
}

/// `text` split the one way every reader of a template splits it, so what
/// renders, what is checked and what is renamed agree on what a token is.
/// Each token is bounded by its own closing pair, so an unrecognised template
/// does not consume the variables after it; an unterminated `{{` is text.
fn pieces(text: &str) -> impl Iterator<Item = Piece<'_>> {
    let mut rest = text;
    let mut token_next: Option<&str> = None;
    std::iter::from_fn(move || {
        if let Some(token) = token_next.take() {
            return Some(Piece::Token(token));
        }
        if rest.is_empty() {
            return None;
        }
        let Some((start, end)) = rest
            .find("{{")
            .and_then(|start| rest[start + 2..].find("}}").map(|end| (start, end)))
        else {
            return Some(Piece::Text(std::mem::take(&mut rest)));
        };
        let before = &rest[..start];
        let token = &rest[start + 2..start + 2 + end];
        rest = &rest[start + 2 + end + 2..];
        if before.is_empty() {
            Some(Piece::Token(token))
        } else {
            token_next = Some(token);
            Some(Piece::Text(before))
        }
    })
}

/// `None` means "not a variable" and is left verbatim; `Some("")` means a
/// known variable with nothing behind it right now.
fn resolve(token: &str, ctx: &TemplateContext<'_>) -> Option<String> {
    match token {
        "run_dir" => Some(ctx.run_dir.display().to_string()),
        "output" => Some(
            ctx.output
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        ),
        "failure" => Some(ctx.failure.map(|f| f.to_string()).unwrap_or_default()),
        "attempts" => Some(
            ctx.attempts
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => {
            if let Some(name) = token.strip_prefix(INPUT_PREFIX) {
                // Static validation refused an undeclared name, so a miss here
                // is a value the run never had; left verbatim like any other.
                return ctx.inputs.get(name).cloned();
            }
            token
                .strip_prefix("node.")
                .and_then(|rest| rest.strip_suffix(".output"))
                .map(|id| {
                    ctx.node_outputs
                        .get(id)
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                })
        }
    }
}

const INPUT_PREFIX: &str = "input.";

/// Every input name `text` writes as `{{input.<name>}}`, read through the
/// same [`pieces`] [`render`] substitutes from.
pub(crate) fn input_refs(text: &str) -> impl Iterator<Item = &str> {
    pieces(text).filter_map(|piece| match piece {
        Piece::Token(token) => token.strip_prefix(INPUT_PREFIX),
        Piece::Text(_) => None,
    })
}

/// Point every `{{node.<from>.output}}` in `text` at `to`.
///
/// Here rather than in the editor that calls it: this file is what decides
/// what a template token means, and a rewriter that spelled the token itself
/// would be a second answer to that — the kind that stays right until one of
/// them changes.
///
/// Reads through the same `pieces` `render` does, so an unterminated `{{`
/// or a token that is not an output reference is copied through rather than
/// swallowed. Only exact matches move: a node called `design` is not `design2`.
pub fn rename_output_refs(text: &str, from: &NodeId, to: &NodeId) -> String {
    let mut out = String::with_capacity(text.len());
    for piece in pieces(text) {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Token(token) => {
                let renamed = token
                    .strip_prefix("node.")
                    .and_then(|rest| rest.strip_suffix(".output"))
                    .filter(|id| *id == from.as_str())
                    .map(|_| format!("node.{to}.output"));
                out.push_str("{{");
                out.push_str(renamed.as_deref().unwrap_or(token));
                out.push_str("}}");
            }
        }
    }
    out
}

/// What an included flow's `{{input.<name>}}` becomes in the flow that
/// includes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InputBinding {
    /// One token of the including flow — `input.x` or `node.x.output` —
    /// rendered, and quoted for a shell, when the node runs.
    Token(String),
    /// Text fixed in the including file, quoted now for a shell surface.
    Literal(String),
}

impl InputBinding {
    /// How an include's value for one input binds, or `None` for a value
    /// that mixes text with a token. Mixed values are refused: a shell
    /// surface would need the quoted text and the later-quoted token to
    /// join into one word, which is not shown to hold on every shell.
    pub(crate) fn of(value: &crate::input::InputValue) -> Option<InputBinding> {
        let text = match value {
            crate::input::InputValue::Text(text) => text,
            other => return Some(InputBinding::Literal(other.as_text())),
        };
        if !text.contains("{{") {
            return Some(InputBinding::Literal(text.clone()));
        }
        let mut parts = pieces(text);
        match (parts.next(), parts.next()) {
            (Some(Piece::Token(token)), None) if is_bindable(token) => {
                Some(InputBinding::Token(token.to_string()))
            }
            _ => None,
        }
    }
}

/// A token whose meaning does not change when it moves into another flow's
/// node: a run input, or an output by node id. `{{output}}` or
/// `{{failure}}` would silently start meaning the included node's own.
fn is_bindable(token: &str) -> bool {
    token.strip_prefix(INPUT_PREFIX).is_some()
        || token
            .strip_prefix("node.")
            .and_then(|rest| rest.strip_suffix(".output"))
            .is_some()
}

/// Replace each `{{input.<name>}}` in `text` that `bindings` names: a token
/// with its token, a literal with its text as `surface` takes it. Every
/// other piece is copied through, read by the same [`pieces`] as `render`.
pub(crate) fn rewrite_input_refs(
    text: &str,
    bindings: &std::collections::BTreeMap<String, InputBinding>,
    surface: Surface,
) -> String {
    let mut out = String::with_capacity(text.len());
    for piece in pieces(text) {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Token(token) => match token
                .strip_prefix(INPUT_PREFIX)
                .and_then(|name| bindings.get(name))
            {
                Some(InputBinding::Token(bound)) => {
                    out.push_str("{{");
                    out.push_str(bound);
                    out.push_str("}}");
                }
                Some(InputBinding::Literal(value)) => out.push_str(&emit(value, surface)),
                None => {
                    out.push_str("{{");
                    out.push_str(token);
                    out.push_str("}}");
                }
            },
        }
    }
    out
}

/// Point every `{{node.<id>.output}}` in `text` whose id is in `ids` at
/// that node's id inside the flow including it. One pass over `text`, so an
/// id that already contains the include's prefix is not prefixed twice.
pub(crate) fn prefix_output_refs(
    text: &str,
    ids: &std::collections::HashSet<NodeId>,
    include: &NodeId,
) -> String {
    let mut out = String::with_capacity(text.len());
    for piece in pieces(text) {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Token(token) => {
                let moved = token
                    .strip_prefix("node.")
                    .and_then(|rest| rest.strip_suffix(".output"))
                    .filter(|id| ids.contains(*id))
                    .map(|id| {
                        let member = crate::include::member_id(include, &NodeId::from(id));
                        format!("node.{member}.output")
                    });
                out.push_str("{{");
                out.push_str(moved.as_deref().unwrap_or(token));
                out.push_str("}}");
            }
        }
    }
    out
}

fn emit(value: &str, surface: Surface) -> String {
    match surface {
        Surface::Prompt => value.to_string(),
        // `try_quote` only fails on an interior NUL, which no path or
        // rendered failure can hold. The fallback is a quoted empty word,
        // not nothing: dropping the argument entirely would leave a command
        // like `grep -q x ` reading stdin, which hangs instead of failing.
        Surface::Shell => shlex::try_quote(value)
            .map(|q| q.into_owned())
            .unwrap_or_else(|_| "''".to_string()),
    }
}

#[cfg(test)]
mod tests;
