//! A flow that includes another runs, and resumes, as the one expanded flow
//! its `run.yaml` records.

use super::*;
use crate::include::{IncludeSource, IncludedFlow};

/// Two steps, which the including flow places between its own two.
const STEPS: &str = "\
version: 1
nodes:
  - id: a
    kind: command
    run: \"true\"
  - id: b
    kind: command
    deps: [a]
    run: \"true\"
";

const INCLUDING: &str = "\
version: 1
nodes:
  - id: build
    kind: command
    run: \"true\"
  - id: steps
    kind: include
    flow: steps
    deps: [build]
  - id: ship
    kind: command
    deps: [steps]
    run: \"true\"
";

struct OneFlow(&'static str);

impl IncludeSource for OneFlow {
    fn find(&self, flow: &str) -> Option<IncludedFlow> {
        (flow == "steps").then(|| IncludedFlow {
            text: self.0.to_string(),
            dir: std::path::PathBuf::from("/flows"),
        })
    }
}

/// A request for [`INCLUDING`], expanded the way a host with flows to offer
/// would. `request_for` seeds the rest; it cannot load an include itself.
fn including_request(dir: &std::path::Path) -> crate::request::RunRequest {
    let mut request = request_for(STEPS, dir);
    request.loaded =
        crate::load_with(INCLUDING, None, &OneFlow(STEPS)).expect("the including flow loads");
    request
}

fn ids(runner: &FakeRunner) -> Vec<String> {
    runner
        .ids()
        .into_iter()
        .map(|id| id.into_string())
        .collect()
}

#[test]
fn an_included_flow_runs_between_the_nodes_around_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runner = FakeRunner::new();
    let report = execute(
        &including_request(dir.path()),
        &runner,
        &CancelToken::default(),
    );

    assert!(
        matches!(report.outcome, RunOutcome::Done),
        "{:?}",
        report.outcome
    );
    assert_eq!(ids(&runner), ["build", "steps--a", "steps--b", "ship"]);
    let spec = std::fs::read_to_string(report.run_dir.join(crate::resume::RUN_SPEC_FILE))
        .expect("run.yaml");
    assert!(
        !spec.contains("include"),
        "the record holds the expansion:\n{spec}"
    );
}

/// A resume reads `run.yaml` alone, so it neither needs the included flow
/// nor sees an edit made to it since the run started: there is no source
/// of flows on this path at all.
#[test]
fn an_interrupted_include_resumes_from_its_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = super::resuming::killed_running(
        including_request(dir.path()),
        "steps--b",
        FakeRunner::new(),
    );

    let runner = FakeRunner::new();
    let report = super::resuming::resume_run(&run_dir, STEPS, &runner);

    assert!(
        matches!(report.outcome, RunOutcome::Done),
        "{:?}",
        report.outcome
    );
    assert_eq!(ids(&runner), ["steps--b", "ship"]);
}
