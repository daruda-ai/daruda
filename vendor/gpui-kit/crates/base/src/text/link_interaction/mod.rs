//! daruda patch: window-local link gestures shared by independent text hosts.
mod model;
#[cfg(test)]
mod tests;

use gpui::{
    App, AppContext as _, Entity, Global, Pixels, Point, SharedString, WeakEntity, Window, WindowId,
};
use model::Gesture;
pub(super) use model::{LinkHit, LinkOwner};
use std::collections::HashMap;

#[derive(Default)]
struct Registry(HashMap<WindowId, WeakEntity<LinkInteraction>>);
impl Global for Registry {}

#[derive(Default)]
pub(super) struct LinkInteraction {
    gesture: Gesture,
}

impl LinkInteraction {
    pub(super) fn acquire(window: &Window, cx: &mut App) -> Entity<Self> {
        let id = window.window_handle().window_id();
        if let Some(state) = cx
            .default_global::<Registry>()
            .0
            .get(&id)
            .and_then(WeakEntity::upgrade)
        {
            return state;
        }
        let state = cx.new(|cx| {
            let entity_id = cx.entity_id();
            cx.on_release(move |_: &mut Self, cx| {
                if !cx.has_global::<Registry>() {
                    return;
                }
                let registry = &mut cx.global_mut::<Registry>().0;
                if registry
                    .get(&id)
                    .is_some_and(|state| state.entity_id() == entity_id)
                {
                    registry.remove(&id);
                }
            })
            .detach();
            Self::default()
        });
        cx.default_global::<Registry>()
            .0
            .insert(id, state.downgrade());
        state
    }

    pub(super) fn capture_down(state: &Entity<Self>, window: &Window, cx: &mut App) {
        let Some(generation) = state.update(cx, |state, _| {
            state
                .gesture
                .begin_down(window.mouse_button_event_sequence())
        }) else {
            return;
        };
        let state = state.downgrade();
        cx.defer(move |cx| {
            if let Some(state) = state.upgrade() {
                state.update(cx, |state, _| state.gesture.end_down(generation));
            }
        });
    }

    pub(super) fn capture_up(state: &Entity<Self>, window: &Window, cx: &mut App) {
        let Some(generation) = state.update(cx, |state, _| {
            state.gesture.begin_up(window.mouse_button_event_sequence())
        }) else {
            return;
        };
        let state = state.downgrade();
        cx.defer(move |cx| {
            if let Some(state) = state.upgrade() {
                state.update(cx, |state, _| state.gesture.end_up(generation));
            }
        });
    }

    pub(super) fn record_left(&mut self, hit: LinkHit) {
        self.gesture.record_left(hit);
    }
    pub(super) fn record_right(&mut self, at: Point<Pixels>, url: &str) {
        self.gesture.record_right(position(at), url);
    }
    pub(super) fn take_left(&mut self, hit: &LinkHit) -> bool {
        self.gesture.take_left(hit)
    }
}

fn position(at: Point<Pixels>) -> [f32; 2] {
    [f32::from(at.x), f32::from(at.y)]
}

/// The link captured by this window's current right press, consumed once.
/// Read in the host's mouse-down bubble handler; the dispatch end expires it.
pub fn take_right_clicked_link(
    cx: &mut App,
    window: &Window,
    at: Point<Pixels>,
) -> Option<SharedString> {
    let id = window.window_handle().window_id();
    let state = cx.default_global::<Registry>().0.get(&id)?.upgrade()?;
    state
        .update(cx, |state, _| state.gesture.take_right(position(at)))
        .map(Into::into)
}
