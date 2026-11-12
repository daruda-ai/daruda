//! Compact input references with immutable prompt content carried by the token.

use std::path::PathBuf;

use crate::ui::{InlineToken, InputContent};
use daruda_core::shell::quote::{Shell, quote_str};
use serde::{Deserialize, Serialize};

use super::super::file_view_pane::{DiffSource, FileViewMode};
use super::reference::{EXCERPT_LIMIT, FileReference, SelectionSource, inline_code};

const TOKEN_PREFIX: &str = "file-context:";

#[derive(Serialize, Deserialize)]
pub(super) struct FileContext {
    pub path: PathBuf,
    #[serde(default = "working_tree_source")]
    pub source: DiffSource,
    #[serde(default)]
    selection: Option<CapturedSelection>,
}

#[derive(Serialize, Deserialize)]
struct CapturedSelection {
    #[serde(default)]
    location: Option<SelectionLocation>,
    prompt: String,
}

#[derive(Serialize, Deserialize)]
enum SelectionLocation {
    Source { line: usize },
    Changes { row: usize, hide_unchanged: bool },
}

fn working_tree_source() -> DiffSource {
    DiffSource::WorkingTree
}

impl FileContext {
    /// Capture and validate the prompt once, before encoding its input token.
    pub(super) fn capture(reference: &FileReference, hide_unchanged: bool) -> Option<Self> {
        if reference
            .selected
            .as_ref()
            .is_some_and(|selected| selected.excerpt().len() > EXCERPT_LIMIT)
        {
            return None;
        }
        let context = Self {
            path: reference.path.clone(),
            source: reference.source.clone(),
            selection: reference
                .selected
                .as_ref()
                .map(|selected| CapturedSelection {
                    location: Some(match selected.source {
                        SelectionSource::Raw { .. } => SelectionLocation::Source {
                            line: selected.line_range().start + 1,
                        },
                        SelectionSource::Diff(_) => SelectionLocation::Changes {
                            row: selected.line_range().start + 1,
                            hide_unchanged,
                        },
                    }),
                    prompt: reference.format(&reference.path.to_string_lossy(), None),
                }),
        };
        if context
            .selection
            .as_ref()
            .is_some_and(|selection| selection.prompt.len() > EXCERPT_LIMIT)
        {
            return None;
        }
        Some(context)
    }

    pub(super) fn into_token(
        self,
        reference: &FileReference,
        display_path: &str,
    ) -> Result<InlineToken, serde_json::Error> {
        let label = reference.label(
            &reference
                .path
                .file_name()
                .unwrap_or(reference.path.as_os_str())
                .to_string_lossy(),
        );
        // Escaping control characters keeps legal filesystem names valid in
        // the field's single-line atomic token representation.
        let clean = |text: String| {
            text.chars()
                .flat_map(|c| {
                    if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
                        c.escape_default().collect::<Vec<_>>()
                    } else {
                        vec![c]
                    }
                })
                .collect::<String>()
        };
        let id = format!("{TOKEN_PREFIX}{}", serde_json::to_string(&self)?);
        Ok(InlineToken::new(id, clean(reference.label(display_path))).with_label(clean(label)))
    }

    pub(super) fn from_token(token: &InlineToken) -> Option<Self> {
        serde_json::from_str(token.id().strip_prefix(TOKEN_PREFIX)?).ok()
    }

    pub(super) fn location(&self) -> (FileViewMode, Option<usize>, bool) {
        match self
            .selection
            .as_ref()
            .and_then(|selection| selection.location.as_ref())
        {
            Some(SelectionLocation::Changes {
                row,
                hide_unchanged,
            }) => (FileViewMode::Changes, Some(*row), *hide_unchanged),
            Some(SelectionLocation::Source { line }) => (FileViewMode::Raw, Some(*line), false),
            None => (FileViewMode::Raw, None, false),
        }
    }

    fn prompt(&self, shell: Option<Shell>) -> String {
        let path = self.path.to_string_lossy();
        match (&self.selection, shell) {
            (None, Some(shell)) => quote_str(&path, shell),
            (None, None) => inline_code(&path),
            (Some(selection), Some(shell)) => quote_str(&selection.prompt, shell),
            (Some(selection), None) => selection.prompt.clone(),
        }
    }
}

/// Only explicit submission expands references; editing, drafts and history
/// continue to use the coherent text-and-token snapshot.
pub(in crate::workspace) fn prompt_text(content: &InputContent, shell: Option<Shell>) -> String {
    let mut result = String::new();
    let mut cursor = 0;
    for span in content.tokens() {
        let range = span.range();
        result.push_str(&content.text()[cursor..range.start]);
        if let Some(context) = FileContext::from_token(span.token()) {
            result.push_str(&context.prompt(shell));
        } else {
            result.push_str(&content.text()[range.clone()]);
        }
        cursor = range.end;
    }
    result.push_str(&content.text()[cursor..]);
    result
}

/// Appending a fallback attachment path must retain existing reference spans.
pub(in crate::workspace) fn append_text(content: &InputContent, text: &str) -> InputContent {
    let mut appended = InputContent::new(format!("{}{text}", content.text()));
    for span in content.tokens() {
        appended = appended
            .with_token(span.range(), span.token().clone())
            .expect("appending text preserves validated token ranges");
    }
    appended
}

#[cfg(test)]
mod tests;
