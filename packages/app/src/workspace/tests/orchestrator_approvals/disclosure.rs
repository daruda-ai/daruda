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
use daruda_control_types::agent_text::{PROMPT_EXCERPT_HEAD_CHARS, PROMPT_EXCERPT_TAIL_CHARS};
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
    let prompt = format!(
        "Fix the login\nredirect.  {} and finally drop the users table",
        "x".repeat(400)
    );
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
    // A long prompt is cut in the middle: its end is quoted, with how much
    // of it the card leaves out.
    assert!(card.contains("drop the users table”"), "{card}");
    let hidden = prompt.chars().count() - 1 - PROMPT_EXCERPT_HEAD_CHARS - PROMPT_EXCERPT_TAIL_CHARS;
    assert!(
        card.contains(&s::control::approval_prompt_elided(hidden)),
        "{card}"
    );
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

/// `worktree_create` quotes the prompt it will deliver: a blank one reads as
/// no prompt, and an invisible one as the count of what it carries.
#[gpui::test]
fn a_worktree_card_quotes_its_prompt_when_it_carries_one(cx: &mut TestAppContext) {
    let (_fixture, lane, claude) = host(cx);
    let agent = s::control::approval_agent(&claude.name, BYPASS);
    for prompt in [
        Some("Port the tax table\nto the new API"),
        None,
        Some(""),
        Some(" \u{200B}\u{202E}"),
    ] {
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
        let name = s::control::approval_quoted("disclosure");
        let hidden = s::control::approval_hidden_chars(2);
        let expected = match prompt.map(str::trim) {
            Some("") | None => s::control::approval_lane_create(&name, &agent),
            Some(p) if p.starts_with("Port") => s::control::approval_lane_create_prompt(
                &name,
                &agent,
                s::control::approval_quoted("Port the tax table to the new API"),
            ),
            Some(_) => s::control::approval_lane_create_prompt(&name, &agent, hidden.trim()),
        };
        assert_eq!(card, expected);
        refuse(id, cx);
    }
}

/// A title someone else wrote cannot reverse the rest of its card, which
/// counts what it dropped, and a task with a blank prompt starts without
/// quoting one.
#[gpui::test]
async fn a_task_title_reads_plainly_and_an_empty_prompt_is_not_quoted(cx: &mut TestAppContext) {
    let (fixture, lane, claude) = host(cx);
    let title = "Login\u{202E} \u{2066}evil";
    let (id, _outcome, card) = ask(
        GatedCommand::TaskCreate {
            workspace: lane.workspace,
            project: lane.project,
            task: crate::test_support::new_task(title, "Fix it", Some(lane)),
        },
        cx,
    );
    let shown = format!(
        "{}{}",
        s::control::approval_quoted("Login evil"),
        s::control::approval_hidden_chars(2)
    );
    assert!(card.contains(&shown), "{card}");
    assert!(
        !card.contains('\u{202E}') && !card.contains('\u{2066}'),
        "{card}"
    );
    refuse(id, cx);

    let created = fixture.workspace.update(cx, |ws, cx| {
        ws.control_task_create(
            lane.project,
            crate::test_support::new_task(title, " \n ", Some(lane)),
            cx,
        )
        .unwrap()
    });
    cx.run_until_parked();
    let task = created.await.unwrap();
    let (id, _outcome, card) = ask(GatedCommand::TaskStart { task }, cx);
    assert_eq!(
        card,
        s::control::approval_task_start_no_prompt(
            &shown,
            s::control::approval_agent(&claude.name, BYPASS)
        )
    );
    refuse(id, cx);
}

/// Reopening a task's chat in a new pane names the agent and the mode that
/// pane asks for; bringing up the pane still open names neither, since it
/// keeps its own.
#[gpui::test]
fn a_task_open_card_names_the_mode_only_for_a_new_pane(cx: &mut TestAppContext) {
    let (fixture, _lane, claude) = host(cx);
    let (task, pane) = cx
        .update_window(fixture.window.into(), |_, window, cx| {
            fixture.workspace.update(cx, |ws, cx| {
                let cwd = ws.active_lane().unwrap().path.clone();
                let pane = ws.create_agent_chat_pane(
                    Some(daruda_store::project::PaneCwd::Local(cwd)),
                    Some("task-session".into()),
                    claude.id.clone(),
                    None,
                    window,
                    cx,
                );
                let pane_id = pane.id;
                ws.active_runtime_mut().panes.push(pane);
                let project = ws.project_for(ws.active.project).unwrap().uuid;
                let spec = crate::runs::seed::spec(project, "Login\u{202E} evil");
                let task = crate::runs::seed::task(spec, None, cx);
                ws.run_task_in_chat_for_test(&task, pane_id, cx);
                (task, pane_id)
            })
        })
        .unwrap();
    let shown = format!(
        "{}{}",
        s::control::approval_quoted("Login evil"),
        s::control::approval_hidden_chars(1)
    );
    let (id, _outcome, card) = ask(GatedCommand::TaskOpen { task: task.clone() }, cx);
    assert_eq!(card, s::control::approval_task_open(&shown));
    refuse(id, cx);

    fixture.workspace.update(cx, |ws, _| {
        ws.active_runtime_mut().panes.retain(|p| p.id != pane);
    });
    let (id, _outcome, card) = ask(GatedCommand::TaskOpen { task }, cx);
    assert_eq!(
        card,
        s::control::approval_task_open_new(
            &shown,
            s::control::approval_agent(&claude.name, BYPASS)
        )
    );
    refuse(id, cx);
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

/// The agent a card pinned is removed while it waits: approving opens no
/// other agent in its place, and nothing is recorded, created or opened.
#[gpui::test]
async fn an_approval_whose_pinned_agent_was_removed_is_refused(cx: &mut TestAppContext) {
    let (fixture, lane, claude) = host(cx);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let created = fixture.workspace.update(cx, |ws, cx| {
        ws.control_task_create(
            lane.project,
            crate::test_support::new_task("Login", "Fix the login", Some(lane)),
            cx,
        )
        .unwrap()
    });
    cx.run_until_parked();
    let task = created.await.unwrap();
    cx.executor().allow_parking();
    let commands = [
        GatedCommand::ChatNew {
            lane,
            agent: Some("other".into()),
        },
        GatedCommand::LaneCreate {
            workspace: lane.workspace,
            project: lane.project,
            name: "removed-agent".into(),
            base_ref: None,
            agent: Some("other".into()),
            prompt: None,
        },
        GatedCommand::TaskStart { task: task.clone() },
    ];
    for command in commands {
        fixture.workspace.update(cx, |ws, _| {
            set_catalog(ws, &claude);
            ws.last_agent_id = Some("other".into());
        });
        let (id, outcome, card) = ask(command, cx);
        assert!(card.contains("Other Agent"), "{card}");
        let before = fixture
            .workspace
            .read_with(cx, |ws, _| (ws.all_pane_ids(), lane_count(ws)));
        // Removed through the path a Settings edit takes.
        let without_other = daruda_config::Config {
            agents: vec![daruda_config::AgentEntry::custom(claude.clone())],
            ..daruda_config::Config::default()
        };
        fixture
            .workspace
            .update(cx, |ws, cx| ws.reload_config(&without_other, cx));
        cx.update(|cx| assert!(resolve_desktop(id.0, "approve", cx)));
        let refused = outcome.recv().await.unwrap().unwrap();
        cx.run_until_parked();
        assert_eq!(
            refused,
            Err(crate::control::result::ControlError::AgentUnavailable)
        );
        let after = fixture
            .workspace
            .read_with(cx, |ws, _| (ws.all_pane_ids(), lane_count(ws)));
        assert_eq!(after, before);
    }
    let lifecycle = cx.update(|cx| {
        cx.global::<crate::runs::GlobalTasks>()
            .get(&task)
            .unwrap()
            .lifecycle
            .clone()
    });
    assert_eq!(lifecycle, daruda_store::tasks::Lifecycle::Backlog);
}

fn lane_count(ws: &Workspace) -> usize {
    ws.projects.iter().map(|p| p.lanes.len()).sum()
}
