use super::*;
use crate::ui::InlineToken;

#[test]
fn history_restores_tokens_and_the_unsent_draft_without_text_collisions() {
    let content = InputContent::new("review file.rs:2")
        .with_token(7..16, InlineToken::new("snapshot", "file.rs:2"))
        .unwrap();
    let mut history = InputHistory::default();
    history.push_content("review captured source", &content);
    let draft = InputContent::new("file.rs:2")
        .with_token(0..9, InlineToken::new("unsent-snapshot", "file.rs:2"))
        .unwrap();
    assert_eq!(history.prev(draft.clone()), Some(content.clone()));
    assert_eq!(history.forward(), Some(draft));
    // Identical visible text typed without a token remains plain on recall.
    history.push("review file.rs:2");
    assert!(history.prev("").unwrap().tokens().is_empty());
    assert_eq!(history.prev(""), Some(content));
}
