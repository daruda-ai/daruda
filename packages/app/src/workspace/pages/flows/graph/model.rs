//! A flow, in the shape the canvas draws it. Pure — no GPUI, no vendor
//! types — so the mapping is testable on its own.
//!
//! The model carries *data*, not wording. Formatting an axis into
//! `claude · effort high · 10m` belongs to the renderer, which is also the
//! only layer allowed to reach `surface::strings`.

use std::path::PathBuf;
use std::time::Duration;

use daruda_flow::NodeId;
use daruda_flow::model::{AgentFail, Flow, GateFail, NodeKind, Prompt};

/// Nodes and the two kinds of line between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FlowGraphModel {
    pub nodes: Vec<GraphNode>,
    /// `deps` — execution order. Drawn as the canvas's own edges.
    pub deps: Vec<GraphEdge>,
    /// A gate's `rerun` — nodes it sends back to be re-derived.
    ///
    /// **Not a dependency.** In the model this lives inside
    /// `GateFail::Repair`, so it is drawn as an overlay rather than as a
    /// graph edge; giving it an `Edge` would put it in the same visual
    /// vocabulary as `deps` and say the wrong thing.
    pub rerun: Vec<GraphEdge>,
    /// What the engine's graph-dependent rules refuse about this flow.
    ///
    /// Carried rather than turned into a refusal, because they are about a
    /// flow that resolved and whose graph built — so the cards can be drawn
    /// and the ones at fault can say so. The run still refuses it: the engine
    /// asks `load`, which is `inspect` plus that refusal.
    pub issues: Vec<daruda_flow::error::ValidationIssue>,
    /// The node ids a run of this flow reports on, in the flow's order. The
    /// drawn ids, except where an include's card stands for its members.
    pub run_ids: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct GraphEdge {
    pub from: NodeId,
    pub to: NodeId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct GraphNode {
    pub id: NodeId,
    pub kind: GraphNodeKind,
    /// Where this node runs, relative to the run directory. `None` is that
    /// directory itself — which is also what lets two nodes overlap.
    pub cwd: Option<PathBuf>,
    pub fail: FailPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum GraphNodeKind {
    Agent {
        agent: AgentAxes,
        prompt: PromptSummary,
        output: PathBuf,
        timeout: Duration,
    },
    /// A shell line whose exit code is the verdict.
    Gate { run: String, timeout: Duration },
    /// Another flow's nodes, drawn as the one node the file wrote. Only the
    /// editor's picture has one ([`FlowGraphModel::collapsed`]); a run's
    /// graph shows what was expanded.
    Include {
        flow: String,
        profile: Option<String>,
        /// The ids its nodes run under, which a run reports them by.
        members: Vec<NodeId>,
    },
}

/// The agent axes worth seeing without opening the node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct AgentAxes {
    pub id: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum PromptSummary {
    /// First line of an inline prompt — the card has one line to spend.
    Inline(String),
    File(PathBuf),
}

/// What the node does when it fails, in the terms the card shows.
///
/// A gate that repairs says so even when `rerun` is empty: an empty set
/// re-runs the gate alone, which is a real policy and not the absence of
/// one, and no edge would otherwise reveal it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum FailPolicy {
    Halt,
    Retry {
        max_attempts: u32,
    },
    Repair {
        max_attempts: u32,
        rerun_count: usize,
    },
}

pub(in crate::workspace) use crate::runs::{NodeRunState, RunColouring};

impl FlowGraphModel {
    pub(super) fn from_flow(flow: &Flow, issues: Vec<daruda_flow::error::ValidationIssue>) -> Self {
        let mut deps = Vec::new();
        let mut rerun = Vec::new();
        let nodes: Vec<GraphNode> = flow
            .nodes
            .iter()
            .map(|node| {
                for dep in &node.deps {
                    deps.push(GraphEdge {
                        from: dep.clone(),
                        to: node.id.clone(),
                    });
                }
                if let NodeKind::Command {
                    on_fail: GateFail::Repair { rerun: members, .. },
                    ..
                } = &node.kind
                {
                    for member in members {
                        rerun.push(GraphEdge {
                            from: node.id.clone(),
                            to: member.clone(),
                        });
                    }
                }
                GraphNode {
                    id: node.id.clone(),
                    kind: kind_of(node),
                    cwd: node.cwd.clone(),
                    fail: fail_of(&node.kind),
                }
            })
            .collect();
        let run_ids = nodes
            .iter()
            .map(|node: &GraphNode| node.id.clone())
            .collect();
        Self {
            nodes,
            deps,
            rerun,
            issues,
            run_ids,
        }
    }

    /// The editor's picture of `loaded`: each `include` the one node the file
    /// wrote, standing for the members it expanded into. Lines into or out of
    /// a member are drawn to the include; lines between members are not drawn,
    /// and an issue about a member is the include's.
    pub(super) fn collapsed(
        loaded: &daruda_flow::LoadedFlow,
        issues: Vec<daruda_flow::error::ValidationIssue>,
    ) -> Self {
        let mut model = Self::from_flow(loaded.flow(), issues);
        let sites = loaded.includes();
        if sites.is_empty() {
            return model;
        }
        let owner: std::collections::HashMap<&NodeId, &daruda_flow::IncludeSite> = sites
            .iter()
            .flat_map(|site| site.members.iter().map(move |member| (member, site)))
            .collect();
        let card = |id: &NodeId| {
            owner
                .get(id)
                .map_or_else(|| id.clone(), |site| site.id.clone())
        };

        let mut nodes = Vec::with_capacity(model.nodes.len());
        for node in model.nodes {
            match owner.get(&node.id) {
                None => nodes.push(node),
                // Members were spliced in where the include stood, so the
                // first one is where its card goes.
                Some(site) if site.members.first() == Some(&node.id) => nodes.push(GraphNode {
                    id: site.id.clone(),
                    kind: GraphNodeKind::Include {
                        flow: site.flow.clone(),
                        profile: site.profile.clone(),
                        members: site.members.clone(),
                    },
                    cwd: None,
                    fail: FailPolicy::Halt,
                }),
                Some(_) => {}
            }
        }
        let fold = |edges: Vec<GraphEdge>| {
            let mut folded: Vec<GraphEdge> = Vec::new();
            for edge in edges {
                let edge = GraphEdge {
                    from: card(&edge.from),
                    to: card(&edge.to),
                };
                if edge.from != edge.to && !folded.contains(&edge) {
                    folded.push(edge);
                }
            }
            folded
        };
        model.deps = fold(std::mem::take(&mut model.deps));
        model.rerun = fold(std::mem::take(&mut model.rerun));
        for issue in &mut model.issues {
            if let Some(node) = issue.node.as_mut() {
                *node = card(node);
            }
        }
        model.nodes = nodes;
        model
    }

    /// Whether `node` is an include's card, which stands for nodes the run
    /// knows by other ids — so it cannot be where a run stops.
    pub(in crate::workspace) fn is_include(&self, node: &NodeId) -> bool {
        self.nodes
            .iter()
            .any(|n| &n.id == node && matches!(n.kind, GraphNodeKind::Include { .. }))
    }

    /// How many refusals name `node`. Flow-level ones — a cycle, an unknown
    /// version — name none, and stay in the banner where the whole sentence
    /// is readable.
    pub(in crate::workspace) fn issues_naming(&self, node: &NodeId) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.node.as_ref() == Some(node))
            .count()
    }
}

fn kind_of(node: &daruda_flow::model::Node) -> GraphNodeKind {
    match &node.kind {
        NodeKind::Agent(body) => GraphNodeKind::Agent {
            agent: AgentAxes {
                id: body.agent.id.clone(),
                model: body.agent.model.clone(),
                effort: body.agent.effort.clone(),
                mode: body.agent.mode.clone(),
            },
            prompt: match &body.prompt {
                Prompt::Inline(text) => PromptSummary::Inline(first_line(text)),
                Prompt::File(path) => PromptSummary::File(path.clone()),
            },
            output: body.output.clone(),
            timeout: node.timeout,
        },
        NodeKind::Command { run, .. } => GraphNodeKind::Gate {
            run: run.clone(),
            timeout: node.timeout,
        },
    }
}

fn fail_of(kind: &NodeKind) -> FailPolicy {
    match kind {
        // Two levels for the agent arm rather than one nested pattern: its
        // body is boxed, and a box cannot be matched through.
        NodeKind::Agent(body) => match &body.on_fail {
            AgentFail::Halt => FailPolicy::Halt,
            AgentFail::Retry { max_attempts, .. } => FailPolicy::Retry {
                max_attempts: *max_attempts,
            },
        },
        NodeKind::Command {
            on_fail: GateFail::Halt,
            ..
        } => FailPolicy::Halt,
        NodeKind::Command {
            on_fail:
                GateFail::Repair {
                    max_attempts,
                    rerun,
                    ..
                },
            ..
        } => FailPolicy::Repair {
            max_attempts: *max_attempts,
            rerun_count: rerun.len(),
        },
    }
}

/// The card has one line for a prompt, and a prompt is prose that usually
/// opens with the instruction worth seeing.
fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::{NodeRunStates, apply_run_event};
    use daruda_flow::event::FlowEvent;

    /// Every fixture shares one `defaults` block; the test supplies only
    /// the nodes. A backslash line-continuation would eat the leading
    /// indentation and hand YAML a different document, so the node text is
    /// always a plain literal.
    const AGENT_DEFAULTS: &str = concat!(
        "version: 1\n",
        "defaults:\n",
        "  agent:\n",
        "    id: claude\n",
        "    mode: bypassPermissions\n",
        "nodes:\n",
    );

    fn model(nodes: &str) -> FlowGraphModel {
        let yaml = format!("{AGENT_DEFAULTS}{nodes}");
        let loaded = daruda_flow::load(&yaml, None)
            .unwrap_or_else(|e| panic!("fixture should load: {e}\n{yaml}"));
        FlowGraphModel::from_flow(loaded.flow(), Vec::new())
    }

    fn edge(from: &str, to: &str) -> GraphEdge {
        GraphEdge {
            from: from.into(),
            to: to.into(),
        }
    }

    #[test]
    fn a_chain_becomes_one_edge_per_link() {
        let m = model(
            "  - id: a
    kind: agent
    output: a.md
    prompt: first
  - id: b
    kind: agent
    deps: [a]
    output: b.md
    prompt: second
  - id: c
    kind: agent
    deps: [b]
    output: c.md
    prompt: third
",
        );

        assert_eq!(m.nodes.len(), 3);
        assert_eq!(m.deps, vec![edge("a", "b"), edge("b", "c")]);
        assert!(m.rerun.is_empty());
    }

    #[test]
    fn two_nodes_on_one_parent_both_get_an_edge() {
        let m = model(
            "  - id: root
    kind: agent
    output: root.md
    prompt: go
  - id: left
    kind: agent
    deps: [root]
    output: left.md
    prompt: go
  - id: right
    kind: agent
    deps: [root]
    output: right.md
    prompt: go
",
        );

        let from_root: Vec<_> = m.deps.iter().filter(|e| e.from == "root").collect();
        assert_eq!(from_root.len(), 2, "a fan-out draws both branches");
    }

    /// The design's two axes meet here: a gate that repairs sends named
    /// nodes back, and that line is not a dependency.
    #[test]
    fn a_gate_s_rerun_becomes_a_back_edge_not_a_dep() {
        let m = model(
            "  - id: review
    kind: agent
    output: review.md
    prompt: review it
  - id: gate
    kind: command
    deps: [review]
    run: \"true\"
    on_fail:
      repair:
        fix: \"{{failure}} {{attempts}} fix it\"
        rerun: [review]
        max_attempts: 2
",
        );

        assert_eq!(
            m.deps,
            vec![edge("review", "gate")],
            "the only dependency is review -> gate"
        );
        assert_eq!(
            m.rerun,
            vec![edge("gate", "review")],
            "and the back edge runs the other way, from the gate"
        );
    }

    /// An empty `rerun` re-runs the gate alone. There is no edge to draw,
    /// so the card is the only place that can say a repair policy exists.
    #[test]
    fn an_empty_rerun_draws_no_edge_but_still_reports_the_policy() {
        let m = model(
            "  - id: gate
    kind: command
    run: \"true\"
    on_fail:
      repair:
        fix: \"{{failure}} {{attempts}} fix it\"
        max_attempts: 3
",
        );

        assert!(m.rerun.is_empty());
        assert_eq!(
            m.nodes[0].fail,
            FailPolicy::Repair {
                max_attempts: 3,
                rerun_count: 0
            }
        );
    }

    #[test]
    fn an_agent_node_and_a_gate_carry_different_shapes() {
        let m = model(
            "  - id: work
    kind: agent
    agent: { effort: high }
    output: out.md
    timeout: 5m
    prompt: |
      first line worth showing
      second line that is not
  - id: check
    kind: command
    deps: [work]
    run: cargo test
",
        );

        let work = &m.nodes[0];
        assert_eq!(work.fail, FailPolicy::Halt);
        let GraphNodeKind::Agent {
            agent,
            prompt,
            output,
            timeout,
        } = &work.kind
        else {
            panic!("work is an agent node");
        };
        assert_eq!(*timeout, Duration::from_secs(300));
        assert_eq!(agent.id, "claude");
        assert_eq!(agent.effort.as_deref(), Some("high"));
        assert_eq!(
            *prompt,
            PromptSummary::Inline("first line worth showing".into()),
            "the card has one line to spend"
        );
        assert_eq!(output, &PathBuf::from("out.md"));

        assert_eq!(
            m.nodes[1].kind,
            GraphNodeKind::Gate {
                run: "cargo test".into(),
                timeout: Duration::from_secs(600),
            },
            "a gate with no timeout of its own takes the engine's default"
        );
    }

    #[test]
    fn a_file_backed_prompt_keeps_its_path() {
        let m = model(
            "  - id: work
    kind: agent
    output: out.md
    prompt_file: ./prompts/work.md
",
        );

        let GraphNodeKind::Agent { prompt, .. } = &m.nodes[0].kind else {
            panic!("work is an agent node");
        };
        assert_eq!(
            *prompt,
            PromptSummary::File(PathBuf::from("./prompts/work.md"))
        );
    }

    struct Steps;

    impl daruda_flow::IncludeSource for Steps {
        fn find(&self, flow: &str) -> Option<daruda_flow::IncludedFlow> {
            (flow == "steps").then(|| daruda_flow::IncludedFlow {
                text: "version: 1\nnodes:\n  - id: a\n    kind: command\n    run: \"true\"\n  - id: b\n    kind: command\n    deps: [a]\n    run: \"true\"\n  - id: c\n    kind: command\n    deps: [a]\n    run: \"true\"\n".to_string(),
                dir: PathBuf::from("/flows"),
            })
        }
    }

    const INCLUDING: &str = "\
version: 1
nodes:
  - id: build
    kind: command
    run: \"true\"
  - id: steps
    kind: include
    flow: steps
    profile: quick
    deps: [build]
  - id: ship
    kind: command
    deps: [steps]
    run: \"true\"
";

    /// The editor draws the file as written: the include is one card where
    /// it stood, with the lines the file drew to and from it.
    #[test]
    fn an_include_is_drawn_as_one_card() {
        let text = INCLUDING.replace("    profile: quick\n", "");
        let loaded = daruda_flow::load_with(&text, None, &Steps).expect("the include expands");
        let m = FlowGraphModel::collapsed(&loaded, Vec::new());

        let ids: Vec<_> = m.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["build", "steps", "ship"]);
        assert_eq!(
            m.nodes[1].kind,
            GraphNodeKind::Include {
                flow: "steps".into(),
                profile: None,
                members: vec!["steps--a".into(), "steps--b".into(), "steps--c".into()],
            }
        );
        let run: Vec<_> = m.run_ids.iter().map(NodeId::as_str).collect();
        assert_eq!(
            run,
            ["build", "steps--a", "steps--b", "steps--c", "ship"],
            "a run still reports on every member"
        );
        assert_eq!(
            m.deps,
            vec![edge("build", "steps"), edge("steps", "ship")],
            "one line per pair, none inside the include"
        );
        assert!(m.is_include(&"steps".into()));
        assert!(!m.is_include(&"build".into()));

        let expanded = FlowGraphModel::from_flow(loaded.flow(), Vec::new());
        assert_eq!(expanded.nodes.len(), 5, "a run's graph keeps every member");
    }

    #[test]
    fn an_issue_about_a_member_is_the_includes() {
        let text = INCLUDING.replace("    profile: quick\n", "");
        let loaded = daruda_flow::load_with(&text, None, &Steps).expect("the include expands");
        let issue = |node: &str| daruda_flow::error::ValidationIssue {
            node: Some(node.into()),
            kind: daruda_flow::error::ValidationKind::DuplicateOutput,
            message: String::new(),
        };
        let m = FlowGraphModel::collapsed(&loaded, vec![issue("steps--b"), issue("ship")]);
        assert_eq!(m.issues_naming(&"steps".into()), 1);
        assert_eq!(m.issues_naming(&"ship".into()), 1);
    }

    fn fold(events: &[FlowEvent]) -> NodeRunStates {
        let mut states = NodeRunStates::new();
        for event in events {
            apply_run_event(&mut states, event);
        }
        states
    }

    fn started(node: &str, attempt: u32) -> FlowEvent {
        FlowEvent::NodeStarted {
            node: node.into(),
            attempt,
        }
    }

    fn passed(node: &str) -> FlowEvent {
        FlowEvent::NodePassed {
            node: node.into(),
            attempt: 1,
        }
    }

    fn failed(node: &str) -> FlowEvent {
        FlowEvent::NodeFailed {
            node: node.into(),
            attempt: 1,
            failure: daruda_flow::runner::NodeFailure::ContextExhausted,
        }
    }

    /// A node nobody has mentioned is pending, not missing.
    #[test]
    fn a_node_the_run_has_not_reached_is_pending() {
        let states = fold(&[started("design", 1)]);
        assert_eq!(
            states.get("design"),
            Some(&NodeRunState::Running { attempt: 1 })
        );
        assert_eq!(
            states.get("build").copied().unwrap_or_default(),
            NodeRunState::Pending
        );
    }

    /// The straight line through a run: start, pass, next.
    #[test]
    fn a_node_that_runs_and_passes_ends_passed() {
        let states = fold(&[started("design", 1), passed("design"), started("build", 1)]);
        assert_eq!(states.get("design"), Some(&NodeRunState::Passed));
        assert_eq!(
            states.get("build"),
            Some(&NodeRunState::Running { attempt: 1 })
        );
    }

    /// A second attempt is visible as one — the card says which try it is.
    #[test]
    fn a_retry_reports_its_attempt_number() {
        let states = fold(&[started("design", 1), failed("design"), started("design", 2)]);
        assert_eq!(
            states.get("design"),
            Some(&NodeRunState::Running { attempt: 2 })
        );
    }

    /// A gate under repair reads as fixing, and goes back to failed when the
    /// fix ends — it has not re-run yet, so it is not a pass.
    #[test]
    fn a_gate_under_repair_is_fixing_then_failed_again() {
        let mid = fold(&[
            failed("gate"),
            FlowEvent::FixStarted {
                gate: "gate".into(),
            },
        ]);
        assert_eq!(mid.get("gate"), Some(&NodeRunState::Fixing));

        let after = fold(&[
            failed("gate"),
            FlowEvent::FixStarted {
                gate: "gate".into(),
            },
            FlowEvent::FixEnded {
                gate: "gate".into(),
                failure: None,
            },
        ]);
        assert_eq!(after.get("gate"), Some(&NodeRunState::Failed));
    }

    /// The transition `event.rs` says a host cannot infer: a repair sends
    /// nodes that already passed back to pending. Miss it and the re-run set
    /// keeps the green of a result that no longer holds.
    #[test]
    fn rerunning_sends_its_members_back_to_pending() {
        let states = fold(&[
            started("design", 1),
            passed("design"),
            started("verdict", 1),
            passed("verdict"),
            failed("gate"),
            FlowEvent::Rerunning {
                gate: "gate".into(),
                members: vec!["verdict".into()],
            },
        ]);
        assert_eq!(
            states.get("verdict"),
            Some(&NodeRunState::Pending),
            "a re-derived node must lose its pass"
        );
        assert_eq!(
            states.get("design"),
            Some(&NodeRunState::Passed),
            "a node outside the rerun set keeps its result"
        );
        assert_eq!(states.get("gate"), Some(&NodeRunState::Failed));
    }

    /// The end of a run does not repaint anything: the last word on each node
    /// is the word that stands.
    #[test]
    fn the_end_of_a_run_settles_rather_than_clears() {
        let states = fold(&[
            started("design", 1),
            passed("design"),
            FlowEvent::RunEnded {
                end: daruda_flow::event::RunEnd::Done,
            },
        ]);
        assert_eq!(states.get("design"), Some(&NodeRunState::Passed));
    }

    fn colouring(of_nodes: &[&str]) -> RunColouring {
        RunColouring {
            states: NodeRunStates::new(),
            of_nodes: of_nodes
                .iter()
                .map(|s| daruda_flow::NodeId::from(*s))
                .collect(),
        }
    }

    /// A run of a flow with an include reports on the members; the editor
    /// draws the include. The picture is still the one the run is about.
    #[test]
    fn a_run_of_an_include_colours_the_collapsed_picture() {
        let text = INCLUDING.replace("    profile: quick\n", "");
        let loaded = daruda_flow::load_with(&text, None, &Steps).expect("the include expands");
        let m = FlowGraphModel::collapsed(&loaded, Vec::new());
        let ran = colouring(&["build", "steps--a", "steps--b", "steps--c", "ship"]);
        assert!(
            ran.is_about(m.run_ids.iter().cloned()),
            "a run reports on the members the card stands for"
        );
        let before = colouring(&["build", "steps--a", "ship"]);
        assert!(
            !before.is_about(m.run_ids.iter().cloned()),
            "a run of the included flow as it was is a different flow"
        );
    }

    /// A colouring is about the nodes the run was submitted with — the same ids
    /// in the same order. Anything else is a different flow wearing the same
    /// names, which is exactly the case that painted a green card on a node
    /// that never ran.
    #[test]
    fn a_colouring_knows_which_nodes_it_is_about() {
        let c = colouring(&["design", "build"]);
        assert!(c.is_about(["design", "build"].iter().map(|s| NodeId::from(*s))));
        assert!(
            !c.is_about(["design"].iter().map(|s| NodeId::from(*s))),
            "a node gone is a different flow"
        );
        assert!(
            !c.is_about(["design", "build", "ship"].iter().map(|s| NodeId::from(*s))),
            "and so is a node arrived"
        );
        assert!(
            !c.is_about(["build", "design"].iter().map(|s| NodeId::from(*s))),
            "order is part of it — the layout is derived from it"
        );
    }
}
