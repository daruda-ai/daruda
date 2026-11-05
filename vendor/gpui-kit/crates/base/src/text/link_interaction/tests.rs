use super::model::{Gesture, LinkOwner};

#[test]
fn a_dispatch_resets_once_and_context_expires_at_its_end() {
    let mut gesture = Gesture::default();
    let generation = gesture.begin_down(1).unwrap();
    gesture.record_right([1., 2.], "first");
    assert!(gesture.begin_down(1).is_none());
    assert_eq!(gesture.take_right([1., 2.]).as_deref(), Some("first"));
    assert_eq!(gesture.take_right([1., 2.]), None);
    gesture.record_right([1., 2.], "unconsumed");
    gesture.end_down(generation);
    assert_eq!(gesture.take_right([1., 2.]), None);
}

#[test]
fn equal_urls_require_the_same_live_owner_host_range_and_placement() {
    let owner = LinkOwner::default();
    let other = LinkOwner::default();
    for candidate in [
        other.hit(1, [0., 0.], 0..4, "same"),
        owner.hit(2, [0., 0.], 0..4, "same"),
        owner.hit(1, [0., 0.], 5..9, "same"),
        owner.hit(1, [0., 40.], 0..4, "same"),
    ] {
        let mut gesture = Gesture::default();
        gesture.record_left(owner.hit(1, [0., 0.], 0..4, "same"));
        assert!(!gesture.take_left(&candidate));
    }
    let mut gesture = Gesture::default();
    let hit = owner.hit(1, [0., 0.], 0..4, "same");
    gesture.record_left(hit.clone());
    assert!(gesture.take_left(&hit));
    assert!(!gesture.take_left(&hit));
    gesture.record_left(hit.clone());
    drop(owner);
    assert!(!gesture.take_left(&hit));
}

#[test]
fn old_cleanup_cannot_erase_a_new_press_and_release_clears_unconsumed_input() {
    let owner = LinkOwner::default();
    let hit = owner.hit(1, [0., 0.], 0..4, "same");
    let mut gesture = Gesture::default();
    let first = gesture.begin_down(1).unwrap();
    gesture.record_left(hit.clone());
    gesture.end_down(first);
    let release = gesture.begin_up(2).unwrap();
    let next = gesture.begin_down(3).unwrap();
    gesture.record_left(hit.clone());
    gesture.end_up(release);
    assert!(gesture.take_left(&hit));
    gesture.end_down(next);
    gesture.record_left(hit.clone());
    let release = gesture.begin_up(4).unwrap();
    gesture.end_up(release);
    assert!(!gesture.take_left(&hit));
}

#[test]
fn a_wrong_position_consumes_context_instead_of_leaving_it_for_another_host() {
    let mut gesture = Gesture::default();
    gesture.begin_down(1);
    gesture.record_right([1., 2.], "link");
    assert_eq!(gesture.take_right([8., 9.]), None);
    assert_eq!(gesture.take_right([1., 2.]), None);
}

#[test]
fn ephemeral_link_owners_do_not_change_content_equality() {
    use crate::text::inline::InlineState;
    assert_eq!(InlineState::default(), InlineState::default());
}

#[gpui::test]
fn closing_a_window_releases_its_interaction_state(cx: &mut gpui::TestAppContext) {
    use super::{LinkInteraction, Registry};
    use gpui::{Context, IntoElement, Render, Window, div};

    struct Host {
        state: gpui::Entity<LinkInteraction>,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    let (host, window) = cx.add_window_view(|window, cx| Host {
        state: LinkInteraction::acquire(window, cx),
    });
    let weak = window.update(|_, cx| host.read(cx).state.downgrade());
    window.update(|window, _| window.remove_window());
    drop(host);
    // GPUI releases dropped entity values at the end of an App update.
    cx.update(|_| {});
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(weak.upgrade().is_none());
        assert!(cx.global::<Registry>().0.is_empty());
    });
}

#[test]
fn skipped_button_events_invalidate_a_stale_primary() {
    let owner = LinkOwner::default();
    let hit = owner.hit(1, [0., 0.], 0..4, "same");
    for release_sequence in [12, 13] {
        let mut gesture = Gesture::default();
        gesture.begin_down(10);
        gesture.record_left(hit.clone());
        gesture.end_down(10);
        gesture.begin_up(release_sequence);
        assert!(!gesture.take_left(&hit));
    }
}
