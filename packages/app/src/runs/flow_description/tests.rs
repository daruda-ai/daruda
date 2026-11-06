use super::*;

/// An unnamed flow that stalls: its first node removes the folder the second
/// works in, after the start checked that folder exists.
const UNNAMED_STALL: &str = "\
version: 1
nodes:
  - id: first
    kind: command
    run: \"rmdir sub\"
  - id: there
    kind: command
    deps: [first]
    cwd: sub
    run: \"true\"
";

fn request(lane: &Path, run_dir: &Path, source: Option<&str>) -> RunRequest {
    RunRequest {
        history: daruda_flow::session::HistoryContext {
            source: source.map(PathBuf::from),
            accounts: Default::default(),
        },
        started_by: Some(daruda_flow::request::StartedBy::Orchestrator),
        loaded: daruda_flow::load(UNNAMED_STALL, None).unwrap(),
        cwd: lane.to_path_buf(),
        run_dir: run_dir.to_path_buf(),
        lock_dir: lane.join("locks"),
        flow_dir: lane.to_path_buf(),
        agents: Default::default(),
        node_install_dir: lane.join("node"),
        budget: daruda_flow::request::Budget::unlimited(),
        is_alive: Box::new(|_| false),
        git_status: None,
        events: None,
        ask: None,
        until: None,
        pinned: Vec::new(),
        inputs: Default::default(),
        resume: None,
        held_runs: Default::default(),
    }
}

/// Run [`UNNAMED_STALL`] to its stall in a fresh lane; the lane and the run.
fn stalled_run(source: Option<&str>) -> (tempfile::TempDir, PathBuf) {
    let lane = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(lane.path().join("sub")).unwrap();
    let run_dir = lane.path().join("runs").join("01STALL");
    let request = request(lane.path(), &run_dir, source);
    let runners = daruda_flow::runner::Runners {
        command: daruda_flow::runner::ProcessRunner::new(Vec::new()),
        agent: daruda_flow::runner::AcpRunner::new(
            Default::default(),
            request.node_install_dir.clone(),
        ),
    };
    let report = daruda_flow::schedule::execute(
        &request,
        &runners,
        &daruda_flow::runner::CancelToken::default(),
    );
    assert!(
        matches!(
            report.outcome,
            daruda_flow::schedule::RunOutcome::Stalled { .. }
        ),
        "{report:?}"
    );
    (lane, run_dir)
}

/// The name a start describes is the one its directory reads back, for a
/// fresh submission and for the resume that continues it.
#[test]
fn an_unnamed_file_flow_is_named_alike_live_on_resume_and_from_disk() {
    let source = ".daruda/flows/ship.yaml";
    let (lane, run_dir) = stalled_run(Some(source));
    let fresh = FlowDescription::of_request(&request(lane.path(), &run_dir, Some(source)));
    assert_eq!(fresh.what, "ship");
    assert_eq!(flow_run_name(&run_dir), "ship");

    let lock_dir = daruda_flow::lock::lock_dir_for(
        &lane.path().join("locks"),
        &daruda_flow::lock::CanonicalTree::resolve(lane.path()).unwrap(),
    );
    let resumed = daruda_flow::resume::prepare(&run_dir, Some(&lock_dir), &|_| false)
        .expect("a stalled run resumes");
    let mut resume = request(lane.path(), &run_dir, None);
    resume.resume = Some(resumed.continuation());
    resume.loaded = resumed.loaded;
    let live = FlowDescription::of_request(&resume);
    assert_eq!(
        live.what, "ship",
        "a resume reads the source its start recorded"
    );
    assert_eq!(live.why, fresh.why);
    assert_eq!(live.pointers, fresh.pointers);
}

/// A start that recorded no source is named by its run id, live and on disk.
#[test]
fn an_unattributed_run_is_named_by_its_run_id() {
    let (lane, run_dir) = stalled_run(None);
    let live = FlowDescription::of_request(&request(lane.path(), &run_dir, None));
    assert_eq!(live.what, "01STALL");
    assert_eq!(flow_run_name(&run_dir), "01STALL");
}

#[test]
fn a_flows_own_name_wins_over_its_file() {
    let loaded = daruda_flow::load(
        "version: 1\nname: Ship it\nnodes:\n  - id: a\n    kind: command\n    run: \"true\"\n",
        None,
    )
    .unwrap();
    assert_eq!(
        name(
            Some(&loaded),
            Some(Path::new(".daruda/flows/ship.yaml")),
            Path::new("/runs/01A")
        ),
        "Ship it"
    );
}

#[test]
fn outputs_are_the_selections_final_agent_nodes() {
    let dir = Path::new("/runs/01A");
    let loaded = daruda_flow::inspect(
        "version: 1\nname: A flow\ndefaults:\n  agent:\n    id: claude\nnodes:\n  - id: first\n    kind: agent\n    prompt: first\n    output: first.md\n  - id: last\n    deps: [first]\n    kind: agent\n    prompt: last\n    output: last.md\n",
        None,
    )
    .unwrap()
    .loaded;
    assert_eq!(final_outputs(&loaded, None, dir), vec![dir.join("last.md")]);
    assert_eq!(
        final_outputs(&loaded, Some(&"first".into()), dir),
        vec![dir.join("first.md")]
    );
}
