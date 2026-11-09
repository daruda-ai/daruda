use daruda_chat::slash_dispatch::{LocalSlashCommand, SlashDispatch, classify_slash};

#[test]
fn plain_text_forwards() {
    assert_eq!(classify_slash("hello"), SlashDispatch::Forward);
}

#[test]
fn clear_is_local() {
    assert_eq!(
        classify_slash("/clear"),
        SlashDispatch::Local(LocalSlashCommand::Clear)
    );
}

#[test]
fn clear_with_trailing_args_is_local() {
    assert_eq!(
        classify_slash("/clear now"),
        SlashDispatch::Local(LocalSlashCommand::Clear)
    );
}

#[test]
fn clear_with_leading_whitespace_is_local() {
    assert_eq!(
        classify_slash("  /clear"),
        SlashDispatch::Local(LocalSlashCommand::Clear)
    );
}

#[test]
fn unrecognized_slash_command_forwards() {
    assert_eq!(classify_slash("/foo"), SlashDispatch::Forward);
}

#[test]
fn bare_slash_forwards() {
    assert_eq!(classify_slash("/"), SlashDispatch::Forward);
}
