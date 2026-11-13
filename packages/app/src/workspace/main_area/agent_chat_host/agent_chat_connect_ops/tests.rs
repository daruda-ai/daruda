use super::{retries_fresh, session_resume};

#[gpui::test]
fn account_preparation_failure_records_the_selected_account_was_unavailable(
    cx: &mut gpui::TestAppContext,
) {
    use crate::runs::seed::{self, SeedEnd, SeedRun};
    use crate::runs::{GlobalTasks, RunsHost};
    use daruda_store::tasks::Lifecycle;
    use daruda_store::tasks::machine::{ErrorCause, StartFailure};
    use gpui::AppContext as _;
    let (window, workspace) = crate::workspace::tests::build_workspace(cx);
    let (pane, id) = cx
        .update_window(window.into(), |_, window, cx| {
            workspace.update(cx, |ws, cx| {
                let pane = ws.create_agent_chat_pane(
                    Some(daruda_store::project::PaneCwd::Local(std::env::temp_dir())),
                    None,
                    ws.mirrors.agents[0].id.clone(),
                    None,
                    window,
                    cx,
                );
                let pane_id = pane.id;
                ws.active_runtime_mut().panes.push(pane);
                let run = SeedRun::chat(std::env::temp_dir());
                let id = seed::task(
                    seed::spec(Default::default(), "Account"),
                    Some((&run, SeedEnd::Dispatching)),
                    cx,
                );
                RunsHost::bind_execution(ws.run_pane(pane_id), run.exec(&id), cx);
                ws.send_agent_task_prompt_text(
                    pane_id,
                    "task prompt".into(),
                    Some(run.exec(&id)),
                    cx,
                );
                (pane_id, id)
            })
        })
        .unwrap();
    super::fail_connect_account_prepare(
        &workspace.downgrade(),
        pane,
        "test preparation error".into(),
        &mut cx.to_async(),
    );
    cx.run_until_parked();
    workspace.read_with(cx, |ws, cx| {
        assert!(
            !ws.agent_chat_view(pane)
                .unwrap()
                .read(cx)
                .queue()
                .has_pending()
        )
    });
    cx.update(|cx| {
        assert!(matches!(
            cx.global::<GlobalTasks>().get(&id).unwrap().lifecycle,
            Lifecycle::Error {
                cause: ErrorCause::StartFailed(StartFailure::AccountUnavailable),
                ..
            }
        ))
    });
}

/// An exact resume asks the adapter for that session and nothing else,
/// and its load failure is never retried as a fresh conversation.
#[test]
fn an_exact_resume_never_becomes_a_fresh_session() {
    use daruda_acp::{AcpEvent, AcpFailure, SessionResume};
    let failed = AcpEvent::Error(AcpFailure::unclassified("session not found"));

    assert!(matches!(
        session_resume(Some("cli".into()), true),
        SessionResume::Required(_)
    ));
    assert!(!retries_fresh(true, true, false, &failed));

    assert!(matches!(
        session_resume(Some("chat".into()), false),
        SessionResume::BestEffort(_)
    ));
    assert!(retries_fresh(true, false, false, &failed));
    assert!(!retries_fresh(true, false, true, &failed));
    assert!(matches!(session_resume(None, true), SessionResume::Fresh));
}

mod browser {
    use std::path::PathBuf;
    use std::sync::Arc;

    use daruda_agent::browser_tool::install::{InstallError, InstallStage};
    use daruda_store::project::{LaneRef, LaneSessionHost};
    use gpui::{AppContext as _, BorrowAppContext as _, TestAppContext};

    use crate::browser_tool::{BrowserTool, Installer};
    use crate::settings_store::SettingsStore;
    use crate::surface::constants::BROWSER_MCP_SERVER_NAME;
    use crate::test_support::ControlFixture;

    /// A tool whose cache answers from memory; never downloads anything.
    struct Cached(Option<PathBuf>);

    impl Installer for Cached {
        fn cached(&self) -> Option<PathBuf> {
            self.0.clone()
        }

        fn install(
            &self,
            _on_stage: &mut dyn FnMut(InstallStage),
        ) -> Result<PathBuf, InstallError> {
            Err(InstallError::Browser("tests never install".to_owned()))
        }
    }

    /// Set `[browser] enabled` and a tool that is `Ready` when `ready`.
    fn browser_tool(enabled: bool, ready: bool, cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut config = (*SettingsStore::global(cx).user_arc()).clone();
            config.browser.enabled = enabled;
            cx.update_global::<SettingsStore, _>(|store, _| store.set_user_for_testing(config));
            let cached = ready.then(|| PathBuf::from("/opt/agent-browser/agent-browser"));
            cx.set_global(BrowserTool::supported(
                Arc::new(Cached(cached)),
                PathBuf::from("/tmp/ab"),
            ));
        });
    }

    /// Asked through `Workspace::update`, the way a connect asks.
    fn offered(
        workspace: &gpui::Entity<crate::workspace::Workspace>,
        pane: u64,
        cx: &mut TestAppContext,
    ) -> Vec<serde_json::Value> {
        workspace
            .update(cx, |ws, cx| ws.mcp_servers_for_pane_for_test(pane, cx))
            .iter()
            .map(|server| serde_json::to_value(server).expect("serialize"))
            .collect()
    }

    /// The `AGENT_BROWSER_SESSION` of the single browser server offered.
    fn browser_session(
        workspace: &gpui::Entity<crate::workspace::Workspace>,
        pane: u64,
        cx: &mut TestAppContext,
    ) -> String {
        let servers = offered(workspace, pane, cx);
        let [server] = servers.as_slice() else {
            panic!("exactly one server: {servers:?}");
        };
        assert_eq!(server["name"], BROWSER_MCP_SERVER_NAME, "{server}");
        server["env"]
            .as_array()
            .expect("env")
            .iter()
            .find(|e| e["name"] == daruda_agent::browser_tool::SESSION_ENV)
            .and_then(|e| e["value"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| panic!("the server names a session: {server}"))
    }

    fn open_chat(fixture: &ControlFixture, cx: &mut TestAppContext) -> u64 {
        cx.update_window(fixture.window.into(), |_, window, cx| {
            fixture
                .workspace
                .update(cx, |ws, cx| ws.open_agent_chat_pane_for_test(window, cx))
        })
        .expect("window is live")
    }

    /// Add a lane rooted at `root` to the active project, activate it, and
    /// open a chat in it.
    fn open_chat_in_new_lane(
        fixture: &ControlFixture,
        root: PathBuf,
        cx: &mut TestAppContext,
    ) -> u64 {
        cx.update_window(fixture.window.into(), |_, window, cx| {
            fixture.workspace.update(cx, |ws, cx| {
                let project = ws.active.project;
                let lane_id = ws.alloc_id();
                let mut lane = crate::lane::Lane::default_for_project(lane_id, root);
                lane.tab_order = 1;
                ws.project_for_mut(project)
                    .expect("active project")
                    .lanes
                    .push(lane);
                ws.activate_lane(
                    LaneRef {
                        project,
                        lane: lane_id,
                    },
                    window,
                    cx,
                );
                ws.open_agent_chat_pane_for_test(window, cx)
            })
        })
        .expect("window is live")
    }

    #[gpui::test]
    fn a_local_lane_chat_gets_the_browser_named_for_its_lane(cx: &mut TestAppContext) {
        let user = crate::test_support::workspace_with_agent_chat(cx);
        browser_tool(true, true, cx);
        let lane_root = user.workspace.read_with(cx, |ws, _| {
            ws.lane_for(ws.lane_ref_for_pane(user.pane()).expect("owning lane"))
                .expect("lane")
                .path
                .clone()
        });

        assert_eq!(
            browser_session(&user.workspace, user.pane(), cx),
            daruda_agent::browser_tool::session_name(&lane_root)
        );
    }

    #[gpui::test]
    fn chats_share_a_browser_only_within_one_lane(cx: &mut TestAppContext) {
        let user = crate::test_support::workspace_with_agent_chat(cx);
        browser_tool(true, true, cx);
        let sibling = open_chat(&user, cx);
        let other_root = user.root().join("other-lane");
        std::fs::create_dir_all(&other_root).expect("other lane root");
        let elsewhere = open_chat_in_new_lane(&user, other_root, cx);

        let first = browser_session(&user.workspace, user.pane(), cx);
        assert_eq!(
            browser_session(&user.workspace, sibling, cx),
            first,
            "two chats in one lane drive one browser"
        );
        assert_ne!(
            browser_session(&user.workspace, elsewhere, cx),
            first,
            "another lane gets its own browser"
        );
    }

    #[gpui::test]
    fn a_remote_lane_gets_no_browser(cx: &mut TestAppContext) {
        let cases = [
            (
                "ssh",
                LaneSessionHost::Ssh {
                    target: "vm".into(),
                    session_path: "/srv/app".into(),
                    registry_id: None,
                },
            ),
            (
                "docker",
                LaneSessionHost::Docker {
                    container: "box".into(),
                    session_path: "/srv/app".into(),
                    registry_id: None,
                },
            ),
        ];
        for (label, host) in cases {
            let user = crate::test_support::workspace_with_agent_chat(cx);
            browser_tool(true, true, cx);
            user.workspace.update(cx, |ws, cx| {
                let lane = ws.lane_ref_for_pane(user.pane()).expect("owning lane");
                ws.set_lane_session_host(lane, host, cx);
            });
            assert!(
                offered(&user.workspace, user.pane(), cx).is_empty(),
                "{label}: the adapter runs on another host"
            );
        }
    }

    #[gpui::test]
    fn no_browser_unless_enabled_and_ready(cx: &mut TestAppContext) {
        let cases = [
            ("disabled", false, true),
            ("enabled, not installed", true, false),
        ];
        for (label, enabled, ready) in cases {
            let user = crate::test_support::workspace_with_agent_chat(cx);
            browser_tool(enabled, ready, cx);
            assert!(
                offered(&user.workspace, user.pane(), cx).is_empty(),
                "{label}"
            );
        }
    }

    #[gpui::test]
    fn a_snapshot_chat_gets_no_servers(cx: &mut TestAppContext) {
        let user = crate::test_support::workspace_with_agent_chat(cx);
        browser_tool(true, true, cx);
        assert_eq!(
            offered(&user.workspace, user.pane(), cx).len(),
            1,
            "an interactive chat in this lane gets the browser"
        );

        user.workspace.update(cx, |ws, cx| {
            ws.agent_chat_view(user.pane())
                .expect("chat view")
                .update(cx, |view, _| {
                    view.set_access(daruda_store::tasks::AgentChatAccess::CliSnapshot(
                        daruda_store::tasks::ExecutionRef {
                            task_id: "fixture".into(),
                            execution_id: "fixture".into(),
                        },
                    ));
                });
        });

        assert!(offered(&user.workspace, user.pane(), cx).is_empty());
    }

    #[gpui::test]
    fn the_orchestrator_keeps_only_the_control_server(cx: &mut TestAppContext) {
        let orchestrator = crate::test_support::register_test_orchestrator(cx);
        cx.update(crate::orchestrator::seed_control_surface_for_test);
        browser_tool(true, true, cx);
        let (_, weak) = cx
            .update(|cx| crate::window_registry::WindowRegistry::orchestrator(cx))
            .expect("registered");
        let workspace = weak.upgrade().expect("live");

        let servers = offered(&workspace, orchestrator.pane, cx);
        let [server] = servers.as_slice() else {
            panic!("exactly the control server: {servers:?}");
        };
        assert_eq!(
            server["args"],
            serde_json::json!([crate::control::mcp::shim::SUBCOMMAND]),
            "{server}"
        );
    }
}
