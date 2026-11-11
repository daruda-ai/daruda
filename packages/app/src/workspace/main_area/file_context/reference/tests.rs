use super::*;

#[test]
fn references_quote_paths_for_the_destination_and_bound_excerpt_fences() {
    let reference = FileReference {
        path: "unused".into(),
        source: DiffSource::WorkingTree,
        selected: None,
    };
    for (label, path, shell, expected) in [
        (
            "posix metacharacters",
            "/tmp/a $x`b.rs",
            daruda_core::shell::quote::Shell::Posix,
            "'/tmp/a $x`b.rs'",
        ),
        (
            "Windows path",
            r"C:\My Project\auth.rs",
            daruda_core::shell::quote::Shell::PowerShell,
            r"'C:\My Project\auth.rs'",
        ),
    ] {
        assert_eq!(reference.format(path, Some(shell)), expected, "{label}");
    }
    let reference = FileReference {
        selected: Some(SelectedSource {
            text: "   ```".into(),
            range: 0..6,
            source: SelectionSource::Raw { unsaved: true },
        }),
        ..reference
    };
    let formatted = reference.format("file`name.md", None);
    assert!(formatted.contains("``file`name.md:1``"));
    assert!(formatted.contains("````\n   ```\n````"));
}

#[test]
fn saved_selections_include_the_captured_text_for_chat_and_terminal_inputs() {
    let text = "before\nlet message = \"it's $HOME\";\nafter";
    let reference = FileReference {
        path: "file.rs".into(),
        source: DiffSource::WorkingTree,
        selected: Some(SelectedSource {
            text: text.into(),
            range: text.find("let ").unwrap()..text.find("\nafter").unwrap(),
            source: SelectionSource::Raw { unsaved: false },
        }),
    };
    let label = crate::surface::strings::file_viewer::input_selection_snapshot();
    for (case, shell, expected) in [
        (
            "chat",
            None,
            format!("`file.rs:2`\n{label}\n```\nlet message = \"it's $HOME\";\n```\n"),
        ),
        (
            "POSIX shell",
            Some(daruda_core::shell::quote::Shell::Posix),
            format!("'`file.rs:2`\n{label}\n```\nlet message = \"it'\\''s $HOME\";\n```\n'"),
        ),
        (
            "PowerShell",
            Some(daruda_core::shell::quote::Shell::PowerShell),
            format!("'`file.rs:2`\n{label}\n```\nlet message = \"it''s $HOME\";\n```\n'"),
        ),
    ] {
        assert_eq!(reference.format("file.rs", shell), expected, "{case}");
    }
}

#[test]
fn historical_raw_selections_include_the_version_and_selected_contents() {
    for source in [
        DiffSource::Index,
        DiffSource::Range {
            from: "base".into(),
            to: "tip".into(),
            old_path: None,
            status: 'M',
        },
    ] {
        let reference = FileReference {
            path: "file.rs".into(),
            source,
            selected: Some(SelectedSource {
                text: "old contents".into(),
                range: 0..12,
                source: SelectionSource::Raw { unsaved: false },
            }),
        };
        let formatted = reference.format("file.rs", None);
        assert!(formatted.contains("old contents"));
        assert!(formatted.contains(if reference.source.is_index() {
            "index"
        } else {
            "tip"
        }));
    }
}

#[test]
fn selection_lines_use_included_bytes_and_real_source_rows() {
    for (label, text, range, expected) in [
        ("newline boundary", "one\ntwo\nthree", 0..4, 0..1),
        ("multiple lines", "one\ntwo\nthree", 1..6, 0..2),
        ("unicode end", "한글\n다음", 0..6, 0..1),
        ("second line", "one\ntwo", 4..7, 1..2),
    ] {
        let selected = SelectedSource {
            text: text.into(),
            range,
            source: SelectionSource::Raw { unsaved: false },
        };
        assert_eq!(selected.line_range(), expected, "{label}");
    }
    let reference = FileReference {
        path: "file.rs".into(),
        source: DiffSource::Index,
        selected: Some(SelectedSource {
            text: "@@ header\nold\nnew".into(),
            range: 10..17,
            source: SelectionSource::Diff(vec![
                SourceRow {
                    kind: VisualRowKind::HunkHeader,
                    old: String::new(),
                    new: String::new(),
                },
                SourceRow {
                    kind: VisualRowKind::Removed,
                    old: "42".into(),
                    new: String::new(),
                },
                SourceRow {
                    kind: VisualRowKind::Added,
                    old: String::new(),
                    new: "60".into(),
                },
            ]),
        }),
    };
    let formatted = reference.format("file.rs", None);
    assert!(formatted.contains("HEAD -> index"));
    assert!(formatted.find("42").unwrap() < formatted.find("60").unwrap());
    assert!(formatted.contains("-old\n+new"));
    assert_eq!(reference.label("file.rs"), "file.rs (−42,+60)");
}
