//! What a flow run is called, why, and where its results land: built one way
//! for a run in flight and for its directory read back, so an end seen live
//! and the same end read after a restart compare equal.
use super::result::{Pointers, flow_run_id, flow_why};
use daruda_flow::LoadedFlow;
use daruda_flow::NodeId;
use daruda_flow::request::RunRequest;
use std::path::{Path, PathBuf};

pub(super) struct FlowDescription {
    pub what: String,
    pub why: Option<String>,
    pub pointers: Pointers,
}

/// What a run fixed when it started, each as its run log records it. A
/// legacy journal records no source.
#[derive(Default)]
pub(super) struct StartFacts<'a> {
    pub source: Option<&'a Path>,
    pub until: Option<&'a NodeId>,
    pub pinned: Vec<&'a NodeId>,
}

impl FlowDescription {
    /// A submission, from the facts it is about to record. A continuation
    /// names no source of its own; its log holds the one its start recorded.
    pub(super) fn of_request(request: &RunRequest) -> Self {
        let dir = &request.run_dir;
        let recorded = request
            .resume
            .as_ref()
            .and_then(|_| daruda_flow::run_log::reader::state(dir));
        let facts = match &request.resume {
            None => StartFacts {
                source: request.history.source.as_deref(),
                until: request.until.as_ref(),
                pinned: request.pinned.iter().map(|pin| &pin.node).collect(),
            },
            Some(resume) => StartFacts {
                source: recorded.as_ref().and_then(|state| state.source()),
                until: request.until.as_ref().or(resume.until.as_ref()),
                pinned: resume.pinned.iter().collect(),
            },
        };
        describe(dir, Some(&request.loaded), facts)
    }

    /// A run directory read back: its `run.yaml`, under `facts` from its log.
    pub(super) fn read(dir: &Path, facts: StartFacts<'_>) -> Self {
        describe(dir, read_spec(dir).as_ref(), facts)
    }
}

/// What a flow run calls itself, read from its directory. One reading, so a
/// result and a refusal about the run name it alike.
pub(crate) fn flow_run_name(dir: &Path) -> String {
    let recorded = daruda_flow::run_log::reader::state(dir);
    name(
        read_spec(dir).as_ref(),
        recorded.as_ref().and_then(|state| state.source()),
        dir,
    )
}

/// The resolved flow a run left as `run.yaml`. Inspected rather than loaded:
/// describing a run needs its shape, not a verdict on whether it would run.
fn read_spec(dir: &Path) -> Option<LoadedFlow> {
    let text = std::fs::read_to_string(dir.join(daruda_flow::resume::RUN_SPEC_FILE)).ok()?;
    daruda_flow::inspect(&text, None)
        .ok()
        .map(|inspected| inspected.loaded)
}

fn describe(dir: &Path, flow: Option<&LoadedFlow>, facts: StartFacts<'_>) -> FlowDescription {
    FlowDescription {
        what: name(flow, facts.source, dir),
        why: flow_why(
            facts.until,
            facts.pinned.into_iter().map(ToString::to_string),
        ),
        pointers: Pointers {
            run_dir: Some(dir.to_path_buf()),
            report: Some(dir.join(daruda_flow::record::RUN_REPORT_FILE)),
            outputs: flow
                .map(|flow| final_outputs(flow, facts.until, dir))
                .unwrap_or_default(),
            ..Default::default()
        },
    }
}

/// The flow's own name, else its recorded file's stem, else the run id: the
/// source is the one the run log keeps, so a restart reads the same name.
fn name(flow: Option<&LoadedFlow>, source: Option<&Path>, dir: &Path) -> String {
    flow.and_then(LoadedFlow::name)
        .map(str::to_owned)
        .or_else(|| {
            source
                .and_then(Path::file_stem)
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| flow_run_id(dir))
}

/// The outputs of the selected agent nodes nothing selected depends on.
fn final_outputs(loaded: &LoadedFlow, until: Option<&NodeId>, dir: &Path) -> Vec<PathBuf> {
    let flow = loaded.flow();
    let selected = daruda_flow::graph::Selection::of(flow, until);
    flow.nodes
        .iter()
        .filter(|node| {
            selected.includes(&node.id)
                && !flow
                    .nodes
                    .iter()
                    .any(|other| selected.includes(&other.id) && other.deps.contains(&node.id))
        })
        .filter_map(|node| match &node.kind {
            daruda_flow::model::NodeKind::Agent(body) => Some(dir.join(&body.output)),
            daruda_flow::model::NodeKind::Command { .. } => None,
        })
        .collect()
}

#[cfg(test)]
mod tests;
