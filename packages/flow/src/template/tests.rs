use super::*;
use crate::runner::NodeFailure;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

fn ctx_with(run_dir: &str) -> (PathBuf, HashMap<crate::NodeId, PathBuf>) {
    let run_dir = PathBuf::from(run_dir);
    let mut outputs = HashMap::new();
    outputs.insert("review".into(), run_dir.join("review.md"));
    outputs.insert("implement".into(), run_dir.join("implement.md"));
    (run_dir, outputs)
}

#[test]
fn a_prompt_gets_absolute_paths_and_no_quoting() {
    let (run_dir, outputs) = ctx_with("/repo/.daruda/flow-runs/01J");
    let own = run_dir.join("design.md");
    let ctx = TemplateContext {
        inputs: &Default::default(),
        run_dir: &run_dir,
        output: Some(&own),
        node_outputs: &outputs,
        failure: None,
        attempts: &[],
    };
    let text = render(
        "write {{output}}, having read {{node.implement.output}} under {{run_dir}}",
        &ctx,
        Surface::Prompt,
    );
    assert_eq!(
        text,
        format!(
            "write {}, having read {} under {}",
            own.display(),
            run_dir.join("implement.md").display(),
            run_dir.display()
        )
    );
}

/// An input's value is prose in a prompt and one quoted argument in a
/// command, so a value cannot add flags or chain a second command.
#[test]
fn an_input_is_quoted_on_the_shell_and_plain_in_a_prompt() {
    let (run_dir, outputs) = ctx_with("/repo/.daruda/flow-runs/01J");
    let inputs = BTreeMap::from([
        ("pr".to_string(), "1234".to_string()),
        ("title".to_string(), "fix it; rm -rf /".to_string()),
    ]);
    let ctx = TemplateContext {
        inputs: &inputs,
        run_dir: &run_dir,
        output: None,
        node_outputs: &outputs,
        failure: None,
        attempts: &[],
    };
    assert_eq!(
        render(
            "review {{input.pr}}: {{input.title}}",
            &ctx,
            Surface::Prompt
        ),
        "review 1234: fix it; rm -rf /"
    );
    let line = render(
        "gh pr view {{input.pr}} --title {{input.title}}",
        &ctx,
        Surface::Shell,
    );
    assert_eq!(
        shlex::split(&line).unwrap(),
        ["gh", "pr", "view", "1234", "--title", "fix it; rm -rf /"]
    );
    assert_eq!(
        render("{{input.missing}} {{.Names}}", &ctx, Surface::Shell),
        "{{input.missing}} {{.Names}}",
        "an unknown token is left verbatim"
    );
}

#[test]
fn input_refs_reads_tokens_the_way_render_does() {
    let refs: Vec<_> = input_refs("{{input.a}} {{node.x.output}} {{input.b}} {{input.c").collect();
    assert_eq!(refs, ["a", "b"]);
}

/// The reason `Surface` exists: a run directory with a space in it must
/// still reach `grep` as one argument.
#[test]
fn a_command_quotes_a_path_with_a_space_into_one_argument() {
    let (run_dir, outputs) = ctx_with("/Users/me/my repo/.daruda/flow-runs/01J");
    let ctx = TemplateContext {
        inputs: &Default::default(),
        run_dir: &run_dir,
        output: None,
        node_outputs: &outputs,
        failure: None,
        attempts: &[],
    };
    let text = render(
        "grep -q '^VERDICT: PASS' {{node.review.output}}",
        &ctx,
        Surface::Shell,
    );
    assert_eq!(
        shlex::split(&text).unwrap(),
        [
            "grep",
            "-q",
            "^VERDICT: PASS",
            &run_dir.join("review.md").to_string_lossy()
        ]
    );
}

/// Each match is bounded by its own nearest `}}`, so a template this
/// function does not recognise leaves the variables after it alone.
#[test]
fn an_unrelated_template_does_not_swallow_a_later_variable() {
    let (run_dir, outputs) = ctx_with("/repo/run");
    let ctx = TemplateContext {
        inputs: &Default::default(),
        run_dir: &run_dir,
        output: None,
        node_outputs: &outputs,
        failure: None,
        attempts: &[],
    };
    let text = render(
        "{{unknown}} then {{node.review.output}}",
        &ctx,
        Surface::Prompt,
    );
    assert_eq!(
        text,
        format!(
            "{{{{unknown}}}} then {}",
            run_dir.join("review.md").display()
        )
    );
}

#[test]
fn failure_and_attempts_render_for_a_repair_prompt() {
    let (run_dir, outputs) = ctx_with("/repo/run");
    let failure = NodeFailure::Timeout {
        elapsed: Duration::from_secs(600),
    };
    let attempts = vec![
        PathBuf::from("/repo/run/logs/test.attempt-1.evidence-1.log"),
        PathBuf::from("/repo/run/logs/review.attempt-1.evidence-1.md"),
    ];
    let ctx = TemplateContext {
        inputs: &Default::default(),
        run_dir: &run_dir,
        output: None,
        node_outputs: &outputs,
        failure: Some(&failure),
        attempts: &attempts,
    };
    let text = render("{{failure}}\n{{attempts}}", &ctx, Surface::Prompt);
    assert!(text.starts_with("timed out after 600 seconds"));
    assert!(text.contains("/repo/run/logs/test.attempt-1.evidence-1.log\n/repo/run/logs/review.attempt-1.evidence-1.md"));
}

/// A variable with nothing behind it renders empty rather than leaking
/// `{{failure}}` into a prompt an agent has to read.
#[test]
fn an_unavailable_variable_renders_empty() {
    let (run_dir, outputs) = ctx_with("/repo/run");
    let ctx = TemplateContext {
        inputs: &Default::default(),
        run_dir: &run_dir,
        output: None,
        node_outputs: &outputs,
        failure: None,
        attempts: &[],
    };
    assert_eq!(
        render("[{{failure}}][{{output}}]", &ctx, Surface::Prompt),
        "[][]"
    );
}

/// On the shell an empty substitution has to stay a word. Dropping it
/// would turn `grep -q x {{output}}` into `grep -q x`, which reads
/// stdin and hangs rather than failing.
#[test]
fn an_empty_shell_substitution_is_still_one_argument() {
    let (run_dir, outputs) = ctx_with("/repo/run");
    let ctx = TemplateContext {
        inputs: &Default::default(),
        run_dir: &run_dir,
        output: None,
        node_outputs: &outputs,
        failure: None,
        attempts: &[],
    };
    assert_eq!(
        render("grep -q x {{output}}", &ctx, Surface::Shell),
        "grep -q x ''"
    );
}

/// A rename has to reach the prompts, or it produces a file whose text
/// names a node that is not there — and the person who pressed rename has
/// to go find them.
#[test]
fn a_rename_moves_only_the_reference_it_is_about() {
    let moved = rename_output_refs(
        "read {{node.design.output}} and {{node.design2.output}}, write {{output}}",
        &NodeId::from("design"),
        &NodeId::from("blueprint"),
    );
    assert_eq!(
        moved, "read {{node.blueprint.output}} and {{node.design2.output}}, write {{output}}",
        "an exact match moves; a longer name that starts the same does not"
    );
}

/// Scanned the way `render` scans, so text it does not understand comes
/// out unchanged rather than being swallowed.
#[test]
fn a_rename_leaves_everything_it_does_not_understand_alone() {
    let from = NodeId::from("design");
    let to = NodeId::from("blueprint");
    for text in [
        "no templates here",
        "{{unknown}} and {{node.design.output}}",
        "an unterminated {{node.design.output",
        "{{node.design.other}}",
    ] {
        let moved = rename_output_refs(text, &from, &to);
        assert_eq!(
            moved.contains("{{node.design.output}}"),
            text.contains("{{node.design.output}}") && !text.contains("{{unknown}}"),
            "for {text:?} -> {moved:?}"
        );
        assert!(!moved.is_empty(), "nothing is swallowed: {text:?}");
    }
    assert_eq!(
        rename_output_refs("an unterminated {{node.design.output", &from, &to),
        "an unterminated {{node.design.output"
    );
}

/// How one value an include gives binds — or `None`, refused.
#[test]
fn an_include_value_binds_as_a_token_or_a_literal() {
    use crate::input::InputValue;
    let text = |t: &str| InputValue::Text(t.to_string());
    let token = |t: &str| Some(InputBinding::Token(t.to_string()));
    let literal = |t: &str| Some(InputBinding::Literal(t.to_string()));
    let rows: Vec<(&str, InputValue, Option<InputBinding>)> = vec![
        ("plain text", text("security"), literal("security")),
        (
            "a number keeps its spelling",
            InputValue::Number(2.into()),
            literal("2"),
        ),
        ("a boolean", InputValue::Bool(true), literal("true")),
        (
            "an input of the including flow",
            text("{{input.pr}}"),
            token("input.pr"),
        ),
        (
            "an output by node id",
            text("{{node.build.output}}"),
            token("node.build.output"),
        ),
        (
            "text around a token is refused",
            text("PR {{input.pr}}"),
            None,
        ),
        (
            "two tokens are refused",
            text("{{input.a}}{{input.b}}"),
            None,
        ),
        (
            "a token meaning the node's own output is refused",
            text("{{output}}"),
            None,
        ),
        ("a run variable is refused", text("{{run_dir}}"), None),
        ("an unterminated brace is refused", text("{{input.pr"), None),
    ];
    for (label, value, want) in rows {
        assert_eq!(InputBinding::of(&value), want, "{label}");
    }
}

#[test]
fn input_refs_are_rewritten_by_their_binding() {
    let bindings: std::collections::BTreeMap<String, InputBinding> = [
        (
            "pr".to_string(),
            InputBinding::Token("input.number".to_string()),
        ),
        (
            "notes".to_string(),
            InputBinding::Token("node.build.output".to_string()),
        ),
        (
            "focus".to_string(),
            InputBinding::Literal("it's broken".to_string()),
        ),
    ]
    .into();
    let rows: Vec<(&str, &str, Surface, &str)> = vec![
        (
            "a token stays a token",
            "gh pr view {{input.pr}}",
            Surface::Shell,
            "gh pr view {{input.number}}",
        ),
        (
            "an output token",
            "read {{input.notes}}",
            Surface::Prompt,
            "read {{node.build.output}}",
        ),
        (
            "a literal is prose in a prompt",
            "focus: {{input.focus}}",
            Surface::Prompt,
            "focus: it's broken",
        ),
        (
            "a literal is one quoted word in a shell",
            "grep {{input.focus}} x",
            Surface::Shell,
            "grep \"it's broken\" x",
        ),
        (
            "an unbound input is left",
            "{{input.other}}",
            Surface::Shell,
            "{{input.other}}",
        ),
        (
            "other tokens are left",
            "{{output}} {{node.a.output}}",
            Surface::Shell,
            "{{output}} {{node.a.output}}",
        ),
        (
            "an unterminated brace is text",
            "{{input.focus",
            Surface::Prompt,
            "{{input.focus",
        ),
    ];
    for (label, text, surface, want) in rows {
        assert_eq!(
            rewrite_input_refs(text, &bindings, surface),
            want,
            "{label}"
        );
    }
}
