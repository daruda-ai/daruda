//! What a reload should do, decided before anything is done.
//!
//! GPUI-free: every question here is about bytes, a model and a set of typed
//! values, and none of it needs a window. That matters because this is where
//! the pane's bugs have been — a graph rebuilt when it did not have to be, a
//! restored file that never redrew, typing thrown away without a word — and
//! each one had to be reproduced through a window to be seen at all.
//!
//! The view reads the file and carries the decision out. It does not make it.

use super::FlowGraphError;
use super::model::FlowGraphModel;

/// What is on screen now, as the decision needs it. The canvas is deliberately
/// absent: it is an entity, and nothing here may hold one.
pub(super) enum Drawn<'a> {
    Graph(&'a FlowGraphModel),
    Nothing(&'a FlowGraphError),
}

/// What to do about the file as it now reads.
#[derive(Debug, PartialEq)]
pub(super) enum Reload {
    /// The file says what the pane already shows.
    Unchanged,
    /// Same nodes, same lines between them — the layout still holds, so the
    /// canvas stays and only the cards are written again. This is what keeps a
    /// save from moving the picture under the person who pressed it.
    Restamp { text: String, model: FlowGraphModel },
    /// A node arrived or left: the layout is genuinely different.
    Rebuild { text: String, model: FlowGraphModel },
    /// Say why instead of drawing.
    ///
    /// `text` is `Some` when the file was read but does not load — a later
    /// reload of those same bytes is then rightly nothing to do — and `None`
    /// when it could not be read at all, so that the *same* bytes coming back
    /// count as a change and redraw.
    Unreadable {
        text: Option<String>,
        error: FlowGraphError,
    },
}

/// Decide, given what the file reads as and what the pane shows.
pub(super) fn decide(
    incoming: Result<String, FlowGraphError>,
    shown_text: Option<&str>,
    drawn: Drawn<'_>,
    sources: Option<&crate::workspace::flow_paths::FlowSources>,
) -> Reload {
    let text = match incoming {
        Ok(text) => text,
        Err(error) => {
            // Already saying this, about a file it could not read either: there
            // is nothing to change.
            if shown_text.is_none() && matches!(drawn, Drawn::Nothing(shown) if *shown == error) {
                return Reload::Unchanged;
            }
            return Reload::Unreadable { text: None, error };
        }
    };
    let same_text = shown_text == Some(text.as_str());
    // The same bytes are nothing to do — unless the file includes another
    // flow, whose own edits this file's text cannot show. Those are read
    // again, and only a picture that came out different is redrawn.
    if same_text && !includes_a_flow(&text) {
        return Reload::Unchanged;
    }
    match model_from(&text, sources) {
        Ok(model) => match drawn {
            Drawn::Graph(current) if same_text && *current == model => Reload::Unchanged,
            Drawn::Graph(current) if same_shape(current, &model) => Reload::Restamp { text, model },
            _ => Reload::Rebuild { text, model },
        },
        Err(error) => match drawn {
            Drawn::Nothing(shown) if same_text && *shown == error => Reload::Unchanged,
            _ => Reload::Unreadable {
                text: Some(text),
                error,
            },
        },
    }
}

/// Whether `text` has an `include` node. A file that does not parse has none
/// worth reading again for.
fn includes_a_flow(text: &str) -> bool {
    daruda_flow::parse::parse_flow_file(text).is_ok_and(|file| {
        file.nodes
            .iter()
            .any(|node| matches!(node.kind, daruda_flow::parse::NodeKindFile::Include { .. }))
    })
}

/// The model to draw, and what the engine refuses about it.
///
/// `inspect` rather than `load`: the graph-dependent rules run on a resolved
/// flow and a built graph, so a flow they refuse is one that can still be
/// drawn — and a card that says where the problem is beats a banner that took
/// the picture away. The stages before that leave nothing to draw and stay a
/// refusal here.
///
/// An `include` is expanded from `sources` and drawn as the one node the file
/// wrote ([`FlowGraphModel::collapsed`]).
pub(super) fn model_from(
    text: &str,
    sources: Option<&crate::workspace::flow_paths::FlowSources>,
) -> Result<FlowGraphModel, FlowGraphError> {
    match crate::workspace::flow_include::inspect_flow(text, None, sources) {
        Ok(inspected) => Ok(FlowGraphModel::collapsed(
            &inspected.loaded,
            inspected.issues,
        )),
        Err(daruda_flow::FlowError::Parse(detail)) => Err(FlowGraphError::Parse { detail }),
        // Worded here, not carried: `ValidationIssue.message` is the engine's
        // developer detail and says so, and `s::flow::issue` exists to be the
        // one place a `ValidationKind` becomes something a person reads.
        Err(daruda_flow::FlowError::Validate(issues)) => Err(FlowGraphError::Validate {
            issues: crate::surface::strings::flow::issue_lines(&issues),
        }),
    }
}

/// Everything the layout is derived from: the node ids in order, and both kinds
/// of line. What a card *says* — a prompt, an output, a policy — is not shape,
/// and changing one of those is the common save.
fn same_shape(a: &FlowGraphModel, b: &FlowGraphModel) -> bool {
    a.nodes
        .iter()
        .map(|n| &n.id)
        .eq(b.nodes.iter().map(|n| &n.id))
        && a.deps == b.deps
        && a.rerun == b.rerun
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: &str = "\
version: 1
defaults:
  agent:
    id: claude
    mode: bypassPermissions
nodes:
  - id: design
    kind: agent
    output: design.md
    prompt: write a line
  - id: build
    kind: agent
    deps: [design]
    output: build.md
    prompt: build it
";

    fn model(text: &str) -> FlowGraphModel {
        model_from(text, None).expect("the fixture loads")
    }

    #[test]
    fn the_same_bytes_are_nothing_to_do() {
        let current = model(TWO);
        assert_eq!(
            decide(Ok(TWO.to_string()), Some(TWO), Drawn::Graph(&current), None),
            Reload::Unchanged
        );
    }

    /// A file that includes another reads the same while the other changes
    /// under it: the same bytes are read again, and only a different picture
    /// redraws.
    #[test]
    fn the_same_bytes_with_an_edited_include_redraw() {
        let root = tempfile::tempdir().unwrap();
        let sources = crate::workspace::flow_paths::FlowSources {
            lane: root.path().join("lane"),
            project: None,
            global: root.path().join("global"),
        };
        std::fs::create_dir_all(&sources.global).unwrap();
        let steps = |nodes: &str| format!("version: 1\nnodes:\n{nodes}");
        let one = "  - id: a\n    kind: command\n    run: \"true\"\n";
        let two = "  - id: a\n    kind: command\n    run: \"true\"\n  - id: b\n    kind: command\n    deps: [a]\n    run: \"true\"\n";
        std::fs::write(sources.global.join("steps.yaml"), steps(one)).unwrap();
        let text = "version: 1\nnodes:\n  - id: s\n    kind: include\n    flow: steps\n";
        let current = model_from(text, Some(&sources)).expect("the include expands");

        assert_eq!(
            decide(
                Ok(text.to_string()),
                Some(text),
                Drawn::Graph(&current),
                Some(&sources)
            ),
            Reload::Unchanged,
            "nothing changed anywhere"
        );
        std::fs::write(sources.global.join("steps.yaml"), steps(two)).unwrap();
        assert!(
            matches!(
                decide(
                    Ok(text.to_string()),
                    Some(text),
                    Drawn::Graph(&current),
                    Some(&sources)
                ),
                Reload::Restamp { .. }
            ),
            "the included flow grew, and the card says so"
        );
    }

    /// The common save: one field, same nodes. The canvas has to survive it.
    #[test]
    fn a_changed_field_keeps_the_layout() {
        let current = model(TWO);
        let edited = TWO.replace("build it", "build it twice");
        assert!(matches!(
            decide(Ok(edited), Some(TWO), Drawn::Graph(&current), None),
            Reload::Restamp { .. }
        ));
    }

    #[test]
    fn a_node_arriving_or_leaving_is_a_new_layout() {
        let current = model(TWO);
        let gone = TWO.replace(
            "  - id: build\n    kind: agent\n    deps: [design]\n    output: build.md\n    prompt: build it\n",
            "",
        );
        assert!(matches!(
            decide(Ok(gone), Some(TWO), Drawn::Graph(&current), None),
            Reload::Rebuild { .. }
        ));
    }

    /// A line changing moves nothing on a card and everything in the layout.
    #[test]
    fn a_dependency_changing_is_a_new_layout() {
        let current = model(TWO);
        let unchained = TWO.replace("    deps: [design]\n", "");
        assert!(matches!(
            decide(Ok(unchained), Some(TWO), Drawn::Graph(&current), None),
            Reload::Rebuild { .. }
        ));
    }

    /// The trap a restored file walked into: nothing on screen, and bytes that
    /// match what the pane last held. Keeping the text would make this
    /// `Unchanged` and leave the pane on its message for good — so a fall to
    /// `Unreadable` drops the text with it.
    #[test]
    fn a_file_that_could_not_be_read_keeps_no_text() {
        let error = FlowGraphError::Parse {
            detail: "gone".into(),
        };
        assert_eq!(
            decide(
                Err(error.clone()),
                Some(TWO),
                Drawn::Graph(&model(TWO)),
                None
            ),
            Reload::Unreadable { text: None, error }
        );
    }

    /// But a file that reads and does not load keeps its bytes: reading the
    /// same broken file again is genuinely nothing to do.
    #[test]
    fn a_file_that_does_not_load_keeps_its_bytes() {
        let broken = "nodes: [".to_string();
        let Reload::Unreadable { text, .. } = decide(
            Ok(broken.clone()),
            Some(TWO),
            Drawn::Graph(&model(TWO)),
            None,
        ) else {
            panic!("a file that does not load is unreadable");
        };
        assert_eq!(text.as_deref(), Some(broken.as_str()));
    }

    #[test]
    fn the_same_failure_twice_is_nothing_to_do() {
        let error = FlowGraphError::Parse {
            detail: "same".into(),
        };
        assert_eq!(
            decide(Err(error.clone()), None, Drawn::Nothing(&error), None),
            Reload::Unchanged
        );
        let other = FlowGraphError::Parse {
            detail: "different".into(),
        };
        assert!(matches!(
            decide(Err(other), None, Drawn::Nothing(&error), None),
            Reload::Unreadable { .. }
        ));
    }
}
