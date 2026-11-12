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
/// a SHA-256 digest of the bytes there, with every flow it includes found
/// the same way. A start a card approved compares this, so a flow — or one
/// it includes — edited, or shadowed by another file of that name, after
/// the card named its modes is refused rather than run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FlowFileSeen {
    path: PathBuf,
    digest: [u8; 32],
    included: Vec<crate::workspace::flow_include::IncludeSeen>,
}

impl FlowFileSeen {
    /// Read `path`, and expand it under `profile` from `sources`, once: what
    /// was seen, and the permission modes that same read names. `None` when
    /// the file cannot be read.
    fn read(
        path: &Path,
        profile: Option<&str>,
        sources: Option<&crate::workspace::flow_paths::FlowSources>,
    ) -> Option<(Self, Vec<Option<String>>)> {
        use sha2::{Digest as _, Sha256};
        let bytes = std::fs::read(path).ok()?;
        let text = String::from_utf8_lossy(&bytes);
        let (modes, included) = match sources {
            Some(sources) => {
                let recording = crate::workspace::flow_include::RecordingSources::new(sources);
                let modes = permission_modes(daruda_flow::load_with(&text, profile, &recording));
                (modes, recording.into_seen())
            }
            None => (
                permission_modes(daruda_flow::load(&text, profile)),
                Vec::new(),
            ),
        };
        let seen = Self {
            path: path.to_path_buf(),
            digest: Sha256::digest(&bytes).into(),
            included,
        };
        Some((seen, modes))
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
        let read = self.read_task_flow(project, run_in, base, &target);
        let modes = read
            .as_ref()
            .map_or_else(Vec::new, |(_, modes)| modes.clone());
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
        self.read_task_flow(project, run_in, base, &target)
            .map(|(seen, _)| seen)
    }

    /// The one read both of the above make, so the card and the start hash
    /// the same files: the flow's, and those it includes, found from the
    /// scopes the flow's own name was found in — where the run will look.
    fn read_task_flow(
        &self,
        project: ProjectUuid,
        run_in: &TaskRunIn,
        base: Option<&Path>,
        target: &super::TaskFlowTarget,
    ) -> Option<(FlowFileSeen, Vec<Option<String>>)> {
        let sources = self.task_flow_sources(project, run_in, base);
        FlowFileSeen::read(&target.path, target.profile.as_deref(), sources.as_ref())
    }
}

/// How many of a flow's distinct permission modes a card quotes; the rest
/// are counted. Bounds the card when a flow names many, so it still fits
/// one phone message, while a count says there is more to read.
const FLOW_MODES_QUOTED: usize = 4;

/// The permission modes the agents of a loaded flow run under, each once,
/// in node order; `None` for a node that names none and so runs in the
/// agent's own default. A new flow defaults to `bypassPermissions`, so an
/// approval says it rather than leave it to be found out.
fn permission_modes(
    loaded: Result<daruda_flow::LoadedFlow, daruda_flow::FlowError>,
) -> Vec<Option<String>> {
    let Ok(loaded) = loaded else {
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
        FlowFileSeen::read(&path, None, None).unwrap().1
    }

    /// A card names the modes of the agents a run will actually open, and
    /// an include brings its flow's agents with it. What the card saw covers
    /// the included flow too, so an edit to it alone is a different flow.
    #[test]
    fn an_included_flows_modes_are_on_the_card_and_in_what_it_saw() {
        let root = tempfile::tempdir().unwrap();
        let sources = crate::workspace::flow_paths::FlowSources {
            lane: root.path().join("lane"),
            project: None,
            global: root.path().join("global"),
        };
        std::fs::create_dir_all(&sources.global).unwrap();
        let steps = |mode: &str| {
            format!(
                "version: 1\ndefaults:\n  agent: {{ id: claude, mode: {mode} }}\nnodes:\n  - id: a\n    kind: agent\n    output: a.md\n    prompt: p\n"
            )
        };
        std::fs::write(sources.global.join("steps.yaml"), steps("plan")).unwrap();
        let outer = root.path().join("outer.yaml");
        std::fs::write(
            &outer,
            "version: 1\nnodes:\n  - id: s\n    kind: include\n    flow: steps\n",
        )
        .unwrap();

        let (seen, modes) = FlowFileSeen::read(&outer, None, Some(&sources)).unwrap();
        assert_eq!(modes, vec![Some("plan".to_string())]);
        let (_, alone) = FlowFileSeen::read(&outer, None, None).unwrap();
        assert!(
            alone.is_empty(),
            "with nowhere to look the include does not load"
        );

        std::fs::write(
            sources.global.join("steps.yaml"),
            steps("bypassPermissions"),
        )
        .unwrap();
        let (edited, _) = FlowFileSeen::read(&outer, None, Some(&sources)).unwrap();
        assert_ne!(
            edited, seen,
            "an edit to the included flow alone is noticed"
        );
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
