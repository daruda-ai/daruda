//! A run in flight's result description, from the facts its submission
//! records, including a resume's recorded ones.
use crate::runs::flow_description::FlowDescription;
use crate::runs::result::{Pointers, RunKey, RunSnapshot, RunState};
use daruda_flow::request::RunRequest;
use std::path::Path;

pub(super) fn for_request(request: &RunRequest) -> (RunSnapshot, Pointers) {
    split(&request.run_dir, FlowDescription::of_request(request))
}

/// A seed stands in for a submission of `source`'s file, so it is named as
/// that start would be; its directory may hold no log to say so.
#[cfg(any(test, feature = "screenshot"))]
pub(super) fn flow_metadata(dir: &Path, source: &super::FlowSource) -> (RunSnapshot, Pointers) {
    let submitted = match source {
        super::FlowSource::File(path) => Some(path.as_path()),
        super::FlowSource::Resumed { .. } => None,
    };
    let facts = crate::runs::flow_description::StartFacts {
        source: submitted,
        ..Default::default()
    };
    split(dir, FlowDescription::read(dir, facts))
}

fn split(dir: &Path, description: FlowDescription) -> (RunSnapshot, Pointers) {
    let snapshot = RunSnapshot {
        key: RunKey::flow_of(dir),
        what: description.what,
        why: description.why,
        state: RunState::Unknown,
    };
    (snapshot, description.pointers)
}
