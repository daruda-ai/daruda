use super::*;
use crate::workspace::main_area::file_context::reference::{SelectedSource, SelectionSource};
use crate::workspace::main_area::file_view_pane::DiffSource;

fn reference() -> FileReference {
    let text = "before\nlet message = \"한글 it's $HOME\";\nafter";
    FileReference {
        path: "/tmp/a path/file.rs".into(),
        source: DiffSource::WorkingTree,
        selected: Some(SelectedSource {
            text: text.into(),
            range: text.find("let ").unwrap()..text.find("\nafter").unwrap(),
            source: SelectionSource::Raw { unsaved: false },
        }),
    }
}

#[test]
fn compact_unicode_references_expand_in_place_only_when_submitted() {
    let reference = reference();
    let token = FileContext::token(&reference, "a path/file.rs", false).unwrap();
    assert_eq!(token.label(), "file.rs:2");
    let prefix = "여기를 확인해줘 ";
    let content = InputContent::new(format!(
        "{prefix}{} 그리고 {} 끝",
        token.text(),
        token.text()
    ));
    let second = prefix.len() + token.text().len() + " 그리고 ".len();
    let content = content
        .with_token(
            prefix.len()..prefix.len() + token.text().len(),
            token.clone(),
        )
        .unwrap()
        .with_token(second..second + token.text().len(), token)
        .unwrap();
    assert!(!content.text().contains("$HOME"));
    let snapshot = reference.format(&reference.path.to_string_lossy(), None);
    assert_eq!(
        prompt_text(&content, None),
        format!("{prefix}{snapshot} 그리고 {snapshot} 끝")
    );
    let appended = append_text(&content, " /another/path");
    assert_eq!(appended.tokens(), content.tokens());
    assert!(prompt_text(&appended, None).ends_with(" /another/path"));
}

#[test]
fn terminal_submission_quotes_each_snapshot_for_its_current_shell() {
    let reference = reference();
    let token = FileContext::token(&reference, "file.rs", false).unwrap();
    let content = InputContent::new(token.text().clone())
        .with_token(0..token.text().len(), token)
        .unwrap();
    for shell in [Shell::Posix, Shell::PowerShell] {
        assert_eq!(
            prompt_text(&content, Some(shell)),
            reference.format(&reference.path.to_string_lossy(), Some(shell)),
        );
    }
}

#[test]
fn plain_text_and_unrecognized_tokens_do_not_acquire_hidden_context() {
    for content in [
        InputContent::new("file.rs:2"),
        InputContent::new("file.rs:2")
            .with_token(0..9, InlineToken::new("another-feature", "file.rs:2"))
            .unwrap(),
    ] {
        assert_eq!(prompt_text(&content, None), "file.rs:2");
    }
}

#[test]
fn references_retain_file_versions_and_diff_display_locations() {
    use crate::workspace::main_area::file_context::reference::SourceRow;
    use crate::workspace::main_area::file_view_pane::VisualRowKind;

    for source in [
        DiffSource::WorkingTree,
        DiffSource::Index,
        DiffSource::Range {
            from: "base".into(),
            to: "tip".into(),
            old_path: Some("before.rs".into()),
            status: 'R',
        },
    ] {
        let mut reference = reference();
        reference.source = source.clone();
        let token = FileContext::token(&reference, "file.rs", false).unwrap();
        let context = FileContext::from_token(&token).unwrap();
        assert_eq!(context.source, source);
        assert_eq!(context.location(), (FileViewMode::Raw, Some(2), false));
        reference.selected.as_mut().unwrap().source = SelectionSource::Diff(vec![
            SourceRow {
                kind: VisualRowKind::Added,
                old: String::new(),
                new: "60".into(),
            };
            3
        ]);
        let token = FileContext::token(&reference, "file.rs", true).unwrap();
        let context = FileContext::from_token(&token).unwrap();
        assert_eq!(context.source, source);
        assert_eq!(context.location(), (FileViewMode::Changes, Some(2), true));
    }
}
