use super::*;

fn fold(events: &[FlowEvent]) -> NodeRunStates {
    let mut states = NodeRunStates::new();
    for event in events {
        apply_run_event(&mut states, event);
    }
    states
}

/// A node takes the colour of the last thing the run said about it; a
/// node the run has not spoken about is pending.
#[test]
fn each_node_wears_the_last_word_on_it() {
    let states = fold(&[
        FlowEvent::NodeStarted {
            node: "design".into(),
            attempt: 2,
        },
        FlowEvent::NodeStarted {
            node: "build".into(),
            attempt: 1,
        },
        FlowEvent::NodePassed {
            node: "build".into(),
            attempt: 1,
        },
    ]);
    assert_eq!(
        states.get(&NodeId::from("design")),
        Some(&NodeRunState::Running { attempt: 2 })
    );
    assert_eq!(
        states.get(&NodeId::from("build")),
        Some(&NodeRunState::Passed)
    );
    assert_eq!(
        states
            .get(&NodeId::from("review"))
            .copied()
            .unwrap_or_default(),
        NodeRunState::Pending
    );
}

/// A repair: the gate is fixing, then still failed until it re-runs, and
/// the members it re-derives lose the pass they had.
#[test]
fn a_repair_takes_back_what_it_re_derives() {
    let states = fold(&[
        FlowEvent::NodePassed {
            node: "design".into(),
            attempt: 1,
        },
        FlowEvent::NodeFailed {
            node: "gate".into(),
            attempt: 1,
            failure: daruda_flow::runner::NodeFailure::Exit { code: Some(1) },
        },
        FlowEvent::FixStarted {
            gate: "gate".into(),
        },
    ]);
    assert_eq!(
        states.get(&NodeId::from("gate")),
        Some(&NodeRunState::Fixing)
    );
    let states = {
        let mut states = states;
        apply_run_event(
            &mut states,
            &FlowEvent::FixEnded {
                gate: "gate".into(),
                failure: None,
            },
        );
        apply_run_event(
            &mut states,
            &FlowEvent::Rerunning {
                gate: "gate".into(),
                members: vec!["design".into()],
            },
        );
        states
    };
    assert_eq!(
        states.get(&NodeId::from("gate")),
        Some(&NodeRunState::Failed)
    );
    assert_eq!(
        states.get(&NodeId::from("design")),
        Some(&NodeRunState::Pending)
    );
}

/// A colouring is only about the graph it was made for: the same ids in
/// the same order.
#[test]
fn a_colouring_is_about_the_nodes_it_was_made_for() {
    let colouring = RunColouring {
        states: NodeRunStates::new(),
        of_nodes: vec!["design".into(), "build".into()],
        ended: false,
    };
    assert!(colouring.is_about(["design", "build"].into_iter().map(NodeId::from)));
    assert!(!colouring.is_about(["build", "design"].into_iter().map(NodeId::from)));
    assert!(!colouring.is_about(["design"].into_iter().map(NodeId::from)));
}

/// Each row: an include's members, whether the run has ended, and the one
/// state its card shows.
#[test]
fn an_includes_card_folds_its_members_states() {
    use NodeRunState::*;
    let rows: Vec<(&str, Vec<NodeRunState>, bool, NodeRunState)> = vec![
        ("nothing started", vec![Pending, Pending], false, Pending),
        ("all passed", vec![Passed, Passed], false, Passed),
        (
            "part way through",
            vec![Passed, Pending],
            false,
            Running { attempt: 1 },
        ),
        // A stopped run leaves the rest pending for good: still showing it
        // running would say work is under way that never will be.
        ("stopped part way", vec![Passed, Pending], true, Pending),
        ("ended all passed", vec![Passed, Passed], true, Passed),
        ("ended on a failure", vec![Passed, Failed], true, Failed),
        (
            "one running",
            vec![Passed, Running { attempt: 1 }, Pending],
            false,
            Running { attempt: 1 },
        ),
        (
            "the latest retry shows",
            vec![Running { attempt: 2 }, Running { attempt: 1 }],
            false,
            Running { attempt: 2 },
        ),
        (
            "a repair outranks running",
            vec![Fixing, Running { attempt: 1 }],
            false,
            Fixing,
        ),
        (
            "a failure is the news",
            vec![Passed, Failed, Fixing],
            false,
            Failed,
        ),
        ("no members", vec![], false, Pending),
    ];
    for (label, members, ended, want) in rows {
        assert_eq!(fold_states(members, ended), want, "{label}");
    }
}

/// An ended run leaves nothing under way: what was running stopped with it,
/// a gate mid-repair keeps its failure, and an include stopped part way
/// reads as pending rather than still running.
#[test]
fn an_ended_run_leaves_nothing_under_way() {
    use NodeRunState::*;
    let live = RunColouring {
        states: [
            ("cut", Running { attempt: 2 }),
            ("gate", Fixing),
            ("done", Passed),
            ("broke", Failed),
            ("s--a", Passed),
        ]
        .into_iter()
        .map(|(id, state)| (NodeId::from(id), state))
        .collect(),
        of_nodes: Vec::new(),
        ended: false,
    };
    let members: Vec<NodeId> = vec!["s--a".into(), "s--b".into()];
    assert_eq!(live.folded(&members), Running { attempt: 1 });

    let ended = live.ended();
    assert!(ended.ended);
    for (id, want) in [
        ("cut", Pending),
        ("gate", Failed),
        ("done", Passed),
        ("broke", Failed),
        ("never", Pending),
    ] {
        assert_eq!(ended.state(&NodeId::from(id)), want, "{id}");
    }
    assert_eq!(ended.folded(&members), Pending);
}
