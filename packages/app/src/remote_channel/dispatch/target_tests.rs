use super::*;
use crate::remote_channel::dispatch::tests::aim;
use gpui::TestAppContext;

fn dispatch_to_workspace_async(
    cx: &mut gpui::AsyncApp,
    workspace: daruda_store::project::WorkspaceUuid,
    on_match: impl FnMut(&mut Workspace, &mut gpui::Context<Workspace>),
) {
    cx.update(|cx| dispatch_to_workspace(cx, workspace, on_match));
}

#[test]
fn compose_edit_body_appends_outcome_and_falls_back_when_empty() {
    assert_eq!(
        compose_edit_body("Allow write to /tmp/x?", "OK"),
        "Allow write to /tmp/x?\n\n— OK"
    );
    // No original text available → just the label, no stray separator.
    assert_eq!(compose_edit_body("", "OK"), "OK");
}

#[test]
fn permission_feedback_distinguishes_direction_and_reason() {
    let allow = PermissionDecision::Allow("opt".into());
    let reject = PermissionDecision::Reject("opt".into());
    let applied_allow = permission_feedback(&allow, BotPermissionOutcome::Applied);
    let applied_reject = permission_feedback(&reject, BotPermissionOutcome::Applied);
    let stale = permission_feedback(&allow, BotPermissionOutcome::Stale);
    let gone = permission_feedback(&allow, BotPermissionOutcome::Gone);

    // Every outcome yields a distinct, non-empty label so the toast/edit is
    // never blank and Allow reads differently from Reject.
    for label in [&applied_allow, &applied_reject, &stale, &gone] {
        assert!(!label.is_empty());
    }
    assert_ne!(applied_allow, applied_reject);
    assert_ne!(applied_allow, stale);
    assert_ne!(stale, gone);
}

/// A phone reply names its pane by workspace uuid because a pane id is only
/// unique inside one window: the dispatch reaches the window that uuid
/// names, and no other — two windows can hold a pane with the same id.
#[gpui::test]
async fn dispatch_reaches_only_the_window_its_uuid_names(cx: &mut TestAppContext) {
    use crate::test_support::workspace_with_agent_chat;

    let target = workspace_with_agent_chat(cx);
    let other = workspace_with_agent_chat(cx);
    let target_uuid = target.workspace.read_with(cx, |ws, _| ws.uuid());
    assert_ne!(
        target_uuid,
        other.workspace.read_with(cx, |ws, _| ws.uuid())
    );
    let mut async_cx = cx.to_async();

    let mut reached = Vec::new();
    dispatch_to_workspace_async(&mut async_cx, target_uuid, |ws, _| reached.push(ws.uuid()));
    assert_eq!(reached, [target_uuid], "exactly the named window, once");

    reached.clear();
    let closed = daruda_store::project::WorkspaceUuid::new();
    dispatch_to_workspace_async(&mut async_cx, closed, |ws, _| reached.push(ws.uuid()));
    assert!(
        reached.is_empty(),
        "a window that is not open is reached by nothing"
    );
}

/// The restart case the whole fallback exists for: the bridge knows of no
/// target, and the app's own active lane supplies one. Both cases rejoin
/// the arms that already had a pane, so `/usage` reaches the agent and
/// plain text reaches the same chat.
#[gpui::test]
async fn a_targetless_update_adopts_the_apps_own_lane(cx: &mut TestAppContext) {
    use crate::test_support::workspace_with_agent_chat;

    let fixture = workspace_with_agent_chat(cx);
    let expected = fixture.pane_ref(cx);
    let mut async_cx = cx.to_async();

    assert_eq!(
        aim(
            Routed::NeedsTarget(Unaimed::Text {
                text: "ship it".into()
            }),
            &mut async_cx,
        )
        .action,
        InboundAction::InjectPrompt {
            pane: expected,
            text: "ship it".into()
        }
    );
    assert_eq!(
        aim(
            Routed::NeedsTarget(Unaimed::Slash {
                name: "usage".into(),
                text: "/usage".into(),
                suggestion: Some("use"),
            }),
            &mut async_cx,
        )
        .action,
        InboundAction::UnknownSlash {
            pane: expected,
            name: "usage".into(),
            text: "/usage".into(),
            suggestion: Some("use"),
        }
    );
}

/// Ambiguity has to survive the aim step. Two windows each offering their
/// own lane resolve to no candidate, and the update must fall to its
/// terminal answer — picking one of the two would start a turn in
/// whichever the walk happened to see first.
#[gpui::test]
async fn an_ambiguous_app_falls_to_the_terminal_answer(cx: &mut TestAppContext) {
    use crate::test_support::workspace_with_agent_chat;

    let _windows = [workspace_with_agent_chat(cx), workspace_with_agent_chat(cx)];
    let mut async_cx = cx.to_async();

    assert_eq!(
        aim(
            Routed::NeedsTarget(Unaimed::Text {
                text: "ship it".into()
            }),
            &mut async_cx,
        )
        .action,
        InboundAction::NoTarget,
        "two candidates is not a target"
    );
    assert_eq!(
        aim(
            Routed::NeedsTarget(Unaimed::Slash {
                name: "lst".into(),
                text: "/lst".into(),
                suggestion: Some("list"),
            }),
            &mut async_cx,
        )
        .action,
        InboundAction::UnclaimedSlash {
            name: "lst".into(),
            suggestion: Some("list"),
        },
        "the suggestion still reaches the layer that can use it"
    );
}

/// An update that already names its pane is handed straight back — `aim`
/// resolves the target question, it does not revisit a settled one.
#[gpui::test]
async fn an_update_that_already_has_a_target_is_untouched(cx: &mut TestAppContext) {
    use crate::test_support::workspace_with_agent_chat;

    let fixture = workspace_with_agent_chat(cx);
    let pane = fixture.pane_ref(cx);
    let mut async_cx = cx.to_async();

    let action = InboundAction::InjectPrompt {
        pane,
        text: "already aimed".into(),
    };
    assert_eq!(
        aim(Routed::Ready(action.clone()), &mut async_cx).action,
        action
    );
}
