use super::*;

#[test]
fn parses_bare_commands() {
    assert_eq!(parse("/list"), Ok(ControlCommand::List));
    assert_eq!(parse("/brief"), Ok(ControlCommand::Brief));
    assert_eq!(parse("/stop"), Ok(ControlCommand::Stop { target: None }));
}

#[test]
fn parses_use_select_and_clear() {
    assert_eq!(
        parse("/use 2"),
        Ok(ControlCommand::Use(UseTarget::Select(Ordinal(2))))
    );
    assert_eq!(parse("/use -"), Ok(ControlCommand::Use(UseTarget::Clear)));
}

#[test]
fn say_keeps_the_rest_of_the_line_less_one_layer_of_quotes() {
    for input in [r#"/say 3 add tests too"#, r#"/say 3 "add tests too""#] {
        assert_eq!(
            parse(input),
            Ok(ControlCommand::Say {
                target: Ordinal(3),
                text: "add tests too".into(),
            }),
            "{input}"
        );
    }
}

#[test]
fn flow_without_name_lists() {
    assert_eq!(parse("/flow"), Ok(ControlCommand::Flow(FlowCommand::List)));
    assert_eq!(
        parse("/flow deploy"),
        Ok(ControlCommand::Flow(FlowCommand::Run {
            name: "deploy".into()
        }))
    );
}

/// The rest of the line is a prompt, not a nested command surface. One
/// layer of wrapping quotes is stripped; a prompt that merely opens and closes
/// with a quote is not a quoted prompt — stripping there would hand on an
/// unbalanced string.
#[test]
fn daruda_takes_the_rest_of_the_line_verbatim() {
    for (input, text) in [
        ("/daruda make me a pane", "make me a pane"),
        ("/daruda /list", "/list"),
        (r#"/daruda "make me a pane""#, "make me a pane"),
        (r#"/daruda "foo" is not "bar""#, r#""foo" is not "bar""#),
    ] {
        assert_eq!(
            parse(input),
            Ok(ControlCommand::Ask { text: text.into() }),
            "{input}"
        );
    }
}

#[test]
fn missing_argument_names_the_command() {
    for (input, command) in [("/daruda", "daruda"), ("/use", "use"), ("/say 2", "say")] {
        assert_eq!(
            parse(input),
            Err(ParseError::MissingArgument { command }),
            "{input}"
        );
    }
}

/// `daruda` is in the suggestion pool like every other name, so a typo
/// points at it instead of at nothing; a distant one suggests nothing.
#[test]
fn unknown_command_suggests_nearest() {
    for (input, suggestion) in [
        ("/darudo", Some("daruda")),
        ("/lst", Some("list")),
        ("/deploy", None),
    ] {
        assert_eq!(
            parse(input),
            Err(ParseError::Unknown {
                input: input[1..].into(),
                suggestion,
            })
        );
    }
}

#[test]
fn non_command_text_is_not_a_command() {
    assert_eq!(parse("hello"), Err(ParseError::NotACommand));
    assert_eq!(parse(""), Err(ParseError::NotACommand));
}

/// Telegram's own command menu sends `/list@thebot` in a group chat, and a
/// phone keyboard capitalises the first letter of a line. Neither should
/// read as an unknown command.
#[test]
fn a_command_survives_a_bot_suffix_and_a_capital() {
    assert_eq!(parse("/list@daruda_bot"), Ok(ControlCommand::List));
    assert_eq!(parse("/List"), Ok(ControlCommand::List));
    assert_eq!(
        parse("/Say@daruda_bot 2 go on"),
        Ok(ControlCommand::Say {
            target: Ordinal(2),
            text: "go on".into(),
        })
    );
}

#[test]
fn non_numeric_ordinal_is_rejected() {
    assert_eq!(
        parse("/use abc"),
        Err(ParseError::BadOrdinal {
            input: "abc".into()
        })
    );
}

#[test]
fn task_lists_bare_and_names_a_row_for_each_action() {
    assert_eq!(parse("/task"), Ok(ControlCommand::Task(TaskCommand::List)));
    assert_eq!(
        parse("/task start 2"),
        Ok(ControlCommand::Task(TaskCommand::Start(Ordinal(2))))
    );
    assert_eq!(
        parse("/Task Stop 1"),
        Ok(ControlCommand::Task(TaskCommand::Stop(Ordinal(1))))
    );
    assert_eq!(
        parse("/task open 3"),
        Ok(ControlCommand::Task(TaskCommand::Open(Ordinal(3))))
    );
}

#[test]
fn task_without_a_known_action_or_a_number_shows_its_usage() {
    for input in ["/task foo", "/task start", "/task delete 1"] {
        assert_eq!(
            parse(input),
            Err(ParseError::MissingArgument { command: "task" }),
            "{input}"
        );
    }
    assert_eq!(
        parse("/task start x"),
        Err(ParseError::BadOrdinal { input: "x".into() })
    );
}
