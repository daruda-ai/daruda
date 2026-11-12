//! Splicing one flow into another: an `include` node is replaced, when a
//! flow is loaded, by the nodes of the flow it names. A run only ever sees
//! the expanded flow — its `run.yaml` holds no include — so the run log,
//! resume and locking need not know includes exist.
//!
//! The engine never looks for a flow itself. A host hands it an
//! [`IncludeSource`], the same way it hands over the agent catalog.

use crate::NodeId;
use crate::error::{ValidationIssue, ValidationKind};
use crate::graph::FlowGraph;
use crate::input::InputValue;
use crate::model::{AgentFail, AgentSpec, Flow, GateFail, Node, NodeKind, Prompt};
use crate::parse::{FlowFile, NodeKindFile};
use crate::template::{InputBinding, prefix_output_refs, rewrite_input_refs};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// How deep includes may nest. Three is more than any flow written so far
/// has wanted, and a bound turns a runaway chain into a refusal.
pub const MAX_DEPTH: usize = 3;

/// Between an include's id and its member's. Ids are `[A-Za-z0-9_-]`, so the
/// separator has to be made of those; a doubled hyphen is rare in a name.
const ID_SEPARATOR: &str = "--";

/// Where a host finds a flow by the name an `include` gives.
pub trait IncludeSource {
    fn find(&self, flow: &str) -> Option<IncludedFlow>;
}

/// A flow a host found: its text, and the directory its `prompt_file`s
/// are relative to.
#[derive(Debug, Clone)]
pub struct IncludedFlow {
    pub text: String,
    pub dir: PathBuf,
}

/// One include, as it was expanded. What a host needs to draw the members
/// as the single node the file wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludeSite {
    /// The include node's own id.
    pub id: NodeId,
    pub flow: String,
    pub profile: Option<String>,
    /// The expanded nodes, in the included flow's order.
    pub members: Vec<NodeId>,
    /// Members nothing else in the included flow depends on: what a node
    /// depending on the include waits for.
    pub sinks: Vec<NodeId>,
}

/// `<include>--<node>`.
pub fn member_id(include: &NodeId, node: &NodeId) -> NodeId {
    NodeId::from(format!("{include}{ID_SEPARATOR}{node}"))
}

/// The flows being expanded, outermost first. One value, so a cycle and a
/// depth overrun are judged together.
#[derive(Debug, Clone, Default)]
pub(crate) struct IncludeChain(Vec<String>);

impl IncludeChain {
    pub(crate) fn enter(&self, flow: &str) -> Result<IncludeChain, ValidationKind> {
        if self.0.iter().any(|name| name == flow) {
            return Err(ValidationKind::IncludeCycle {
                flow: flow.to_string(),
            });
        }
        if self.0.len() >= MAX_DEPTH {
            return Err(ValidationKind::IncludeTooDeep {
                flow: flow.to_string(),
            });
        }
        let mut names = self.0.clone();
        names.push(flow.to_string());
        Ok(IncludeChain(names))
    }
}

/// An include node taken out of the file, before anything is resolved.
struct IncludeNode {
    id: NodeId,
    deps: Vec<NodeId>,
    flow: String,
    profile: Option<String>,
    inputs: BTreeMap<String, InputValue>,
}

/// Where each node of the file stands, so the expansion keeps its order.
enum Slot {
    Own,
    Include(IncludeNode),
}

/// One include, expanded.
struct Expansion {
    site: IncludeSite,
    nodes: Vec<Node>,
    /// The included flow's repair agent, when one of its gates repairs.
    repair_agent: Option<Option<AgentSpec>>,
}

/// Resolve `file` under `profile` and splice every include in. The flow
/// returned is what a run executes and what its `run.yaml` records.
pub(crate) fn expand(
    mut file: FlowFile,
    profile: Option<&str>,
    source: &dyn IncludeSource,
    chain: &IncludeChain,
) -> Result<(Flow, Vec<IncludeSite>), Vec<ValidationIssue>> {
    let explicit_agent = crate::resolve::explicit_default_agent(&file, profile);
    let mut slots = Vec::with_capacity(file.nodes.len());
    file.nodes.retain_mut(|node| match &mut node.kind {
        NodeKindFile::Include {
            flow,
            profile,
            inputs,
        } => {
            slots.push(Slot::Include(IncludeNode {
                id: node.id.clone(),
                deps: std::mem::take(&mut node.deps),
                flow: std::mem::take(flow),
                profile: profile.take(),
                inputs: std::mem::take(inputs),
            }));
            false
        }
        _ => {
            slots.push(Slot::Own);
            true
        }
    });
    if !slots.iter().any(|slot| matches!(slot, Slot::Include(_))) {
        return crate::resolve::resolve(file, profile).map(|flow| (flow, Vec::new()));
    }

    // Taken out before the graph is built, so the graph's own duplicate check
    // never sees an include's id; and every node depending on one is rewired
    // to its sinks, so a second node by that name would lose its dependents.
    let mut issues = duplicate_include_ids(&slots, &file);
    let mut flow = crate::resolve::resolve(file, profile)?;
    let mut own = std::mem::take(&mut flow.nodes).into_iter();
    let mut nodes = Vec::new();
    let mut sites = Vec::new();
    let mut repair_agents = Vec::new();
    for slot in slots {
        match slot {
            Slot::Own => nodes.extend(own.next()),
            Slot::Include(include) => match expand_one(&include, &flow.inputs, source, chain) {
                Ok(expansion) => {
                    nodes.extend(expansion.nodes);
                    repair_agents.push((include.id.clone(), expansion.repair_agent));
                    sites.push(expansion.site);
                }
                Err(found) => issues.extend(found),
            },
        }
    }
    // A member named like another include would be rewired as that include.
    let include_ids: HashSet<&NodeId> = sites.iter().map(|s| &s.id).collect();
    for node in nodes.iter().filter(|n| include_ids.contains(&n.id)) {
        issues.push(duplicate_id(&node.id));
    }
    if !issues.is_empty() {
        return Err(issues);
    }

    // A node depending on an include waits for everything it spliced in
    // that nothing else there waits on.
    let sinks: HashMap<&NodeId, &[NodeId]> =
        sites.iter().map(|s| (&s.id, s.sinks.as_slice())).collect();
    for node in &mut nodes {
        let mut deps = Vec::with_capacity(node.deps.len());
        for dep in node.deps.drain(..) {
            match sinks.get(&dep) {
                Some(members) => deps.extend(members.iter().cloned()),
                None => deps.push(dep),
            }
        }
        let mut seen = HashSet::new();
        deps.retain(|dep| seen.insert(dep.clone()));
        node.deps = deps;
    }

    // Decided over every node, the way `resolve` decides it over a file's:
    // a resume recomputes it from `run.yaml`, where all of them stand.
    flow.default_agent =
        explicit_agent.or_else(|| crate::resolve::unanimous_agent_from_nodes(&nodes));
    flow.nodes = nodes;
    let issues = agent_issues(&flow, &sites, &repair_agents);
    if issues.is_empty() {
        Ok((flow, sites))
    } else {
        Err(issues)
    }
}

fn duplicate_id(id: &NodeId) -> ValidationIssue {
    ValidationIssue {
        node: Some(id.clone()),
        kind: ValidationKind::DuplicateId,
        message: format!("`{id}` names more than one node"),
    }
}

/// An include's id taken by another include or by one of the file's own nodes.
fn duplicate_include_ids(slots: &[Slot], file: &FlowFile) -> Vec<ValidationIssue> {
    let mut seen: HashSet<&NodeId> = file.nodes.iter().map(|n| &n.id).collect();
    slots
        .iter()
        .filter_map(|slot| match slot {
            Slot::Include(include) => Some(&include.id),
            Slot::Own => None,
        })
        .filter(|id| !seen.insert(id))
        .map(duplicate_id)
        .collect()
}

/// The included flow, as every member of it is rewritten against.
struct Included<'a> {
    include: &'a IncludeNode,
    flow: &'a Flow,
    graph: &'a FlowGraph,
    dir: &'a Path,
    ids: HashSet<NodeId>,
    bindings: BTreeMap<String, InputBinding>,
}

fn expand_one(
    include: &IncludeNode,
    outer_inputs: &[crate::input::InputDecl],
    source: &dyn IncludeSource,
    chain: &IncludeChain,
) -> Result<Expansion, Vec<ValidationIssue>> {
    let refuse = |kind: ValidationKind, message: String| {
        vec![ValidationIssue {
            node: Some(include.id.clone()),
            kind,
            message,
        }]
    };
    let flow_name = include.flow.as_str();
    let inner = chain
        .enter(flow_name)
        .map_err(|kind| refuse(kind, format!("cannot include `{flow_name}` here")))?;
    let found = source.find(flow_name).ok_or_else(|| {
        refuse(
            ValidationKind::UnknownInclude {
                flow: flow_name.to_string(),
            },
            format!("no flow named `{flow_name}`"),
        )
    })?;
    let invalid = |detail: String| {
        refuse(
            ValidationKind::IncludeInvalid {
                flow: flow_name.to_string(),
            },
            format!("`{flow_name}` does not load on its own: {detail}"),
        )
    };
    let included = match crate::load::inspect_in(
        &found.text,
        include.profile.as_deref(),
        Some(source),
        &inner,
    ) {
        Ok(inspected) if inspected.issues.is_empty() => inspected.loaded,
        Ok(inspected) => return Err(invalid(messages(&inspected.issues))),
        Err(crate::FlowError::Validate(issues)) => {
            // A cycle or a chain too deep is found one level down; it is about
            // this include, and a host words it as itself, not as "invalid".
            if let Some(chained) = issues.iter().find(|issue| {
                matches!(
                    issue.kind,
                    ValidationKind::IncludeCycle { .. } | ValidationKind::IncludeTooDeep { .. }
                )
            }) {
                return Err(refuse(chained.kind.clone(), chained.message.clone()));
            }
            return Err(invalid(messages(&issues)));
        }
        Err(e) => return Err(invalid(e.to_string())),
    };
    let sub = included.flow();
    // Nothing to wait for: a node depending on it would lose its order.
    if sub.nodes.is_empty() {
        return Err(invalid("it has no nodes".to_string()));
    }
    let context = Included {
        include,
        flow: sub,
        graph: included.graph(),
        dir: &found.dir,
        ids: sub.nodes.iter().map(|n| n.id.clone()).collect(),
        bindings: bind_inputs(include, &sub.inputs, outer_inputs)?,
    };

    let mut nodes = Vec::with_capacity(sub.nodes.len());
    let mut issues = Vec::new();
    for node in &sub.nodes {
        match member(&context, node) {
            Ok(member) => nodes.push(member),
            Err(issue) => issues.push(issue),
        }
    }
    if !issues.is_empty() {
        return Err(issues);
    }

    let depended_on: HashSet<&NodeId> = sub.nodes.iter().flat_map(|n| &n.deps).collect();
    let sinks = sub
        .nodes
        .iter()
        .filter(|n| !depended_on.contains(&n.id))
        .map(|n| member_id(&include.id, &n.id))
        .collect();
    let repairs = sub.nodes.iter().any(|n| {
        matches!(
            n.kind,
            NodeKind::Command {
                on_fail: GateFail::Repair { .. },
                ..
            }
        )
    });
    Ok(Expansion {
        site: IncludeSite {
            id: include.id.clone(),
            flow: include.flow.clone(),
            profile: include.profile.clone(),
            members: nodes.iter().map(|n: &Node| n.id.clone()).collect(),
            sinks,
        },
        nodes,
        repair_agent: repairs.then(|| sub.default_agent.clone()),
    })
}

/// What each of the included flow's inputs becomes: the include's token, its
/// literal checked against the declaration, or the declaration's default.
fn bind_inputs(
    include: &IncludeNode,
    decls: &[crate::input::InputDecl],
    outer_inputs: &[crate::input::InputDecl],
) -> Result<BTreeMap<String, InputBinding>, Vec<ValidationIssue>> {
    let mut issues = Vec::new();
    let mut mismatch = |name: &str| {
        issues.push(ValidationIssue {
            node: Some(include.id.clone()),
            kind: ValidationKind::IncludeInputMismatch {
                flow: include.flow.clone(),
                name: name.to_string(),
            },
            message: format!("input `{name}` does not fit `{}`", include.flow),
        });
    };
    let mut bindings = BTreeMap::new();
    let mut literals = crate::input::InputValues::new();
    // Refused already: kept out of the check below, which would otherwise
    // report a required one a second time as missing.
    let mut refused = HashSet::new();
    for (name, value) in &include.inputs {
        if !decls.iter().any(|decl| &decl.name == name) {
            mismatch(name);
            continue;
        }
        match InputBinding::of(value) {
            // An input token names an input of the including flow; one it does
            // not declare would go unchecked if the included flow never used it.
            Some(InputBinding::Token(token))
                if token
                    .strip_prefix("input.")
                    .is_some_and(|outer| !outer_inputs.iter().any(|d| d.name == outer)) =>
            {
                mismatch(name);
                refused.insert(name.clone());
            }
            Some(InputBinding::Token(token)) => {
                bindings.insert(name.clone(), InputBinding::Token(token));
            }
            Some(InputBinding::Literal(_)) => {
                literals.insert(name.clone(), value.clone());
            }
            None => {
                mismatch(name);
                refused.insert(name.clone());
            }
        }
    }
    // A token's value is only known when the run starts, so only literals —
    // and the defaults of what the include leaves out — meet the types here.
    let unbound: Vec<_> = decls
        .iter()
        .filter(|decl| !bindings.contains_key(&decl.name) && !refused.contains(&decl.name))
        .cloned()
        .collect();
    match crate::input::resolve(&unbound, &literals) {
        Ok(values) => {
            for (name, value) in values {
                let text = value.as_text();
                // A value is never rendered again on its own, but spliced into
                // a template it would be: a default reading `{{…}}` would turn
                // into a live token, unquoted inside a shell literal's quotes.
                if text.contains("{{") {
                    mismatch(&name);
                    continue;
                }
                bindings.insert(name, InputBinding::Literal(text));
            }
        }
        Err(found) => {
            for issue in found {
                let name = match &issue.kind {
                    ValidationKind::InputMissing { name }
                    | ValidationKind::InputValueInvalid { name }
                    | ValidationKind::InputNotDeclared { name } => name.as_str(),
                    _ => "",
                };
                mismatch(name);
            }
        }
    }
    if issues.is_empty() {
        Ok(bindings)
    } else {
        Err(issues)
    }
}

/// One included node as it stands in the including flow.
fn member(context: &Included<'_>, node: &Node) -> Result<Node, ValidationIssue> {
    let include = context.include;
    let mut member = node.clone();
    member.id = member_id(&include.id, &node.id);
    member.deps = if node.deps.is_empty() {
        include.deps.clone()
    } else {
        node.deps
            .iter()
            .map(|d| member_id(&include.id, d))
            .collect()
    };
    match &mut member.kind {
        NodeKind::Agent(body) => {
            // Taken apart field by field: a field added to `AgentBody` fails to
            // compile here, which is what makes someone decide whether it holds
            // a path or a template that has to move with the node.
            let crate::model::AgentBody {
                agent: _,
                prompt,
                output,
                output_schema: _,
                continue_until: _,
                max_turns: _,
                on_fail,
            } = &mut **body;
            *output = Path::new(include.id.as_str()).join(&*output);
            inline(context, &node.id, prompt, "prompt_file")?;
            if let AgentFail::Retry { hint, .. } = on_fail {
                inline(context, &node.id, hint, "hint_file")?;
            }
        }
        NodeKind::Command { on_fail, .. } => {
            if let GateFail::Repair { rerun, .. } = on_fail {
                for root in rerun.iter_mut() {
                    *root = member_id(&include.id, root);
                }
            }
        }
    }
    // Ids first, so an including flow's `{{node.x.output}}` a binding brings
    // in is never mistaken for one of the included flow's own.
    for (field, text) in member.templates_mut() {
        let moved = prefix_output_refs(text, &context.ids, &include.id);
        *text = rewrite_input_refs(&moved, &context.bindings, field.surface());
    }
    Ok(member)
}

/// Read a file-backed prompt into the node, from the included flow's own
/// folder and under the same rule a run holds its own files to. What it
/// references is checked against the included flow — the check a run makes
/// of its own prompt files — because once inlined, an input it does not
/// declare or a node it does not have would quietly resolve against the
/// including flow's of that name.
fn inline(
    context: &Included<'_>,
    node: &NodeId,
    prompt: &mut Prompt,
    field: &'static str,
) -> Result<(), ValidationIssue> {
    let include = context.include;
    let Prompt::File(relative) = prompt else {
        return Ok(());
    };
    let text = crate::path_rules::read_under(context.dir, relative)
        .map_err(|problem| crate::request::prompt_file_issue(problem, field, &include.id))?;
    let ancestors: HashSet<NodeId> = context.graph.ancestors(node).into_iter().collect();
    let mut found = Vec::new();
    crate::validate::check_output_refs(&text, node, &ancestors, &mut found);
    crate::input::check_refs(node, &[text.as_str()], &context.flow.inputs, &mut found);
    if !found.is_empty() {
        return Err(ValidationIssue {
            node: Some(include.id.clone()),
            kind: ValidationKind::IncludeInvalid {
                flow: include.flow.clone(),
            },
            message: format!(
                "`{}` in `{}`: {}",
                relative.display(),
                include.flow,
                messages(&found)
            ),
        });
    }
    *prompt = Prompt::Inline(text);
    Ok(())
}

/// Refuse an expansion that would run a different agent once read back from
/// `run.yaml`. There, a node's empty axis is filled from the flow's
/// `defaults` — the including flow's, not the one the node was resolved
/// under — and a repair's fix runs as the including flow's default agent.
fn agent_issues(
    flow: &Flow,
    sites: &[IncludeSite],
    repair_agents: &[(NodeId, Option<Option<AgentSpec>>)],
) -> Vec<ValidationIssue> {
    let merged = flow.default_agent.as_ref();
    let drifts = |spec: &AgentSpec| {
        let filled = |axis: &Option<String>, from: Option<&Option<String>>| {
            axis.is_none() && from.is_some_and(Option::is_some)
        };
        filled(&spec.model, merged.map(|m| &m.model))
            || filled(&spec.effort, merged.map(|m| &m.effort))
            || filled(&spec.mode, merged.map(|m| &m.mode))
            // Without a mode the id is not written on the node either.
            || (spec.mode.is_none() && merged.is_none_or(|m| m.id != spec.id))
    };
    let mut issues = Vec::new();
    for site in sites {
        let members: HashSet<&NodeId> = site.members.iter().collect();
        let node_drifts = flow.nodes.iter().any(|node| {
            members.contains(&node.id) && node.kind.agent_body().is_some_and(|b| drifts(&b.agent))
        });
        let repair_drifts = repair_agents
            .iter()
            .find(|(id, _)| id == &site.id)
            .and_then(|(_, agent)| agent.as_ref())
            .is_some_and(|agent| agent.as_ref() != merged);
        if node_drifts || repair_drifts {
            issues.push(ValidationIssue {
                node: Some(site.id.clone()),
                kind: ValidationKind::IncludeAgentMismatch {
                    flow: site.flow.clone(),
                },
                message: format!(
                    "`{}`'s agents would change on resume under this flow's defaults",
                    site.flow
                ),
            });
        }
    }
    issues
}

fn messages(issues: &[ValidationIssue]) -> String {
    issues
        .iter()
        .map(|issue| issue.message.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests;
