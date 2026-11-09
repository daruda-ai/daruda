//! Tests for the host-owned Markdown parse store.

use gpui::{
    Context, ElementId, Entity, IntoElement, ParentElement as _, Render, SharedString,
    TestAppContext, Window, div,
};

use super::MarkdownStates;
use crate::test_support::init_gpui_component;

fn retained(states: &MarkdownStates, id: &str) -> Option<gpui::EntityId> {
    states
        .0
        .borrow()
        .get(&ElementId::Name(id.to_string().into()))
        .map(Entity::entity_id)
}

/// What the retained document for `id` renders, once its parse landed.
fn rendered(cx: &TestAppContext, states: &MarkdownStates, id: &str) -> String {
    let state = states
        .0
        .borrow()
        .get(&ElementId::Name(id.to_string().into()))
        .cloned()
        .expect("the body is retained");
    state.read_with(cx, |state, _| state.rendered_text().as_str().to_string())
}

/// One retained body that the test can stop drawing, the way a virtualized
/// list stops drawing a row scrolled past its overdraw.
struct Probe {
    states: MarkdownStates,
    text: SharedString,
    drawn: bool,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let copy = crate::CopyTooltips {
            copy: "Copy".into(),
            copied: "Copied".into(),
        };
        div().children(
            self.drawn.then(|| {
                crate::markdown("body", self.text.clone(), copy).retained_in(&self.states)
            }),
        )
    }
}

fn open_probe(cx: &mut TestAppContext, text: &str) -> (MarkdownStates, Entity<Probe>) {
    init_gpui_component(cx);
    let states = MarkdownStates::default();
    let probe_states = states.clone();
    let text = SharedString::from(text.to_string());
    let window = cx.add_window(move |_, _| Probe {
        states: probe_states,
        text,
        drawn: true,
    });
    cx.run_until_parked();
    let probe = window.root(cx).expect("the probe window is open");
    (states, probe)
}

fn set_probe(cx: &mut TestAppContext, probe: &Entity<Probe>, f: impl FnOnce(&mut Probe)) {
    probe.update(cx, |probe, cx| {
        f(probe);
        cx.notify();
    });
    cx.run_until_parked();
}

#[gpui::test]
async fn a_body_keeps_its_document_across_frames_it_is_not_drawn(cx: &mut TestAppContext) {
    let (states, probe) = open_probe(cx, "# Title\n\nBody");
    let first = retained(&states, "body").expect("drawing the body retains it");

    set_probe(cx, &probe, |probe| probe.drawn = false);
    set_probe(cx, &probe, |probe| probe.drawn = true);

    assert_eq!(
        retained(&states, "body"),
        Some(first),
        "the document must outlive the frames the body was not drawn"
    );
}

#[gpui::test]
async fn new_text_updates_the_retained_document_in_place(cx: &mut TestAppContext) {
    let (states, probe) = open_probe(cx, "streamed");
    let first = retained(&states, "body").expect("drawing the body retains it");

    set_probe(cx, &probe, |probe| probe.text = "streamed further".into());
    assert_eq!(rendered(cx, &states, "body").trim(), "streamed further");
    set_probe(cx, &probe, |probe| probe.text = "replaced".into());
    assert_eq!(rendered(cx, &states, "body").trim(), "replaced");

    assert_eq!(retained(&states, "body"), Some(first));
}

#[gpui::test]
async fn clear_drops_every_retained_document(cx: &mut TestAppContext) {
    let (states, _probe) = open_probe(cx, "body");
    assert!(retained(&states, "body").is_some());

    states.clear();

    assert!(states.0.borrow().is_empty());
}
