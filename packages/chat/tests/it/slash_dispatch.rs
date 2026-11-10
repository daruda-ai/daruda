use daruda_chat::slash_dispatch::{LocalSlashCommand, SlashDispatch, classify_slash};

#[test]
fn text_without_a_local_command_forwards() {
    for input in ["hello", "/foo", "/"] {
        assert_eq!(classify_slash(input), SlashDispatch::Forward, "{input:?}");
    }
}

#[test]
fn clear_is_local() {
    for input in ["/clear", "/clear now", "  /clear"] {
        assert_eq!(
            classify_slash(input),
            SlashDispatch::Local(LocalSlashCommand::Clear),
            "{input:?}"
        );
    }
}
