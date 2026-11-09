//! The agents a flow run may launch: every configured agent resolved for the
//! lane, and the credentials a command node must not inherit.

use std::collections::HashMap;
use std::path::Path;

use daruda_acp::LaunchSpec;
use daruda_store::project::PaneCwd;
use gpui::Context;

use super::FlowSubmitError;
use crate::agent::launch_resolve::{AgentLaunchSpec, account_recipe_for_connect, resolve_launch};
use crate::workspace::Workspace;
use crate::workspace::command::flow_picker::FlowPurpose;
use crate::workspace::main_area::pane::AccountDomain;

pub(super) struct FlowAgentCatalog {
    pub(super) launches: HashMap<String, LaunchSpec>,
    pub(super) accounts: HashMap<String, daruda_flow::session::SessionAccount>,
}

/// Every env var to unset before a command node runs: the union of what
/// each agent's account strips.
///
/// A command node inherits the environment but not the ACP account
/// credentials, and computing that list is the host's job — the
/// runner only unsets what it is given. Passing an empty vec here (which
/// is what the terminal example does, because its `LaunchSpec` strips
/// nothing) would leak an account's credentials into every shell line a
/// committed flow file names.
pub(in crate::workspace) fn union_strip_env(agents: &HashMap<String, LaunchSpec>) -> Vec<String> {
    let mut names: Vec<String> = agents
        .values()
        .flat_map(|spec| spec.strip_env.iter().cloned())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Every agent id this flow can actually launch: the ones its nodes name,
/// plus the default the repair's fix session runs as — which may be no
/// node's agent at all.
///
/// Used only to decide what the *lane* has to support. The credential
/// strip list below stays wider on purpose: a command node should not
/// inherit any configured account's credentials, whether or not this
/// particular flow launches that agent.
/// Every agent a run of this flow under `until` could open a session as.
///
/// Selection-aware, and the engine's own answer rather than a second one: the
/// catalog this feeds refuses an agent the lane cannot run, so walking the
/// whole flow blocked a run stopping at a head node because of a remote agent
/// on a tail node it never reaches.
pub(in crate::workspace) fn referenced_agents(
    loaded: &daruda_flow::LoadedFlow,
    until: Option<&daruda_flow::NodeId>,
) -> std::collections::HashSet<String> {
    daruda_flow::graph::agents_in_play(loaded.flow(), until)
        .into_iter()
        .map(str::to_owned)
        .collect()
}
impl Workspace {
    /// Every configured agent, resolved for this lane and this run's
    /// account. The whole catalog rather than only what the flow names, so
    /// a node naming an id that is not configured is reported by
    /// `validate_request` as `UnknownAgent` instead of silently resolving
    /// to something else.
    pub(super) fn flow_agent_catalog(
        &mut self,
        lane_ref: daruda_store::project::LaneRef,
        cwd: &Path,
        purpose: FlowPurpose,
        referenced: &std::collections::HashSet<String>,
        cx: &mut Context<Self>,
    ) -> Result<FlowAgentCatalog, FlowSubmitError> {
        let pane_cwd = PaneCwd::Local(cwd.to_path_buf());
        let lane = self.lane_for(lane_ref).cloned();
        let definitions: Vec<(String, AgentLaunchSpec)> = self
            .mirrors
            .agents
            .iter()
            .map(|a| (a.id.clone(), AgentLaunchSpec::of(a)))
            .collect();

        let mut catalog = HashMap::new();
        let mut accounts = HashMap::new();
        for (id, AgentLaunchSpec { launch, env }) in definitions {
            let resolved_host = match lane.as_ref().map(|lane| {
                lane.effective_session_host(
                    &launch,
                    &self.mirrors.session_hosts,
                    &self.mirrors.session_host_tombstones,
                )
            }) {
                None => None,
                Some(Ok(host)) => Some(host),
                Some(Err(unusable)) if referenced.contains(&id) => {
                    return Err(FlowSubmitError::UnusableSessionHost {
                        agent: id,
                        reason: unusable.reason,
                    });
                }
                // Not named by this flow: leaving it out changes nothing.
                Some(Err(_)) => continue,
            };
            // A flow's `cwd` means two things at once — the directory
            // `ProcessRunner` runs a gate in, and the one the ACP session
            // opens on. On a remote lane those are different machines, and
            // the engine's contract has only one field for both. Refusing
            // is honest; running the gates locally and the agents remotely
            // would be a split the user has no way to see.
            // Only for an agent this flow actually names. A catalog entry
            // that happens to be remote says nothing about a command-only
            // flow, or one that runs entirely on a local agent — refusing
            // those would be refusing on a fact the flow does not depend on.
            if referenced.contains(&id)
                && resolved_host
                    .as_ref()
                    .is_some_and(daruda_store::project::LaneSessionHost::is_remote)
            {
                return Err(FlowSubmitError::RemoteLane { agent: id });
            }
            let recipe = account_recipe_for_connect(&launch, false);
            let selection = self.default_account_selection_for_new_pane(recipe);
            let prepared = self.resolve_account(selection, AccountDomain::for_agent(recipe));
            // The same step a freshly created pane takes before its process
            // starts: Codex symlinks its `CODEX_HOME` in, Claude mirrors the
            // shared MCP servers. Skipping it would make only the first run
            // under a new account behave differently, which is the worst
            // kind of difference — but it writes to disk, and checking a
            // flow is supposed to cost nothing and change nothing.
            if let (FlowPurpose::Run, Some(prepared)) = (purpose, prepared.as_ref()) {
                self.prepare_account_dir(prepared, cx);
            }
            let cached_host = lane.as_ref().and_then(|lane| lane.session_host.clone());
            let resolved = resolve_launch(
                &launch,
                &env,
                lane.as_ref(),
                &pane_cwd,
                prepared.as_ref(),
                cached_host.as_ref(),
                resolved_host.as_ref(),
                &self.mirrors.session_hosts,
                &self.mirrors.session_host_tombstones,
            );
            // A launch shape that cannot produce a command is only fatal
            // for an agent this flow actually names — the same scope the
            // remote-lane refusal above uses. An unnamed one is left out of
            // the catalog and changes nothing.
            match resolved.spec {
                Ok(spec) => {
                    accounts.insert(
                        id.clone(),
                        daruda_flow::session::SessionAccount {
                            managed: selection.to_persisted().map(|id| id.0.to_string()),
                        },
                    );
                    catalog.insert(id, spec);
                }
                Err(reason) if referenced.contains(&id) => {
                    return Err(FlowSubmitError::AgentLaunchRefused { agent: id, reason });
                }
                Err(_) => {}
            }
        }
        Ok(FlowAgentCatalog {
            launches: catalog,
            accounts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(strip: &[&str]) -> LaunchSpec {
        LaunchSpec {
            command: "adapter".to_string(),
            strip_env: strip.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    /// What the lane has to support is what the flow can launch, and a
    /// command-only flow launches nothing. Keyed off the whole configured
    /// catalog instead, a single remote agent in the user's settings
    /// refused every flow in that lane — on a fact none of them depend on.
    #[test]
    fn only_the_agents_a_flow_can_launch_are_its_own() {
        let command_only = daruda_flow::load(
            "version: 1\nnodes:\n  - id: g\n    kind: command\n    run: \"true\"\n",
            None,
        )
        .expect("loads");
        assert!(referenced_agents(&command_only, None).is_empty());

        // The repair's fix runs as `defaults.agent`, which may be no
        // node's agent — so the default counts even when nothing names it.
        let repairing = daruda_flow::load(
            "\
version: 1
defaults:
  agent: { id: claude, mode: bypassPermissions }
nodes:
  - id: g
    kind: command
    run: \"true\"
    on_fail:
      repair:
        fix: fix it from {{attempts}}
        max_attempts: 2
",
            None,
        )
        .expect("loads");
        assert_eq!(
            referenced_agents(&repairing, None),
            std::collections::HashSet::from(["claude".to_string()])
        );
    }

    /// The catalog refuses an agent this lane cannot run, so asking it about
    /// a node the run stops before refused the run over an agent it would
    /// never launch — and the tail is exactly where an unfinished flow keeps
    /// the agent it has not settled on yet.
    #[test]
    fn a_selection_does_not_name_the_agents_beyond_it() {
        let two = daruda_flow::load(
            "\
version: 1
defaults:
  agent: { id: claude, mode: bypassPermissions }
nodes:
  - id: design
    kind: agent
    output: design.md
    prompt: write
  - id: review
    kind: agent
    deps: [design]
    agent: { id: codex, mode: auto }
    output: review.md
    prompt: write
",
            None,
        )
        .expect("loads");
        assert_eq!(
            referenced_agents(&two, None),
            std::collections::HashSet::from(["claude".to_string(), "codex".to_string()]),
            "the whole flow does launch both"
        );
        assert_eq!(
            referenced_agents(&two, Some(&daruda_flow::NodeId::from("design"))),
            std::collections::HashSet::from(["claude".to_string()]),
            "`codex` belongs to `review`, which this run stops before"
        );
    }

    /// Security property: a command node inherits the environment minus
    /// the account credentials. An empty list here is
    /// what a committed flow file would need to read them — and a name two
    /// agents share is one name, not two.
    #[test]
    fn every_accounts_credentials_are_stripped_from_a_command_node_exactly_once() {
        let agents = HashMap::from([
            ("claude".to_string(), spec(&["ANTHROPIC_API_KEY"])),
            ("personal".to_string(), spec(&["ANTHROPIC_API_KEY"])),
            ("codex".to_string(), spec(&["OPENAI_API_KEY"])),
        ]);
        assert_eq!(
            union_strip_env(&agents),
            vec!["ANTHROPIC_API_KEY", "OPENAI_API_KEY"]
        );
    }
}
