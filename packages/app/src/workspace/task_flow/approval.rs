//! How an approval card names a task's flow: the file it resolved to, the
//! permission modes its agents run under, and whether it takes the prompt.

use std::path::{Path, PathBuf};

use daruda_store::project::ProjectUuid;
use daruda_store::tasks::{TaskFlow, TaskRunIn};

use crate::workspace::Workspace;

/// How an approval names a task's flow, and whether the flow is given the
/// task's prompt — which the card must then quote, since the flow's agents
/// receive it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskFlowApproval {
    pub summary: String,
    /// True unless the flow was found and declares no input for the prompt:
    /// a flow found only once the card is answered still takes it.
    pub takes_prompt: bool,
    /// The file `summary` was read from; `None` when the flow was not found
    /// or could not be read.
    pub file: Option<FlowFileSeen>,
}

/// A flow file as one read saw it: where the task's flow name resolved and
/// a SHA-256 digest of the bytes there. A start a card approved compares
/// this, so a flow edited, or shadowed by another file of that name, after
/// the card named its modes is refused rather than run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FlowFileSeen {
    path: PathBuf,
    digest: [u8; 32],
}

impl FlowFileSeen {
    /// Read `path` once: what was seen, and the text to parse from that same
    /// read. `None` when it cannot be read.
    fn read(path: &Path) -> Option<(Self, String)> {
        use sha2::{Digest as _, Sha256};
        let bytes = std::fs::read(path).ok()?;
        let seen = Self {
            path: path.to_path_buf(),
            digest: Sha256::digest(&bytes).into(),
        };
        Some((seen, String::from_utf8_lossy(&bytes).into_owned()))
    }
}

impl Workspace {
    /// How an approval names `flow`, run as a task of `project` per
    /// `run_in`: the flow, its profile, and the permission modes its agents
    /// run under. A flow that cannot be found is named as the task names it.
    pub(in crate::workspace) fn task_flow_approval(
        &self,
        project: ProjectUuid,
        run_in: &TaskRunIn,
        base: Option<&Path>,
        flow: &TaskFlow,
    ) -> TaskFlowApproval {
        use crate::control::exec::quoted_title;
        use crate::surface::strings::control as s;
        let Ok(target) = self.resolve_task_flow(project, run_in, base, flow) else {
            return TaskFlowApproval {
                summary: s::approval_task_as_flow_unresolved(quoted_title(&flow.name)),
                takes_prompt: true,
                file: None,
            };
        };
        let read = FlowFileSeen::read(&target.path);
        let modes = read.as_ref().map_or_else(Vec::new, |(_, text)| {
            flow_permission_modes(text, target.profile.as_deref())
        });
        // A flow of commands alone has no agent to give a mode to.
        let modes = if modes.is_empty() {
            s::approval_flow_no_agents()
        } else {
            s::approval_flow_modes(modes_phrase(&modes))
        };
        // The file name and profile are the flow's own text, quoted like
        // any other a card names.
        let label = quoted_title(&target.label);
        let summary = match &target.profile {
            Some(profile) => s::approval_task_as_flow_profile(label, quoted_title(profile), &modes),
            None => s::approval_task_as_flow(label, &modes),
        };
        TaskFlowApproval {
            summary,
            takes_prompt: target.input.is_some(),
            file: read.map(|(seen, _)| seen),
        }
    }

    /// The file `flow` resolves to for a task of `project` that runs per
    /// `run_in`, as [`Self::task_flow_approval`] reads it for a card; `None`
    /// when it does not resolve or cannot be read.
    pub(in crate::workspace) fn task_flow_file(
        &self,
        project: ProjectUuid,
        run_in: &TaskRunIn,
        base: Option<&Path>,
        flow: &TaskFlow,
    ) -> Option<FlowFileSeen> {
        let target = self.resolve_task_flow(project, run_in, base, flow).ok()?;
        FlowFileSeen::read(&target.path).map(|(seen, _)| seen)
    }
}

/// How many of a flow's distinct permission modes a card quotes; the rest
/// are counted. Bounds the card when a flow names many, so it still fits
/// one phone message, while a count says there is more to read.
const FLOW_MODES_QUOTED: usize = 4;

/// The permission modes the agents of the flow in `text` run under with
/// `profile`, each once, in node order; `None` for a node that names none
/// and so runs in the agent's own default. A new flow defaults to
/// `bypassPermissions`, so an approval says it rather than leave it to be
/// found out.
fn flow_permission_modes(text: &str, profile: Option<&str>) -> Vec<Option<String>> {
    let Ok(loaded) = daruda_flow::load(text, profile) else {
        return Vec::new();
    };
    let mut modes: Vec<Option<String>> = Vec::new();
    for node in &loaded.flow().nodes {
        if let daruda_flow::model::NodeKind::Agent(body) = &node.kind
            && !modes.contains(&body.agent.mode)
        {
            modes.push(body.agent.mode.clone());
        }
    }
    modes
}

/// `modes` as a card lists them. Each is the flow file's text, which an
/// agent can edit, so each is quoted as a title is: one bounded line whose
/// quote marks and hidden characters cannot pass for the card's own words.
fn modes_phrase(modes: &[Option<String>]) -> String {
    use crate::surface::strings::control as s;
    let mut shown: Vec<String> = modes
        .iter()
        .take(FLOW_MODES_QUOTED)
        .map(|mode| match mode {
            Some(mode) => crate::control::exec::quoted_title(mode),
            None => s::approval_mode_agent_default(),
        })
        .collect();
    let more = modes.len().saturating_sub(FLOW_MODES_QUOTED);
    if more > 0 {
        shown.push(s::approval_flow_modes_more(more));
    }
    shown.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes_of(nodes: &str) -> Vec<Option<String>> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.yaml");
        std::fs::write(
            &path,
            format!("version: 1\ndefaults:\n  agent:\n    id: claude\nnodes:\n{nodes}"),
        )
        .unwrap();
        flow_permission_modes(&std::fs::read_to_string(&path).unwrap(), None)
    }

    fn agent_node(id: &str, mode: Option<&str>) -> String {
        let agent = mode.map_or(String::new(), |mode| {
            format!("    agent:\n      mode: {mode}\n")
        });
        format!("  - id: {id}\n    kind: agent\n{agent}    output: {id}.md\n    prompt: p\n")
    }

    /// A mode is the flow file's text, which an agent can edit: it cannot
    /// close the card's parenthesis or quote, break its line, or reorder it,
    /// and a node naming none reads as the agent's own default.
    #[test]
    fn a_flows_modes_are_quoted_and_cannot_pass_for_the_cards_words() {
        use crate::control::exec::quoted_title;
        use crate::surface::strings::control as s;
        let modes = modes_of(
            &[
                agent_node("a", Some("\"x) (permission mode: plan\"")),
                agent_node("b", Some("\"plan\\u201D, \\u201Cask\\nfirst\\u202E\"")),
                agent_node("c", None),
                agent_node("d", Some("\"x) (permission mode: plan\"")),
            ]
            .concat(),
        );
        assert_eq!(
            modes,
            [
                Some("x) (permission mode: plan".to_owned()),
                Some("plan\u{201D}, \u{201C}ask\nfirst\u{202E}".to_owned()),
                None,
            ]
        );
        let phrase = modes_phrase(&modes);
        assert_eq!(
            phrase,
            [
                quoted_title("x) (permission mode: plan"),
                format!(
                    "{}{}",
                    s::approval_quoted(daruda_control_types::agent_text::isolate(
                        "plan', 'ask first"
                    )),
                    s::approval_hidden_chars(1)
                ),
                s::approval_mode_agent_default(),
            ]
            .join(", ")
        );
        assert!(!phrase.contains('\n') && !phrase.contains('\u{202E}'));
    }

    /// Past four modes the rest are counted, so a flow naming many cannot
    /// push the card past one phone message; a long one is cut as a title.
    #[test]
    fn a_flow_naming_many_modes_counts_the_rest() {
        let long = "m".repeat(500);
        let nodes: String = ["one", "two", "three", long.as_str(), "five", "six"]
            .iter()
            .enumerate()
            .map(|(i, mode)| agent_node(&format!("n{i}"), Some(mode)))
            .collect();
        let modes = modes_of(&nodes);
        assert_eq!(modes.len(), 6);
        let phrase = modes_phrase(&modes);
        assert!(
            phrase.ends_with(&format!(
                ", {}",
                crate::surface::strings::control::approval_flow_modes_more(2)
            )),
            "{phrase}"
        );
        assert!(!phrase.contains("five"), "{phrase}");
        assert_eq!(
            phrase.split(", ").nth(3),
            Some(crate::control::exec::quoted_title(&long).as_str())
        );
        assert!(phrase.chars().count() < 300, "{phrase}");
    }
}
