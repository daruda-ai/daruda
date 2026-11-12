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
    let token = FileContext::capture(&reference, false)
        .unwrap()
        .into_token(&reference, "a path/file.rs")
        .unwrap();
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
    let token = FileContext::capture(&reference, false)
        .unwrap()
        .into_token(&reference, "file.rs")
        .unwrap();
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
        let token = FileContext::capture(&reference, false)
            .unwrap()
            .into_token(&reference, "file.rs")
            .unwrap();
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
        let token = FileContext::capture(&reference, true)
            .unwrap()
            .into_token(&reference, "file.rs")
            .unwrap();
        let context = FileContext::from_token(&token).unwrap();
        assert_eq!(context.source, source);
        assert_eq!(context.location(), (FileViewMode::Changes, Some(2), true));
    }
}

#[test]
fn capture_limits_the_complete_prompt_including_its_reference_and_fences() {
    let mut reference = reference();
    reference.selected = Some(SelectedSource {
        text: "x".into(),
        range: 0..1,
        source: SelectionSource::Raw { unsaved: false },
    });
    let overhead = FileContext::capture(&reference, false)
        .unwrap()
        .selection
        .unwrap()
        .prompt
        .len()
        - 1;
    for (case, length, accepted) in [
        ("exact prompt limit", EXCERPT_LIMIT - overhead, true),
        (
            "reference and fences exceed limit",
            EXCERPT_LIMIT - overhead + 1,
            false,
        ),
        ("source alone exceeds limit", EXCERPT_LIMIT + 1, false),
    ] {
        let selected = reference.selected.as_mut().unwrap();
        selected.text = "x".repeat(length);
        selected.range = 0..length;
        let context = FileContext::capture(&reference, false);
        assert_eq!(context.is_some(), accepted, "{case}");
        if let Some(context) = context {
            let token = context.into_token(&reference, "file.rs").unwrap();
            assert_eq!(
                FileContext::from_token(&token)
                    .unwrap()
                    .selection
                    .unwrap()
                    .prompt
                    .len(),
                EXCERPT_LIMIT,
                "{case}"
            );
        }
    }
}
