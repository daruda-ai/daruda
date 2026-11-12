//! A flow that includes another, through the paths that build a run from it
//! or write it back.

use super::*;

use crate::workspace::flow_request::FlowSelection;

/// Included from the same repository folder the including flow sits in.
const STEPS: &str = "\
version: 1
defaults:
  agent:
    id: claude
    mode: bypassPermissions
nodes:
  - id: draft
    kind: agent
    output: draft.md
    prompt: write a line
";

const INCLUDING: &str = "\
version: 1
defaults:
  agent:
    id: claude
    mode: bypassPermissions
nodes:
  - id: build
    kind: command
    run: \"true\"
  - id: steps
    kind: include
    flow: steps
    deps: [build]
";

fn with_steps(flow_path: &std::path::Path) {
    std::fs::write(flow_path.with_file_name("steps.yaml"), STEPS).expect("write steps");
}

#[gpui::test]
async fn a_request_runs_the_included_flows_nodes(cx: &mut TestAppContext) {
    let (_lane, ws, flow_path, _wh) = workspace_with_a_flow(cx, INCLUDING);
    with_steps(&flow_path);

    let ids = ws.update(cx, |ws, cx| {
        let submission = ws
            .build_flow_request(ws.active, &flow_path, None, &FlowSelection::default(), cx)
            .unwrap_or_else(|_| panic!("a flow whose include is found builds a request"));
        submission
            .request
            .loaded
            .flow()
            .nodes
            .iter()
            .map(|n| n.id.to_string())
            .collect::<Vec<_>>()
    });
    assert_eq!(ids, ["build", "steps--draft"]);
}

/// The inspector writes a file back only if it still loads; an include it
/// cannot expand would refuse every save on every other node.
#[gpui::test]
async fn an_edit_to_a_flow_with_an_include_is_written(cx: &mut TestAppContext) {
    let (_lane, ws, flow_path, wh) = workspace_with_a_flow(cx, INCLUDING);
    with_steps(&flow_path);
    let mut vcx = gpui::VisualTestContext::from_window(wh.into(), cx);

    let wrote = ws.update_in(&mut vcx, |ws, window, cx| {
        ws.edit_flow(
            &flow_path,
            INCLUDING,
            |file| file.nodes[0].timeout = Some(std::time::Duration::from_secs(5)),
            window,
            cx,
        )
    });
    assert_eq!(wrote, Ok(()));
    let after = std::fs::read_to_string(&flow_path).unwrap();
    assert!(after.contains("    timeout: 5s\n"), "{after}");
    assert!(
        after.contains("    kind: include\n"),
        "the include is left as it was: {after}"
    );
}

/// The editor draws an include as the one node the file wrote, gives it no
/// form to save over it, and will not stop a run there — the run knows its
/// members, not it.
#[gpui::test]
async fn the_graph_draws_an_include_as_one_card(cx: &mut TestAppContext) {
    let (_lane, ws, flow_path, wh) = workspace_with_a_flow(cx, INCLUDING);
    with_steps(&flow_path);
    let mut vcx = gpui::VisualTestContext::from_window(wh.into(), cx);
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.open_flow_graph(ws.active, &flow_path, window, cx)
    });
    vcx.run_until_parked();
    let view = ws
        .read_with(&vcx, |ws, _| ws.open_graph().map(|(_, _, view)| view))
        .expect("the graph opened");
    assert_eq!(
        view.read_with(&vcx, |v, _| v.included_flow(&"steps".into())),
        Some("steps".to_string()),
        "the include is drawn, not an error about it"
    );

    view.update_in(&mut vcx, |v, window, cx| {
        v.select_node_for_test(&"steps".into(), window, cx)
    });
    vcx.run_until_parked();
    let (until, has_form) = view.read_with(&vcx, |v, cx| (v.until_node(cx), v.has_form_for_test()));
    assert_eq!(until, None, "a run cannot stop at an include");
    assert!(!has_form, "no form is bound to an include");

    view.update_in(&mut vcx, |v, window, cx| {
        v.select_node_for_test(&"build".into(), window, cx)
    });
    vcx.run_until_parked();
    assert_eq!(
        view.read_with(&vcx, |v, cx| v.until_node(cx)),
        Some("build".into())
    );
}
