//! What a delegated approval card and the new chat's announcement disclose:
//! the agent, the permission mode it asks for and the start of its prompt,
//! with the agent pinned when the card is built.

use super::*;
use crate::control::{
    approval::ApprovalId,
    exec::{self, CommandOrigin, Dispatch, Pending},
    result::{ControlResult, LaneHandle},
    spec::GatedCommand,
};
use crate::workspace::Workspace;
use daruda_config::AgentDefinition;
use futures::{FutureExt as _, StreamExt as _};

/// The display name of Claude's delegated default, as its adapter names it.
const BYPASS: &str = "Bypass Permissions";

/// The orchestrator's host window, whose catalog holds Claude and a second
/// agent; nothing has been used yet, so a chat opens Claude.
fn host(
    cx: &mut TestAppContext,
) -> (
    crate::test_support::ControlFixture,
    LaneHandle,
    AgentDefinition,
) {
    let (fixture, _view) = desktop_host(cx);
    let claude = AgentDefinition::claude_default();
    let lane = fixture.workspace.update(cx, |ws, _| {
        set_catalog(ws, &claude);
        ws.last_agent_id = None;
        LaneHandle::new(ws.uuid(), ws.control_active_lane())
    });
    (fixture, lane, claude)
}

fn set_catalog(ws: &mut Workspace, claude: &AgentDefinition) {
    let other = AgentDefinition {
        id: "other".into(),
        name: "Other Agent".into(),
        ..claude.clone()
    };
    ws.mirrors.agents = vec![claude.clone(), other].into();
}

/// Send `command` for approval and read the card it put up.
fn ask(
    command: GatedCommand,
    cx: &mut TestAppContext,
) -> (
    ApprovalId,
    Receiver<Option<crate::control::result::ControlOutcome>>,
    String,
) {
    let dispatch = cx.update(|cx| exec::run_gated(command, CommandOrigin::Orchestrator, cx));
    let Dispatch::Deferred {
        outcome,
        pending: Pending::Approval(id),
    } = dispatch
    else {
        panic!("the command waits for approval")
    };
    cx.run_until_parked();
    let summaries = cx.update(|cx| crate::control::approval::waiting_summaries_for_test(cx));
    assert_eq!(summaries.len(), 1, "{summaries:?}");
    (id, outcome, summaries[0].clone())
}

fn refuse(id: ApprovalId, cx: &mut TestAppContext) {
    cx.update(|cx| assert!(resolve_desktop(id.0, "refuse", cx)));
    cx.run_until_parked();
}

/// Switch the window's last agent while the card waits, then approve: the
/// chat must still open the agent the card named.
async fn approve_after_switching_agent(
    fixture: &crate::test_support::ControlFixture,
    id: ApprovalId,
    outcome: Receiver<Option<crate::control::result::ControlOutcome>>,
    cx: &mut TestAppContext,
) -> ControlResult {
    fixture
        .workspace
        .update(cx, |ws, _| ws.last_agent_id = Some("other".into()));
    cx.update(|cx| assert!(resolve_desktop(id.0, "approve", cx)));
    cx.executor().allow_parking();
    let result = outcome.recv().await.unwrap().unwrap().unwrap();
    cx.run_until_parked();
    result
}

fn opened_agent(
    fixture: &crate::test_support::ControlFixture,
    chat: crate::remote_channel::bridge::PaneRef,
    cx: &mut TestAppContext,
) -> String {
    fixture.workspace.read_with(cx, |ws, cx| {
        ws.control_chat_label(chat.pane, cx)
            .expect("the chat is open")
            .agent
    })
}

#[gpui::test]
async fn a_task_start_card_names_agent_mode_and_prompt_and_opens_that_agent(
    cx: &mut TestAppContext,
) {
    let (fixture, lane, claude) = host(cx);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let prompt = format!("Fix the login\nredirect.  {}", "x".repeat(400));
    let created = fixture.workspace.update(cx, |ws, cx| {
        ws.control_task_create(
            lane.project,
            crate::test_support::new_task("Login", &prompt, Some(lane)),
            cx,
        )
        .unwrap()
    });
    cx.run_until_parked();
    let task = created.await.unwrap();

    let (id, outcome, card) = ask(GatedCommand::TaskStart { task }, cx);
    assert!(card.contains(&claude.name), "{card}");
    assert!(card.contains(BYPASS), "{card}");
    assert!(card.contains("“Fix the login redirect. xxx"), "{card}");
    assert!(card.contains("…”"), "a long prompt is cut: {card}");
    assert!(!card.contains(&prompt[20..]), "{card}");
    assert!(!card.contains('\n'), "{card}");

    let ControlResult::TaskStarted {
        chat: Some(chat), ..
    } = approve_after_switching_agent(&fixture, id, outcome, cx).await
    else {
        panic!("the task starts in a chat")
    };
    assert_eq!(opened_agent(&fixture, chat, cx), claude.id);
}

/// `chat_new` names the agent and its mode; there is no prompt to quote.
/// The chat opens that agent, and its announcement says which mode it asked
/// for.
#[gpui::test]
async fn a_chat_new_card_names_agent_and_mode_and_its_announcement_the_requested_mode(
    cx: &mut TestAppContext,
) {
    let (fixture, lane, claude) = host(cx);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let mut outbound = cx.update(crate::remote_channel::global::tests::setup);
    // The channels' settings replace the catalog the window mirrors.
    fixture
        .workspace
        .update(cx, |ws, _| set_catalog(ws, &claude));
    cx.update(|cx| {
        crate::app_presence::seed_for_test(
            crate::platform::presence::AwaySignal::HERE,
            true,
            Some(Duration::ZERO),
            cx,
        );
    });

    let (id, outcome, card) = ask(GatedCommand::ChatNew { lane, agent: None }, cx);
    assert_eq!(
        card,
        s::control::approval_chat_new(
            fixture
                .workspace
                .read_with(cx, |ws, _| s::control::lane_path(
                    ws.control_project_name(lane.project),
                    ws.control_lane_name(lane.lane_ref()),
                )),
            s::control::approval_agent(&claude.name, BYPASS),
        )
    );
    while outbound.next().now_or_never().flatten().is_some() {}

    let ControlResult::ChatCreated { target, agent } =
        approve_after_switching_agent(&fixture, id, outcome, cx).await
    else {
        panic!("a chat opens")
    };
    assert_eq!(agent, claude.id);
    assert_eq!(opened_agent(&fixture, target, cx), claude.id);

    let announced: Vec<String> = std::iter::from_fn(|| outbound.next().now_or_never().flatten())
        .map(|pending| format!("{:?}", pending.outbound))
        .collect();
    assert!(!announced.is_empty());
    let expected = s::control::chat_announced_only(&claude.name, BYPASS);
    for message in announced {
        assert!(message.contains(&expected), "{message}");
    }
}

/// `worktree_create` quotes the prompt it will deliver, and only then.
#[gpui::test]
fn a_worktree_card_quotes_its_prompt_when_it_carries_one(cx: &mut TestAppContext) {
    let (_fixture, lane, claude) = host(cx);
    let agent = s::control::approval_agent(&claude.name, BYPASS);
    for prompt in [Some("Port the tax table\nto the new API"), None] {
        let (id, _outcome, card) = ask(
            GatedCommand::LaneCreate {
                workspace: lane.workspace,
                project: lane.project,
                name: "disclosure".into(),
                base_ref: None,
                agent: None,
                prompt: prompt.map(str::to_owned),
            },
            cx,
        );
        let expected = match prompt {
            Some(_) => s::control::approval_lane_create_prompt(
                "disclosure",
                &agent,
                "Port the tax table to the new API",
            ),
            None => s::control::approval_lane_create("disclosure", &agent),
        };
        assert_eq!(card, expected);
        refuse(id, cx);
    }
}

/// Creating a task starts nothing, so its card names no mode: a later start
/// from the desktop runs in the agent's ordinary mode.
#[gpui::test]
fn a_task_create_card_names_no_mode(cx: &mut TestAppContext) {
    let (_fixture, lane, claude) = host(cx);
    let (id, _outcome, card) = ask(
        GatedCommand::TaskCreate {
            workspace: lane.workspace,
            project: lane.project,
            task: crate::test_support::new_task("Login", "Fix the login", Some(lane)),
        },
        cx,
    );
    assert!(!card.contains(BYPASS), "{card}");
    assert!(!card.contains(&claude.name), "{card}");
    refuse(id, cx);
}
