use super::*;
use crate::model::Prompt;
use std::collections::HashMap;

/// Flows by name, each in its own directory.
struct Flows(HashMap<String, IncludedFlow>);

impl Flows {
    fn new(flows: &[(&str, &str)]) -> Self {
        Self(
            flows
                .iter()
                .map(|(name, text)| {
                    (
                        name.to_string(),
                        IncludedFlow {
                            text: text.to_string(),
                            dir: PathBuf::from(format!("/flows/{name}")),
                        },
                    )
                })
                .collect(),
        )
    }

    fn in_dir(name: &str, text: &str, dir: &Path) -> Self {
        Self(HashMap::from([(
            name.to_string(),
            IncludedFlow {
                text: text.to_string(),
                dir: dir.to_path_buf(),
            },
        )]))
    }
}

impl IncludeSource for Flows {
    fn find(&self, flow: &str) -> Option<IncludedFlow> {
        self.0.get(flow).cloned()
    }
}

const REVIEW: &str = "\
version: 1
defaults:
  agent: { id: claude, mode: default }
inputs:
  pr:
    required: true
  focus:
    default: everything
nodes:
  - id: fetch
    kind: command
    run: gh pr diff {{input.pr}} > diff.txt
  - id: read
    kind: agent
    deps: [fetch]
    output: notes.md
    prompt: \"Review for {{input.focus}}; see {{node.fetch.output}}\"
  - id: check
    kind: command
    deps: [read]
    run: test -s {{node.read.output}}
";

const OUTER: &str = "\
version: 1
defaults:
  agent: { id: claude, mode: default }
inputs:
  number:
    required: true
nodes:
  - id: build
    kind: command
    run: ./build.sh
  - id: review
    kind: include
    flow: review
    deps: [build]
    inputs:
      pr: \"{{input.number}}\"
  - id: ship
    kind: command
    deps: [review]
    run: ./ship.sh
";

fn load(text: &str, flows: &Flows) -> crate::LoadedFlow {
    match crate::load_with(text, None, flows) {
        Ok(loaded) => loaded,
        Err(e) => panic!("expected the flow to load: {e:?}"),
    }
}

fn refused(text: &str, flows: &Flows) -> Vec<ValidationKind> {
    match crate::load_with(text, None, flows) {
        Ok(_) => panic!("expected a refusal"),
        Err(crate::FlowError::Validate(issues)) => issues.into_iter().map(|i| i.kind).collect(),
        Err(e) => panic!("expected validation issues, got {e:?}"),
    }
}

fn node<'a>(loaded: &'a crate::LoadedFlow, id: &str) -> &'a Node {
    loaded
        .flow()
        .nodes
        .iter()
        .find(|n| n.id == id)
        .unwrap_or_else(|| panic!("no node {id}"))
}

fn ids(nodes: &[NodeId]) -> Vec<&str> {
    nodes.iter().map(NodeId::as_str).collect()
}

#[test]
fn an_include_is_spliced_in_where_it_stands() {
    let loaded = load(OUTER, &Flows::new(&[("review", REVIEW)]));
    let order: Vec<_> = loaded.flow().nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(
        order,
        [
            "build",
            "review--fetch",
            "review--read",
            "review--check",
            "ship"
        ]
    );
    assert_eq!(loaded.includes().len(), 1);
    let site = &loaded.includes()[0];
    assert_eq!(site.id, "review");
    assert_eq!(site.flow, "review");
    assert_eq!(
        ids(&site.members),
        ["review--fetch", "review--read", "review--check"]
    );
    assert_eq!(ids(&site.sinks), ["review--check"]);
}

/// Each row is one line of the expansion contract.
#[test]
fn the_expansion_rewrites_every_name_and_reference() {
    let loaded = load(OUTER, &Flows::new(&[("review", REVIEW)]));
    let deps = |id: &str| ids(&node(&loaded, id).deps).join(",");
    let text = |id: &str| {
        node(&loaded, id)
            .templates()
            .into_iter()
            .map(|(_, t)| t.to_string())
            .collect::<Vec<_>>()
            .join(" | ")
    };
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a source takes the include's deps",
            deps("review--fetch"),
            "build",
        ),
        (
            "an inner dep is prefixed",
            deps("review--read"),
            "review--fetch",
        ),
        (
            "a dependent of the include waits for its sinks",
            deps("ship"),
            "review--check",
        ),
        (
            "a token binding stays a token, rendered and quoted at run time",
            text("review--fetch"),
            "gh pr diff {{input.number}} > diff.txt",
        ),
        (
            "a default fills an input the include leaves out; refs move",
            text("review--read"),
            "Review for everything; see {{node.review--fetch.output}}",
        ),
        (
            "a command's output ref moves",
            text("review--check"),
            "test -s {{node.review--read.output}}",
        ),
    ];
    for (label, got, want) in rows {
        assert_eq!(got, want, "{label}");
    }
    let body = node(&loaded, "review--read").kind.agent_body().unwrap();
    assert_eq!(
        body.output,
        Path::new("review/notes.md"),
        "outputs live under the include"
    );
}

#[test]
fn a_literal_is_quoted_for_a_shell_and_plain_in_a_prompt() {
    let outer = OUTER.replace(
        "      pr: \"{{input.number}}\"\n",
        "      pr: \"12 3\"\n      focus: it's\n",
    );
    let loaded = load(&outer, &Flows::new(&[("review", REVIEW)]));
    let run = node(&loaded, "review--fetch").templates()[0].1.to_string();
    assert_eq!(run, "gh pr diff '12 3' > diff.txt");
    let prompt = node(&loaded, "review--read").templates()[0].1.to_string();
    assert!(prompt.starts_with("Review for it's;"), "{prompt}");
}

/// What a resume would read: the expansion as `run.yaml` records it, loaded
/// with nowhere to find flows, is the flow that was started.
fn assert_round_trips(loaded: &crate::LoadedFlow, label: &str) {
    let file = crate::resolve::to_flow_file(loaded.flow(), Path::new("/nowhere"));
    let text = yaml_serde::to_string(&file).unwrap();
    let again = crate::load(&text, None)
        .unwrap_or_else(|e| panic!("{label}: run.yaml does not load: {e:?}\n{text}"));
    assert_eq!(again.flow().nodes, loaded.flow().nodes, "{label}: nodes");
    assert_eq!(
        again.flow().default_agent,
        loaded.flow().default_agent,
        "{label}: default agent"
    );
}

/// An expanded flow written to `run.yaml` and read back the way a resume
/// reads it is the same flow: the record holds everything the run needs.
#[test]
fn an_expanded_flow_survives_its_run_yaml() {
    let loaded = load(OUTER, &Flows::new(&[("review", REVIEW)]));
    let file = crate::resolve::to_flow_file(loaded.flow(), Path::new("/nowhere"));
    let text = yaml_serde::to_string(&file).unwrap();
    let again = crate::load(&text, None).expect("run.yaml loads without a source of flows");
    assert_eq!(again.flow().nodes, loaded.flow().nodes);
    assert_eq!(again.flow().default_agent, loaded.flow().default_agent);
    assert!(again.includes().is_empty(), "the record holds no include");
}

#[test]
fn a_prompt_file_is_read_from_the_included_flows_folder() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ask.md"), "focus on {{input.focus}}").unwrap();
    let review = REVIEW.replace(
        "    prompt: \"Review for {{input.focus}}; see {{node.fetch.output}}\"\n",
        "    prompt_file: ask.md\n",
    );
    let loaded = load(OUTER, &Flows::in_dir("review", &review, dir.path()));
    let body = node(&loaded, "review--read").kind.agent_body().unwrap();
    assert_eq!(
        body.prompt,
        Prompt::Inline("focus on everything".to_string())
    );
}

/// Each row: what the include, or the flow it names, gets wrong.
#[test]
fn an_include_that_cannot_be_spliced_is_refused() {
    let flow = |name: &str| name.to_string();
    let mismatch = |name: &str| ValidationKind::IncludeInputMismatch {
        flow: flow("review"),
        name: name.to_string(),
    };
    let with_inputs = |inputs: &str| {
        let block = if inputs.is_empty() {
            String::new()
        } else {
            format!("    inputs:\n{inputs}")
        };
        OUTER.replace("    inputs:\n      pr: \"{{input.number}}\"\n", &block)
    };
    let rows: Vec<(&str, String, Flows, Vec<ValidationKind>)> = vec![
        (
            "no flow by that name",
            OUTER.replace("    flow: review\n", "    flow: missing\n"),
            Flows::new(&[("review", REVIEW)]),
            vec![ValidationKind::UnknownInclude {
                flow: flow("missing"),
            }],
        ),
        (
            "a required input left out",
            with_inputs(""),
            Flows::new(&[("review", REVIEW)]),
            vec![mismatch("pr")],
        ),
        (
            "an input the flow does not declare",
            with_inputs("      pr: \"1\"\n      colour: red\n"),
            Flows::new(&[("review", REVIEW)]),
            vec![mismatch("colour")],
        ),
        (
            "text mixed with a token",
            with_inputs("      pr: \"PR {{input.number}}\"\n"),
            Flows::new(&[("review", REVIEW)]),
            vec![mismatch("pr")],
        ),
        (
            "a literal its type refuses",
            with_inputs("      pr: \"1\"\n      focus: lots\n"),
            Flows::new(&[(
                "review",
                REVIEW
                    .replace(
                        "  focus:\n    default: everything\n",
                        "  focus:\n    type: number\n    default: 1\n",
                    )
                    .as_str(),
            )]),
            vec![mismatch("focus")],
        ),
        (
            "a blank number is no value, so the default applies",
            with_inputs("      pr: \"1\"\n      focus: \"\"\n"),
            Flows::new(&[(
                "review",
                REVIEW
                    .replace(
                        "  focus:\n    default: everything\n",
                        "  focus:\n    type: number\n    default: 1\n",
                    )
                    .as_str(),
            )]),
            vec![],
        ),
        (
            "an included flow that does not load",
            OUTER.to_string(),
            Flows::new(&[(
                "review",
                "version: 1\nnodes:\n  - id: a\n    kind: command\n    run: x\n    deps: [ghost]\n",
            )]),
            vec![ValidationKind::IncludeInvalid {
                flow: flow("review"),
            }],
        ),
    ];
    for (label, text, flows, want) in rows {
        if want.is_empty() {
            load(&text, &flows);
            continue;
        }
        assert_eq!(refused(&text, &flows), want, "{label}");
    }
}

#[test]
fn a_cycle_and_a_chain_too_deep_are_refused() {
    let including = |next: &str| {
        format!("version: 1\nnodes:\n  - id: step\n    kind: include\n    flow: {next}\n")
    };
    let cycle = Flows::new(&[("a", &including("b")), ("b", &including("a"))]);
    assert_eq!(
        refused(&including("a"), &cycle),
        // Found one level down, and reported as what it is.
        vec![ValidationKind::IncludeCycle {
            flow: "a".to_string()
        }]
    );
    let issue = crate::load_with(&including("b"), None, &cycle);
    let Err(crate::FlowError::Validate(issues)) = issue else {
        panic!("a cycle must be refused");
    };
    assert!(
        issues[0].message.contains("cannot include `b` here"),
        "the innermost refusal names the cycle: {}",
        issues[0].message
    );

    let leaf = "version: 1\nnodes:\n  - id: x\n    kind: command\n    run: \"true\"\n";
    let chain = Flows::new(&[
        ("one", &including("two")),
        ("two", &including("three")),
        ("three", leaf),
    ]);
    let loaded = load(&including("one"), &chain);
    let order: Vec<_> = loaded.flow().nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(
        order,
        ["step--step--step--x"],
        "three levels is the limit, and fits"
    );
    let deeper = Flows::new(&[
        ("one", &including("two")),
        ("two", &including("three")),
        ("three", &including("four")),
        ("four", leaf),
    ]);
    assert_eq!(
        refused(&including("one"), &deeper),
        vec![ValidationKind::IncludeTooDeep {
            flow: "four".to_string()
        }],
        "the innermost refusal reaches the top as itself"
    );
}

#[test]
fn the_include_chain_judges_cycles_and_depth_together() {
    let root = IncludeChain::default();
    let one = root.enter("a").unwrap();
    assert_eq!(
        one.enter("a").unwrap_err(),
        ValidationKind::IncludeCycle {
            flow: "a".to_string()
        }
    );
    let three = one.enter("b").unwrap().enter("c").unwrap();
    assert_eq!(
        three.enter("d").unwrap_err(),
        ValidationKind::IncludeTooDeep {
            flow: "d".to_string()
        }
    );
}

#[test]
fn a_prompt_file_outside_the_included_flows_folder_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let review = REVIEW.replace(
        "    prompt: \"Review for {{input.focus}}; see {{node.fetch.output}}\"\n",
        "    prompt_file: ../secret.md\n",
    );
    let kinds = refused(OUTER, &Flows::in_dir("review", &review, dir.path()));
    assert!(
        matches!(
            kinds.as_slice(),
            [ValidationKind::PromptFileOutsideFlowDir { .. }]
        ),
        "{kinds:?}"
    );
}

/// A file prompt's `{{input.x}}` belongs to the included flow. One it does
/// not declare must not quietly read the including flow's `x`.
#[test]
fn a_prompt_file_cannot_read_the_including_flows_inputs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ask.md"), "PR {{input.number}}").unwrap();
    let review = REVIEW.replace(
        "    prompt: \"Review for {{input.focus}}; see {{node.fetch.output}}\"\n",
        "    prompt_file: ask.md\n",
    );
    assert_eq!(
        refused(OUTER, &Flows::in_dir("review", &review, dir.path())),
        vec![ValidationKind::IncludeInvalid {
            flow: "review".to_string()
        }]
    );
}

/// Rows: how the included flow names its agents, and whether reading the
/// expansion back from `run.yaml` would still run them as written.
#[test]
fn an_include_whose_agents_would_drift_on_resume_is_refused() {
    let review = |defaults: &str, node_agent: &str| {
        format!(
            "version: 1\n{defaults}nodes:\n  - id: read\n    kind: agent\n{node_agent}    output: r.md\n    prompt: read\n"
        )
    };
    let outer = |defaults: &str| {
        format!(
            "version: 1\n{defaults}nodes:\n  - id: review\n    kind: include\n    flow: review\n"
        )
    };
    let claude = "defaults:\n  agent: { id: claude, mode: default }\n";
    let codex = "defaults:\n  agent: { id: codex, mode: auto }\n";
    let modeless = "defaults:\n  agent: { id: codex }\n";
    let opus = "defaults:\n  agent: { id: claude, mode: default, model: opus }\n";
    let own = "    agent: { id: codex, mode: auto, model: o3 }\n";
    let rows: Vec<(&str, String, String, bool)> = vec![
        ("same defaults", outer(claude), review(claude, ""), true),
        (
            "no outer defaults: the included agent is the unanimous one",
            outer(""),
            review(claude, ""),
            true,
        ),
        (
            "the node names everything itself",
            outer(claude),
            review(codex, own),
            true,
        ),
        (
            "a different agent, written with its mode, is recorded on the node",
            outer(claude),
            review(codex, ""),
            true,
        ),
        (
            "an agent inherited without a mode loses its id on the node",
            outer(claude),
            review(modeless, ""),
            false,
        ),
        (
            "an axis the outer defaults would fill in",
            outer(opus),
            review(claude, ""),
            false,
        ),
        (
            "a repair whose fix would run as the including flow's agent",
            outer(claude),
            format!(
                "version: 1\n{codex}nodes:\n  - id: gate\n    kind: command\n    run: \"true\"\n    on_fail:\n      repair:\n        fix: \"fix {{{{failure}}}}\"\n        max_attempts: 2\n"
            ),
            false,
        ),
    ];
    for (label, outer, review, loads) in rows {
        let flows = Flows::new(&[("review", review.as_str())]);
        let result = crate::load_with(&outer, None, &flows);
        if loads {
            let loaded = result.unwrap_or_else(|e| panic!("{label}: {e:?}"));
            assert_round_trips(&loaded, label);
        } else {
            let Err(crate::FlowError::Validate(issues)) = result else {
                panic!("{label}: expected a refusal, got {result:?}");
            };
            assert_eq!(
                issues.iter().map(|i| &i.kind).collect::<Vec<_>>(),
                [&ValidationKind::IncludeAgentMismatch {
                    flow: "review".to_string()
                }],
                "{label}"
            );
        }
    }
}

#[test]
fn a_member_id_joins_the_include_and_the_node() {
    let id = member_id(&NodeId::from("review"), &NodeId::from("read"));
    assert_eq!(id, "review--read");
}

/// An id inside the included flow that already reads like a member is
/// prefixed once, not chained.
#[test]
fn an_output_ref_is_prefixed_once() {
    let ids: HashSet<NodeId> = [NodeId::from("a"), NodeId::from("x--a")].into();
    let text = "{{node.a.output}} {{node.x--a.output}} {{node.b.output}}";
    assert_eq!(
        prefix_output_refs(text, &ids, &NodeId::from("x")),
        "{{node.x--a.output}} {{node.x--x--a.output}} {{node.b.output}}"
    );
}

const GATED: &str = "\
version: 1
defaults:
  agent: { id: claude, mode: default }
profiles:
  quick:
    timeout: 1m
nodes:
  - id: write
    kind: agent
    output: w.md
    prompt: write
    on_fail:
      retry:
        hint_file: hint.md
        max_attempts: 2
  - id: check
    kind: command
    deps: [write]
    run: test -s {{node.write.output}}
    on_fail:
      repair:
        fix: \"fix {{failure}}\"
        rerun: [write]
        max_attempts: 2
";

/// The shapes the first tests do not reach — two includes in a chain, a
/// repair inside one, a hint file, a profile — each spliced, and each
/// surviving its `run.yaml`.
#[test]
fn every_shape_of_an_include_survives_its_run_yaml() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hint.md"), "again").unwrap();
    let flows = Flows::in_dir("gated", GATED, dir.path());
    let outer = "\
version: 1
defaults:
  agent: { id: claude, mode: default }
nodes:
  - id: one
    kind: include
    flow: gated
    profile: quick
  - id: two
    kind: include
    flow: gated
    deps: [one]
";
    let loaded = load(outer, &flows);
    assert_round_trips(&loaded, "chain");
    let deps = |id: &str| ids(&node(&loaded, id).deps).join(",");
    assert_eq!(
        deps("two--write"),
        "one--check",
        "an include after an include waits for its sinks"
    );
    assert_eq!(
        node(&loaded, "one--write").timeout,
        std::time::Duration::from_secs(60),
        "the profile applies"
    );
    let NodeKind::Command {
        on_fail: GateFail::Repair { rerun, .. },
        ..
    } = &node(&loaded, "two--check").kind
    else {
        panic!("the gate still repairs");
    };
    assert_eq!(
        ids(rerun),
        ["two--write"],
        "a repair reruns its own include's node"
    );
    let body = node(&loaded, "one--write").kind.agent_body().unwrap();
    let AgentFail::Retry { hint, .. } = &body.on_fail else {
        panic!("still retries");
    };
    assert_eq!(
        *hint,
        Prompt::Inline("again".to_string()),
        "a hint file is inlined"
    );
}

/// Each row: a way two names could mean one node.
#[test]
fn an_include_id_that_names_another_node_is_refused() {
    let flows = Flows::new(&[("review", REVIEW)]);
    let with = |extra: &str| format!("{OUTER}{extra}");
    let rows: Vec<(&str, String)> = vec![
        (
            "an own node by the include's name",
            with("  - id: review\n    kind: command\n    run: \"true\"\n"),
        ),
        (
            "two includes by one name",
            with(
                "  - id: review\n    kind: include\n    flow: review\n    inputs:\n      pr: \"1\"\n",
            ),
        ),
        (
            "an include named like another's member",
            with(
                "  - id: review--fetch\n    kind: include\n    flow: review\n    inputs:\n      pr: \"1\"\n",
            ),
        ),
    ];
    for (label, text) in rows {
        assert!(
            refused(&text, &flows).contains(&ValidationKind::DuplicateId),
            "{label}"
        );
    }
}

/// A default spliced into a template would be rendered there, which a run's
/// value never is: `{{…}}` in one is refused rather than made live.
#[test]
fn a_default_that_reads_as_a_template_is_refused() {
    let review = REVIEW.replace(
        "    default: everything\n",
        "    default: \"{{input.number}}\"\n",
    );
    assert_eq!(
        refused(OUTER, &Flows::new(&[("review", &review)])),
        vec![ValidationKind::IncludeInputMismatch {
            flow: "review".to_string(),
            name: "focus".to_string()
        }]
    );
}

#[test]
fn a_token_naming_an_input_the_including_flow_lacks_is_refused() {
    let outer = OUTER.replace("{{input.number}}", "{{input.ghost}}");
    assert_eq!(
        refused(&outer, &Flows::new(&[("review", REVIEW)])),
        vec![ValidationKind::IncludeInputMismatch {
            flow: "review".to_string(),
            name: "pr".to_string()
        }]
    );
}

/// A file prompt's `{{node.X.output}}` is the included flow's: one naming a
/// node it does not have must not read the including flow's `X`.
#[test]
fn a_prompt_file_cannot_read_the_including_flows_outputs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ask.md"), "see {{node.build.output}}").unwrap();
    let review = REVIEW.replace(
        "    prompt: \"Review for {{input.focus}}; see {{node.fetch.output}}\"\n",
        "    prompt_file: ask.md\n",
    );
    assert_eq!(
        refused(OUTER, &Flows::in_dir("review", &review, dir.path())),
        vec![ValidationKind::IncludeInvalid {
            flow: "review".to_string()
        }]
    );
}

#[test]
fn an_included_flow_with_no_nodes_is_refused() {
    let flows = Flows::new(&[(
        "review",
        "version: 1\ninputs:\n  pr:\n    required: true\nnodes: []\n",
    )]);
    let kinds: Vec<ValidationKind> = match crate::load_with(OUTER, None, &flows) {
        Err(crate::FlowError::Validate(issues)) => issues.into_iter().map(|i| i.kind).collect(),
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert_eq!(
        kinds,
        vec![ValidationKind::IncludeInvalid {
            flow: "review".to_string()
        }]
    );
}
