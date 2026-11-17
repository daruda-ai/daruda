use super::*;

fn comment(start: u32, end: u32, suggestion: Option<&str>) -> LineCommentDraft {
    LineCommentDraft {
        id: 1,
        path: "a.rs".into(),
        side: DraftSide::Right,
        start,
        end,
        revision: "h".into(),
        body: "Simpler: ".into(),
        suggestion: suggestion.map(str::to_owned),
    }
}

#[test]
fn a_pending_comment_is_sent_on_its_last_line_with_any_suggestion_fenced() {
    let one = thread_of(&comment(4, 4, None));
    assert_eq!(
        (one.line, one.start_line, one.side),
        (4, None, DiffSide::Right)
    );
    assert_eq!(one.body, "Simpler: ");
    let range = thread_of(&comment(2, 4, Some("let x = 1;")));
    assert_eq!((range.line, range.start_line), (4, Some(2)));
    assert_eq!(range.body, "Simpler:\n\n```suggestion\nlet x = 1;\n```");
}
