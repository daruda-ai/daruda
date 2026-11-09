//! Which chats run in their agent's delegated mode: those the phone or the
//! orchestrator asked for, from their first connect, through config edits
//! and restarts — and no chat a person opens.

use super::*;
use crate::workspace::ChatOpen;
use crate::workspace::main_area::pane::TabEntry;
use crate::workspace::main_area::pane_tree::{PaneId, PaneLayout};
use daruda_config::{AgentDefinition, ModeChoice, SessionPreferences};
use daruda_store::project::PaneCwd;
use gpui::Entity;

/// The built-in Claude agent with a `default_mode`, so a delegated connect
/// shows the delegated mode ahead of it and an ordinary one shows it alone.
fn claude(delegated_mode: Option<&str>) -> AgentDefinition {
    AgentDefinition {
        default_mode: Some("default".into()),
        delegated_mode: delegated_mode.map(str::to_owned),
        ..AgentDefinition::claude_default()
    }
}

fn config_with(agent: AgentDefinition) -> daruda_config::Config {
    daruda_config::Config {
        agents: vec![daruda_config::AgentEntry::custom(agent)],
        ..daruda_config::Config::default()
    }
}

fn delegated() -> Vec<String> {
    vec!["bypassPermissions".into(), "default".into()]
}

fn ordinary() -> Vec<String> {
    vec!["default".into()]
}

/// The modes `pane`'s faked connect asked for; panics when it never connected.
fn requested_modes(pane: PaneId, cx: &mut TestAppContext) -> Vec<String> {
    let requested = cx.update(Workspace::faked_connect_preferences_for_test);
    let found: Vec<&SessionPreferences> = requested
        .iter()
        .filter(|(id, _)| *id == pane)
        .map(|(_, preferences)| preferences)
        .collect();
    assert_eq!(found.len(), 1, "pane {pane} connected once: {requested:?}");
    found[0].modes.clone()
}

fn mode_of(workspace: &Entity<Workspace>, pane: PaneId, cx: &mut TestAppContext) -> ModeChoice {
    workspace.read_with(cx, |ws, cx| {
        ws.agent_chat_view(pane)
            .expect("an agent chat")
            .read(cx)
            .pane_choices()
            .mode
    })
}

fn project_workspace(
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    gpui::WindowHandle<gpui_component::Root>,
    Entity<Workspace>,
) {
    let root = tempfile::tempdir().unwrap();
    let project = daruda_store::project::Project::from_path(root.path());
    let (window, workspace) = build_workspace_with(cx, &config_with(claude(None)), Some(project));
    cx.run_until_parked();
    (root, window, workspace)
}

/// `chat_new` from the control surface asks for the delegated mode on its
/// first connect; a chat the person opens from the agent menu does not.
#[gpui::test]
fn a_control_chat_starts_delegated_and_a_menu_chat_does_not(cx: &mut TestAppContext) {
    let (_root, window, workspace) = project_workspace(cx);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let (control, menu) = cx
        .update_window(window.into(), |_, window, cx| {
            workspace.update(cx, |ws, cx| {
                let control = ws
                    .control_chat_new(ws.active, ChatOpen::orchestrator_for_test(), window, cx)
                    .unwrap();
                let agent = ws.mirrors.agents[0].id.clone();
                ws.open_agent_chat_pane_with_agent(agent, window, cx);
                (control.pane, ws.active_runtime().focused_pane_id)
            })
        })
        .unwrap();
    cx.run_until_parked();
    assert_ne!(control, menu);
    assert_eq!(requested_modes(control, cx), delegated());
    assert_eq!(
        mode_of(&workspace, control, cx),
        ModeChoice::FollowDelegated
    );
    assert_eq!(requested_modes(menu, cx), ordinary());
    assert_eq!(mode_of(&workspace, menu, cx), ModeChoice::FollowDefault);
}

/// `worktree_create` opens its chat delegated even without a named agent.
#[gpui::test]
fn a_control_worktree_opens_its_chat_delegated(cx: &mut TestAppContext) {
    let fx = GitFixture::open(cx, &config_with(claude(None)), |repo| repo);
    let (window, workspace, fake) = (fx.window, &fx.ws, &fx.fake);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let pane = cx
        .update_window(window.into(), |_, window, cx| {
            workspace.update(cx, |ws, cx| {
                let project = ws.projects[0].id;
                let plan = ws.control_lane_plan(project, "delegated", None).unwrap();
                // `control_lane_finalize` registers a worktree already on disk.
                fake_lane(fake, &plan.new_path, &plan.branch);
                let (_, pane) = ws
                    .control_lane_finalize(
                        plan,
                        project,
                        ChatOpen::orchestrator_for_test(),
                        window,
                        cx,
                    )
                    .unwrap();
                pane.pane
            })
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(requested_modes(pane, cx), delegated());
}

/// A task's chat reopened in a new pane by the control surface runs
/// delegated; reopened from the Tasks page, it follows `default_mode`.
#[gpui::test]
fn a_reopened_task_chat_follows_who_reopened_it(cx: &mut TestAppContext) {
    for control in [true, false] {
        let (_root, window, workspace) = project_workspace(cx);
        let _connections = cx.update(Workspace::fake_agent_connections_for_test);
        let reopened = cx
            .update_window(window.into(), |_, window, cx| {
                workspace.update(cx, |ws, cx| {
                    let cwd = ws.active_lane().unwrap().path.clone();
                    let pane = ws.create_agent_chat_pane(
                        Some(PaneCwd::Local(cwd)),
                        Some("task-session".into()),
                        ws.mirrors.agents[0].id.clone(),
                        None,
                        window,
                        cx,
                    );
                    let pane_id = pane.id;
                    ws.active_runtime_mut().panes.push(pane);
                    let spec = crate::runs::seed::spec(ws.projects[0].uuid, "Task");
                    let id = crate::runs::seed::task(spec, None, cx);
                    ws.run_task_in_chat_for_test(&id, pane_id, cx);
                    ws.active_runtime_mut().panes.retain(|p| p.id != pane_id);
                    let reopened = if control {
                        ws.control_task_open(&id, ModeChoice::FollowDelegated, window, cx)
                            .unwrap()
                    } else {
                        ws.open_task_chat(&id, window, cx);
                        ws.active_runtime().focused_pane_id
                    };
                    assert_ne!(reopened, pane_id);
                    reopened
                })
            })
            .unwrap();
        cx.run_until_parked();
        let (modes, choice) = if control {
            (delegated(), ModeChoice::FollowDelegated)
        } else {
            (ordinary(), ModeChoice::FollowDefault)
        };
        assert_eq!(requested_modes(reopened, cx), modes, "control: {control}");
        assert_eq!(mode_of(&workspace, reopened, cx), choice);
    }
}

/// The orchestrator's own chat is the person's conversation, not a
/// delegated one.
#[gpui::test]
fn the_orchestrator_chat_follows_default_mode(cx: &mut TestAppContext) {
    let (_root, window, workspace) = project_workspace(cx);
    let _connections = cx.update(Workspace::fake_agent_connections_for_test);
    let pane = cx
        .update_window(window.into(), |_, window, cx| {
            workspace.update(cx, |ws, cx| {
                ws.seed_orchestrator_chat_pane(
                    ws.mirrors.agents[0].id.clone(),
                    std::env::temp_dir(),
                    daruda_store::accounts::AccountSelection::SystemDefault,
                    None,
                    window,
                    cx,
                )
            })
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(requested_modes(pane, cx), ordinary());
}

fn push_chat(
    ws: &mut Workspace,
    mode: ModeChoice,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> PaneId {
    let cwd = ws
        .active_lane()
        .map(|lane| PaneCwd::Local(lane.path.clone()));
    let agent = ws.mirrors.agents[0].id.clone();
    let pane = ws.create_agent_chat_pane_in(cwd, None, agent, None, mode, window, cx);
    let pane_id = pane.id;
    ws.active_runtime_mut().panes.push(pane);
    let tab_id = ws.alloc_id();
    ws.active_runtime_mut().tabs.push(TabEntry {
        id: tab_id,
        layout: PaneLayout::Pane(pane_id),
        last_focused_pane: pane_id,
        user_label: None,
    });
    pane_id
}

/// Editing `delegated_mode` moves a live delegated session to the new mode;
/// a pane whose person picked a mode, and an ordinary one, are left alone.
#[gpui::test]
fn a_delegated_mode_edit_moves_only_delegated_sessions(cx: &mut TestAppContext) {
    use daruda_acp::{ModeStateView, SessionModeView};

    let (_root, window, workspace) = project_workspace(cx);
    let (panes, mut probes) = cx
        .update_window(window.into(), |_, window, cx| {
            workspace.update(cx, |ws, cx| {
                let panes = [
                    push_chat(ws, ModeChoice::FollowDelegated, window, cx),
                    push_chat(ws, ModeChoice::FollowDelegated, window, cx),
                    push_chat(ws, ModeChoice::FollowDefault, window, cx),
                ];
                let mut probes = Vec::new();
                for id in panes {
                    ws.fold_agent_chat_event(
                        id,
                        daruda_acp::AcpEvent::Connected {
                            program: None,
                            session_id: format!("s{id}"),
                            modes: Some(ModeStateView {
                                available: ["default", "plan", "acceptEdits", "bypassPermissions"]
                                    .into_iter()
                                    .map(|id| SessionModeView {
                                        id: id.into(),
                                        name: id.into(),
                                        description: None,
                                    })
                                    .collect(),
                                current: "default".into(),
                            }),
                            config_options: Vec::new(),
                            capabilities: daruda_acp::SessionCapabilitiesView::default(),
                            login_methods: Vec::new(),
                        },
                        cx,
                    );
                    let (handle, probe) = daruda_acp::AcpSessionHandle::detached_for_test();
                    probes.push(probe);
                    ws.agent_chat_view(id)
                        .unwrap()
                        .update(cx, |v, _| v.attach_handle(handle));
                }
                ws.agent_chat_view(panes[1])
                    .unwrap()
                    .update(cx, |v, cx| v.set_mode("plan".into(), cx));
                for probe in &mut probes {
                    probe.drain();
                }
                (panes, probes)
            })
        })
        .unwrap();

    workspace.update(cx, |ws, cx| {
        ws.reload_config(&config_with(claude(Some("acceptEdits"))), cx)
    });
    cx.run_until_parked();

    let current = |pane: PaneId, cx: &mut TestAppContext| {
        workspace.read_with(cx, |ws, cx| {
            ws.agent_chat_view(pane)
                .unwrap()
                .read(cx)
                .session_config()
                .mode_for_chip()
                .map(|m| m.current.clone())
        })
    };
    assert_eq!(current(panes[0], cx).as_deref(), Some("acceptEdits"));
    assert_eq!(probes[0].drain().0, 1, "the delegated session is switched");
    assert_eq!(
        mode_of(&workspace, panes[0], cx),
        ModeChoice::FollowDelegated,
        "and keeps following, never becoming a pick"
    );
    assert_eq!(current(panes[1], cx).as_deref(), Some("plan"));
    assert_eq!(probes[1].drain().0, 0, "a picked mode stands");
    assert_eq!(current(panes[2], cx).as_deref(), Some("default"));
    assert_eq!(probes[2].drain().0, 0, "an ordinary pane never follows it");
}

/// Each mode source survives a save and restore: a restored delegated pane
/// keeps following the delegated mode after a restart.
#[gpui::test]
fn every_mode_choice_survives_a_restart(cx: &mut TestAppContext) {
    let (_root, window, workspace) = project_workspace(cx);
    let choices = [
        ModeChoice::FollowDefault,
        ModeChoice::FollowDelegated,
        ModeChoice::Picked("plan".into()),
    ];
    let (workspace_state, project_states) = cx
        .update_window(window.into(), |_, window, cx| {
            workspace.update(cx, |ws, cx| {
                for (ix, choice) in choices.iter().enumerate() {
                    let pane = push_chat(ws, choice.clone(), window, cx);
                    ws.agent_chat_view(pane)
                        .unwrap()
                        .update(cx, |v, _| v.set_session_id_for_test(Some(format!("s{ix}"))));
                }
                ws.snapshot_for_disk(cx)
            })
        })
        .unwrap();
    let config = config_with(claude(None));
    let restored = cx
        .add_window(|window, cx| {
            let mut ws = Workspace::new_with_project_for_test(
                &config,
                None,
                fresh_test_data_dir(),
                window,
                cx,
            );
            ws.restore_from_disk(&workspace_state, &project_states, window, cx);
            ws
        })
        .root(cx)
        .unwrap();
    restored.read_with(cx, |ws, cx| {
        for (ix, choice) in choices.iter().enumerate() {
            let session = format!("s{ix}");
            let view = ws
                .active_runtime()
                .panes
                .iter()
                .filter_map(|p| p.agent_chat_view())
                .find(|v| v.read(cx).session_id() == Some(session.as_str()))
                .expect("restored pane");
            assert_eq!(&view.read(cx).pane_choices().mode, choice);
        }
    });
}
