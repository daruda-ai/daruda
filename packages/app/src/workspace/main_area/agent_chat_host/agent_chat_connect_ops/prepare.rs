//! Resolve and apply the launch inputs for one ACP connection plan.

use super::AgentChatConnectionPlan;
use crate::agent::launch_resolve::{
    AgentLaunchSpec, ConnectCommandError, account_recipe_for_connect, resolve_launch,
};
use crate::lane::Lane;
use crate::lane::session_host::UnusableSessionHost;
use crate::surface::constants::BROWSER_MCP_SERVER_NAME;
use crate::surface::strings as s;
use crate::workspace::{Workspace, main_area::pane_tree::PaneId};
use daruda_acp::NodeProgress;
use daruda_chat::view::RuntimePrepPhase;
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::project::{LaneSessionHost, PaneCwd};
use gpui::{App, Context};
use std::path::{Path, PathBuf};

/// The banner phase for a runtime-provisioning milestone, or `None` for
/// milestones that shouldn't surface a banner (system node found, or a cache
/// probe — both instant, so the plain "Connecting…" banner already fits).
pub(super) fn runtime_prep_phase(progress: NodeProgress) -> Option<RuntimePrepPhase> {
    match progress {
        NodeProgress::UsingSystemNode | NodeProgress::CheckingCache => None,
        NodeProgress::Downloading => Some(RuntimePrepPhase::Downloading),
        NodeProgress::Verifying => Some(RuntimePrepPhase::Verifying),
        NodeProgress::Extracting => Some(RuntimePrepPhase::Extracting),
    }
}

/// The root a lane's browser session is named from, when the lane's agent
/// sessions run on this machine. Keyed by the lane, not the pane's cwd, so
/// every chat in one lane shares one browser.
fn local_lane_root(lane: Option<&Lane>, host: Option<&LaneSessionHost>) -> Option<PathBuf> {
    if host.is_some_and(LaneSessionHost::is_remote) {
        return None;
    }
    lane.map(|lane| lane.path.clone())
}

/// The launch a pane's agent gets. `repoint_to` names the agent to rewrite
/// a stale `agent_id` to, when the pane's own agent left the catalog.
struct PaneLaunch {
    spec: AgentLaunchSpec,
    repoint_to: Option<String>,
}

impl Workspace {
    /// Resolve the launch spec for `pane_id`'s agent, reconciling the view when
    /// its `agent_id` is stale (a live config reload removed/renamed it) by
    /// rewriting to the session-sticky default. `None` only when the pane is gone.
    fn resolve_pane_launch(
        &mut self,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> Option<AgentLaunchSpec> {
        let PaneLaunch { spec, repoint_to } = self.pane_launch(pane_id, cx)?;
        // Reconcile so the chip / persisted state stop lying.
        if let Some(effective_id) = repoint_to {
            self.repoint_agent_chat(pane_id, effective_id, cx);
            self.mutate_durable(cx, |_, _| {});
        }
        Some(spec)
    }

    /// The launch `pane_id`'s agent gets, without reconciling: a stale id
    /// launches the effective agent (a catalog entry, or the Claude id when
    /// the catalog is somehow empty).
    fn pane_launch(&self, pane_id: PaneId, cx: &App) -> Option<PaneLaunch> {
        let agent_id = self.agent_chat_view(pane_id)?.read(cx).agent_id();
        if let Some(spec) = self.agent_launch_for(agent_id) {
            return Some(PaneLaunch {
                spec,
                repoint_to: None,
            });
        }
        let effective_id = self.open_agent_id(None);
        let spec = self.agent_launch_for(&effective_id).unwrap_or_else(|| {
            AgentLaunchSpec::of(&daruda_config::AgentDefinition::claude_default())
        });
        Some(PaneLaunch {
            spec,
            repoint_to: Some(effective_id),
        })
    }

    #[cfg(test)]
    pub(in crate::workspace) fn resolve_pane_launch_for_test(
        &mut self,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> Option<AgentLaunchSpec> {
        self.resolve_pane_launch(pane_id, cx)
    }

    /// What a connect of `pane_id` asks its agent for. Requested on every
    /// connect, a resume (`session/load`) included: a value the adapter
    /// reports for itself never outranks these. `None` when the pane is gone.
    pub(super) fn connect_session_preferences(
        &self,
        pane_id: PaneId,
        cx: &Context<Self>,
    ) -> Option<daruda_config::SessionPreferences> {
        let view = self.agent_chat_view(pane_id)?.read(cx);
        Some(view.session_preferences(&self.mirrors.agents))
    }

    pub(super) fn prepare_agent_chat_connection(
        &mut self,
        pane_id: PaneId,
        cwd: &PaneCwd,
        cx: &mut Context<Self>,
    ) -> Option<AgentChatConnectionPlan> {
        // Resolve the pane's agent_id -> launch spec, reconciling a stale
        // agent_id. `None` only when the pane is gone.
        let AgentLaunchSpec {
            launch,
            env: agent_env,
        } = self.resolve_pane_launch(pane_id, cx)?;
        // Read back after `resolve_pane_launch` so any stale-id reconcile it did
        // is picked up: the preferences below resolve against that agent, and
        // the id keys the dev-build wire-tap file name.
        let agent_id = self
            .agent_chat_view(pane_id)?
            .read(cx)
            .agent_id()
            .to_owned();
        let daruda_config::SessionPreferences {
            model: initial_model,
            modes: initial_modes,
        } = self.connect_session_preferences(pane_id, cx)?;
        let vocabulary_source = crate::lane::session_host::adapter_command(&launch)
            .trim()
            .to_string();
        if let Some(view) = self.agent_chat_view(pane_id).cloned() {
            view.update(cx, |view, _| {
                view.set_agent_vocabulary_source(vocabulary_source)
            });
        }

        // Resolve the pane's owning lane so remote-ness is decided by the
        // lane's session host, not by whatever host (if any) `launch` itself
        // names.
        let owning_lane_ref = self.lane_ref_for_pane(pane_id);
        let (owning_lane, owning_host) = self.owning_lane_and_host(pane_id, &launch).unzip();
        let resolved_host = match owning_host {
            None => None,
            Some(Ok(host)) => Some(host),
            // The same surface a launch that cannot produce a command gets
            // below: the pane says why, and nothing connects.
            Some(Err(unusable)) => {
                if let Some(view) = self.agent_chat_view(pane_id).cloned() {
                    let message = s::agent_chat::session_host_unusable(
                        daruda_settings::labels::host_error(unusable.reason),
                    );
                    view.update(cx, |v, cx| {
                        v.set_error(message, daruda_acp::Remedy::Configure, cx);
                    });
                    self.notify_status_docks(cx);
                }
                return None;
            }
        };
        let cached_host = owning_lane.and_then(|lane| lane.session_host.clone());
        let is_remote = resolved_host
            .as_ref()
            .is_some_and(LaneSessionHost::is_remote);
        let local_root = local_lane_root(owning_lane, resolved_host.as_ref());
        if is_remote
            && self
                .main_area
                .pane(pane_id)
                .and_then(|pane| pane.agent_chat_content())
                .is_some_and(|chat| chat.restore.requires_exact())
        {
            if let Some(view) = self.agent_chat_view(pane_id).cloned() {
                view.update(cx, |view, cx| {
                    view.set_error(
                        s::flow::session_identity_missing(),
                        daruda_acp::Remedy::Configure,
                        cx,
                    )
                });
            }
            self.notify_status_docks(cx);
            return None;
        }
        let selection = self.agent_chat_account_selection(pane_id);
        let domain = crate::workspace::main_area::pane::AccountDomain::for_agent(
            account_recipe_for_connect(&launch, is_remote),
        );
        let prepared = self.resolve_account(selection, domain);
        if selection.account_id().is_some() && prepared.is_none() {
            // Not an error: the pane falls back to the system account. Log
            // only so a surprised user has ground truth for why.
            let report = ErrorReport::new(
                "Agent chat: pane account not usable for this agent; using system",
            )
            .severity(ErrorSeverity::Info)
            .with_context("agent_id", agent_id.clone())
            .at(file!(), line!())
            .dedup("agent_chat.account.unusable")
            .build();
            daruda_store::observability::log_writer::LogWriter::log(report);
        }
        let resolved = resolve_launch(
            &launch,
            &agent_env,
            owning_lane,
            cwd,
            prepared.as_ref(),
            cached_host.as_ref(),
            resolved_host.as_ref(),
            &self.mirrors.session_hosts,
            &self.mirrors.session_host_tombstones,
        );
        if let Some(lane_ref) = owning_lane_ref
            && let Some(corrected) = resolved.host_write_back
        {
            self.set_lane_session_host(lane_ref, corrected, cx);
        }
        let cwd_changed = self
            .agent_chat_view(pane_id)
            .is_some_and(|view| view.read(cx).cwd() != Some(&resolved.resolved_cwd));
        if cwd_changed {
            if let Some(view) = self.agent_chat_view(pane_id).cloned() {
                view.update(cx, |v, _| v.set_cwd(resolved.resolved_cwd.clone()));
            }
            self.update_agent_chat_cwd(pane_id, resolved.resolved_cwd.clone());
            self.mutate_durable(cx, |_, _| {});
        }

        let connect_cwd = resolved.wire_cwd;
        let launch_spec = match resolved.spec {
            Ok(spec) => spec,
            Err(err) => {
                let message = match err {
                    ConnectCommandError::NoRemotePath => s::agent_chat::no_remote_cwd(),
                    ConnectCommandError::JsonStdioRemote => {
                        s::agent_chat::json_stdio_remote_unsupported()
                    }
                    ConnectCommandError::JsonStdioEnv => {
                        s::agent_chat::json_stdio_env_unsupported()
                    }
                };
                if let Some(view) = self.agent_chat_view(pane_id).cloned() {
                    view.update(cx, |v, cx| {
                        v.set_error(message, daruda_acp::Remedy::Configure, cx);
                    });
                    // Connecting -> Error clears the badge; dirty the cached
                    // docks so it doesn't linger stale.
                    self.notify_status_docks(cx);
                }
                return None;
            }
        };

        self.capture_library_launch(
            pane_id,
            &launch,
            &agent_env,
            prepared.as_ref(),
            &resolved.resolved_cwd,
        );

        let mcp_servers = self.session_mcp_servers(
            pane_id,
            local_root.as_deref(),
            crate::orchestrator::session_tools,
            cx,
        );
        Some(AgentChatConnectionPlan {
            agent_id,
            launch_spec,
            connect_cwd,
            initial_model,
            initial_modes,
            prepared,
            mcp_servers,
        })
    }

    /// `pane_id`'s owning lane and the host its sessions run on for
    /// `launch`; `None` for a pane with no owning lane.
    fn owning_lane_and_host(
        &self,
        pane_id: PaneId,
        launch: &daruda_config::AgentLaunch,
    ) -> Option<(&Lane, Result<LaneSessionHost, UnusableSessionHost>)> {
        let lane = self.lane_for(self.lane_ref_for_pane(pane_id)?)?;
        let host = lane.effective_session_host(
            launch,
            &self.mirrors.session_hosts,
            &self.mirrors.session_host_tombstones,
        );
        Some((lane, host))
    }

    /// The MCP servers for `pane_id`: the orchestrator's pane gets only
    /// `control`; a local lane's pane gets the browser, which a remote adapter
    /// could not run; a read-only CLI snapshot gets nothing.
    fn session_mcp_servers(
        &self,
        pane_id: PaneId,
        local_root: Option<&Path>,
        control: impl FnOnce(&mut App) -> Option<daruda_acp::McpServer>,
        cx: &mut Context<Self>,
    ) -> Vec<daruda_acp::McpServer> {
        if self.loads_snapshot(pane_id, cx) {
            return Vec::new();
        }
        if self.offers_control_tools(pane_id, cx) {
            control(cx).into_iter().collect()
        } else {
            local_root
                .and_then(|root| crate::browser_tool::launch_for_lane(root, cx))
                .map(|launch| {
                    daruda_acp::stdio_mcp_server(
                        BROWSER_MCP_SERVER_NAME.to_owned(),
                        launch.command,
                        launch.args,
                        launch.env,
                    )
                })
                .into_iter()
                .collect()
        }
    }

    fn loads_snapshot(&self, pane_id: PaneId, cx: &App) -> bool {
        self.agent_chat_view(pane_id)
            .and_then(|view| view.read(cx).load_intent())
            == Some(daruda_chat::view::LoadIntent::Snapshot)
    }

    /// Whether this pane's session may reach the control MCP server.
    ///
    /// Exactly one pane may: the orchestrator's. Decided here, at the
    /// single place a session is planned, rather than by whoever opens a pane
    /// — a new pane path must not be able to hand out tools by omission.
    fn offers_control_tools(&self, pane_id: PaneId, cx: &Context<Self>) -> bool {
        let Some((_, weak)) = crate::window_registry::WindowRegistry::orchestrator(cx) else {
            return false;
        };
        // Identity by entity id, never by reading the registry's handle back:
        // this runs inside `prepare_agent_chat_connection`, which is already
        // inside `Workspace::update`, and the orchestrator seeding its own
        // pane *is* that workspace — so `weak.upgrade().read(cx)` would be a
        // second lease on the entity we are in (AGENTS.md pitfall 5).
        weak.entity_id() == cx.entity_id()
            && self.orchestrator_chat_pane().map(|p| p.pane) == Some(pane_id)
    }

    #[cfg(test)]
    pub(crate) fn offers_control_tools_for_test(
        &self,
        pane_id: PaneId,
        cx: &Context<Self>,
    ) -> bool {
        self.offers_control_tools(pane_id, cx)
    }

    /// The servers a session planned for `pane_id` would be offered, through
    /// the same decision a connect makes but without reconciling a stale agent.
    /// The control server comes from `orchestrator::mcp_server`, so asking
    /// schedules no run notices. A missing pane or an unusable lane host,
    /// which refuse the connect, are offered nothing.
    #[cfg(test)]
    pub(crate) fn mcp_servers_for_pane_for_test(
        &mut self,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> Vec<daruda_acp::McpServer> {
        let Some(PaneLaunch {
            spec: AgentLaunchSpec { launch, .. },
            ..
        }) = self.pane_launch(pane_id, cx)
        else {
            return Vec::new();
        };
        let (owning_lane, host) = self.owning_lane_and_host(pane_id, &launch).unzip();
        let host = match host {
            Some(Err(_)) => return Vec::new(),
            host => host.and_then(Result::ok),
        };
        let local_root = local_lane_root(owning_lane, host.as_ref());
        self.session_mcp_servers(
            pane_id,
            local_root.as_deref(),
            |cx| crate::orchestrator::mcp_server(cx),
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::runtime_prep_phase;

    #[test]
    fn instant_runtime_phases_do_not_replace_the_connecting_banner() {
        assert!(runtime_prep_phase(daruda_acp::NodeProgress::UsingSystemNode).is_none());
        assert!(runtime_prep_phase(daruda_acp::NodeProgress::CheckingCache).is_none());
    }
}
