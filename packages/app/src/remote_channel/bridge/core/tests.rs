use super::*;
use daruda_store::project::WorkspaceUuid;

fn pane(n: u64) -> PaneRef {
    PaneRef {
        workspace: WorkspaceUuid::default(),
        pane: n,
    }
}

#[test]
fn opaque_ids_preserve_slack_precision_and_discord_snowflakes() {
    let mut core = RoutingCore::<String>::default();
    let slack = "1700000000.000001".to_owned();
    let discord = "18446744073709551615".to_owned();
    core.record_sent(slack.clone(), pane(1));
    core.record_sent(discord.clone(), pane(2));
    let first = core.route_text("reply".into(), Some(slack), None).ready();
    let second = core.route_text("reply".into(), Some(discord), None).ready();
    assert!(matches!(
        first,
        InboundAction::InjectPrompt {
            pane: PaneRef { pane: 1, .. },
            ..
        }
    ));
    assert!(matches!(
        second,
        InboundAction::InjectPrompt {
            pane: PaneRef { pane: 2, .. },
            ..
        }
    ));
}

fn injected_into(core: &mut RoutingCore<String>) -> Option<PaneRef> {
    match core.route_text("carry on".into(), None, None).ready() {
        InboundAction::InjectPrompt { pane, .. } => Some(pane),
        _ => None,
    }
}

/// A pane chosen with `/use` keeps plain text only until another pane speaks:
/// the user is answering whoever spoke last.
#[test]
fn a_pane_that_speaks_after_the_selection_gets_the_plain_text() {
    let mut core = RoutingCore::<String>::default();
    core.command_state_mut().select(Some(pane(1)));
    core.record_sent("m1".into(), pane(2));
    assert_eq!(injected_into(&mut core), Some(pane(2)));
}

/// The orchestrator reporting on a chat it opened must not pull the phone
/// back to itself; the announced chat stays the target.
#[test]
fn a_bystander_ping_leaves_the_target_alone() {
    let mut core = RoutingCore::<String>::default();
    core.command_state_mut().select(Some(pane(1)));
    core.record_sent("announce".into(), pane(1));
    core.record_bystander_sent("report".into(), pane(9));
    assert_eq!(injected_into(&mut core), Some(pane(1)));
    // Still answerable by replying to it.
    let replied = core
        .route_text("ok".into(), Some("report".into()), None)
        .ready();
    assert!(matches!(
        replied,
        InboundAction::InjectPrompt {
            pane: PaneRef { pane: 9, .. },
            ..
        }
    ));
}

/// With nothing chosen and only a bystander heard from, plain text still has
/// somewhere to go rather than none.
#[test]
fn a_bystander_is_the_last_resort() {
    let mut core = RoutingCore::<String>::default();
    core.record_bystander_sent("report".into(), pane(9));
    assert_eq!(injected_into(&mut core), Some(pane(9)));
}

#[test]
fn only_the_orchestrators_ping_is_a_bystander() {
    let mut core = RoutingCore::<String>::default();
    let orchestrator = Some(pane(9));
    core.record_delivered("worker".into(), pane(1), orchestrator);
    core.record_delivered("report".into(), pane(9), orchestrator);
    assert_eq!(injected_into(&mut core), Some(pane(1)));
    core.record_delivered("worker2".into(), pane(2), orchestrator);
    assert_eq!(injected_into(&mut core), Some(pane(2)));
}

/// A pane that spoke and then closed hands plain text back to the pick, not
/// to itself through `last_pinged` — which would fail the same way forever.
#[test]
fn a_gone_pane_that_spoke_hands_plain_text_back_to_the_pick() {
    use crate::remote_channel::command::Absorbed;
    let mut core = RoutingCore::<String>::default();
    core.command_state_mut().select(Some(pane(2)));
    core.record_sent("m1".into(), pane(3));
    assert_eq!(core.forget(pane(3)), Absorbed::SelectionRestored);
    assert_eq!(injected_into(&mut core), Some(pane(2)));
    assert_eq!(core.command_state_mut().selected(), Some(pane(2)));
}

/// With no pick to fall back to, nothing is the target — not the gone pane.
#[test]
fn a_gone_pane_with_no_pick_leaves_plain_text_unaimed() {
    use crate::remote_channel::command::Absorbed;
    let mut core = RoutingCore::<String>::default();
    core.record_sent("m1".into(), pane(3));
    assert_eq!(core.forget(pane(3)), Absorbed::SelectionDropped);
    assert!(matches!(
        core.route_text("carry on".into(), None, None),
        Routed::NeedsTarget(_)
    ));
}

/// A gone pane the plain text was not going to changes nothing worth saying.
#[test]
fn forgetting_a_pane_plain_text_was_not_aimed_at_moves_nothing() {
    use crate::remote_channel::command::Absorbed;
    let mut core = RoutingCore::<String>::default();
    core.record_sent("m1".into(), pane(3));
    core.record_sent("m2".into(), pane(4));
    assert_eq!(core.forget(pane(3)), Absorbed::Nothing);
    assert_eq!(injected_into(&mut core), Some(pane(4)));
}

/// A reply to a question card answers it, slash or not: only a command daruda
/// owns outranks the card.
#[test]
fn only_an_owned_command_outranks_a_question_card_reply() {
    let mut core = RoutingCore::<String>::default();
    let question = super::super::QuestionPromptRef {
        session: 1,
        question_id: 4,
        field: 0,
    };
    let live = Some(QuestionCard::Live {
        pane: pane(1),
        question,
    });
    let reply = |core: &mut RoutingCore<String>, text: &str, card| {
        core.route_text(text.into(), Some("m".into()), card).ready()
    };
    assert!(matches!(
        reply(&mut core, "/list", live),
        InboundAction::RunCommand { .. }
    ));
    assert_eq!(
        reply(&mut core, "/usage", live),
        InboundAction::RespondQuestion {
            pane: pane(1),
            question,
            reply: QuestionReply::Text("/usage".into()),
        }
    );
    assert_eq!(
        reply(&mut core, "late", Some(QuestionCard::Settled)),
        InboundAction::StaleQuestion
    );
}

/// A reply inside one pane's thread goes to that pane and leaves the
/// plain-text target where it was, so the next top-level message is not
/// redirected by an answer given elsewhere.
#[test]
fn a_thread_reply_reaches_its_pane_without_moving_the_target() {
    let mut core = RoutingCore::<String>::default();
    core.record_sent("1.1".into(), pane(1));
    assert!(matches!(
        core.route_text_to("go".into(), pane(2), None).ready(),
        InboundAction::InjectPrompt {
            pane: PaneRef { pane: 2, .. },
            ..
        }
    ));
    assert_eq!(injected_into(&mut core), Some(pane(1)));
}
