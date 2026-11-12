//! Assembling a [`RunRequest`] — every question `daruda_flow` refuses to
//! answer for itself.
//!
//! The engine is handed finished values: absolute paths, a resolved agent
//! catalog, a budget with no `None` it did not choose. `examples/run_flow.rs`
//! is the same shape written for a terminal; this is it in the app's
//! vocabulary.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use daruda_flow::event::RunNotice;
use daruda_flow::request::{AskGitStatus, Budget, CostLimit, RunRequest};
use gpui::Context;

use super::Workspace;
use super::command::flow_picker::FlowPurpose;
use crate::lane::git::Git;

mod catalog;
mod run_ids;

use catalog::referenced_agents;
pub(in crate::workspace) use catalog::union_strip_env;
// Named outside only by tests and screenshot fixtures that seed run directories.
#[cfg(any(test, feature = "screenshot"))]
pub(in crate::workspace) use run_ids::run_id;
pub(in crate::workspace) use run_ids::run_started_at;

/// Why a flow could not be submitted. These are refusals, not failures —
/// nothing has been taken or written when one is returned.
#[derive(Debug)]
pub(in crate::workspace) enum FlowSubmitError {
    /// No active lane, so no working directory — an empty workspace.
    NoLane,
    /// The lane is there, but its path would not resolve — so there is no
    /// telling which tree it is, and therefore no telling where its lock
    /// lives. Its own refusal rather than [`Self::NoLane`]: the lane exists
    /// and saying it does not sends the reader looking in the wrong place.
    LaneUnresolvable {
        path: PathBuf,
    },
    /// The lane runs its agents on another machine. See the module note
    /// below on why this is refused rather than resolved.
    RemoteLane {
        agent: String,
    },
    Read {
        path: PathBuf,
        message: String,
    },
    Load(daruda_flow::FlowError),
    /// A live process — not this one — already holds the lane.
    ///
    /// Carries the lock's directory, not just the pid. Refusing a pid the
    /// OS has since handed to something else is the one refusal a user has
    /// to undo by hand, and the lock hangs under a mirrored path in the
    /// data directory that nothing on screen would otherwise name.
    LockHeld {
        pid: u32,
        lock_dir: Option<std::path::PathBuf>,
    },
    /// An agent the flow names resolved to a launch shape that cannot
    /// produce a command. Reported rather than dropped: omitting the agent
    /// surfaces as `UnknownAgent`, which points at the flow file instead of
    /// at the launch that was actually refused.
    AgentLaunchRefused {
        agent: String,
        reason: crate::agent::launch_resolve::ConnectCommandError,
    },
    /// An agent the flow names would attach to a host whose value the launch
    /// quoting cannot carry. Reported for the same reason as
    /// `AgentLaunchRefused` — and never resolved to `Local`, which would run
    /// the flow's agents on this machine instead of where the lane says.
    UnusableSessionHost {
        agent: String,
        reason: crate::lane::session_host::SessionHostError,
    },
    /// The rules that need the request's own context — a node naming an
    /// agent the catalog lacks, a `prompt_file` that is not there.
    Invalid(Vec<daruda_flow::error::ValidationIssue>),
    /// The run directory cannot be picked up — it ended on purpose, it is
    /// still going, or what it left behind is not enough to continue from.
    Resume(daruda_flow::resume::ResumeError),
}

// The run's origin travels with the run, which the process owns.
pub(in crate::workspace) use crate::runs::FlowSource;

/// Which part of a flow to spend money on this time.
///
/// A run-scoped axis beside `profile`, and threaded the same way: both are
/// answers a surface gives about *this* run that the committed file must not
/// carry. Default is the whole flow, computed fresh — which is what every
/// surface but the graph's two partial-run glyphs asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::workspace) struct FlowSelection {
    /// Run no further than this node — it and its ancestors.
    pub until: Option<daruda_flow::NodeId>,
    /// Nodes whose finished output to reuse rather than compute. Ids, not
    /// files: which past run holds one cannot be answered until the profile
    /// is known, and the profile is asked for after this is built.
    pub pinned: Vec<daruda_flow::NodeId>,
    /// Values for the inputs the flow declares. `None` until someone has
    /// given them: a desktop start then asks the person first, when the
    /// flow declares any.
    pub inputs: Option<daruda_flow::input::InputValues>,
    /// The run's id, chosen ahead by a caller that recorded it before the run
    /// started — a task finds its run by it. `None` takes a fresh one.
    pub run_id: Option<String>,
}

/// The flow a request is assembled from: a saved file, or text that would
/// live at `path` — the path anchors its relative prompt files and history.
#[derive(Clone, Copy)]
enum FlowText<'a> {
    File(&'a Path),
    Draft { text: &'a str, path: &'a Path },
}

/// Where a lane's interrupted run is read from: the lane's directory, its
/// tree, and the lock that tree implies, decided from the workspace's lanes.
///
/// Reading the run itself is disk work and goes to the background through
/// [`ResumeSite::read`]. What it reads can be stale by the time the resume is
/// submitted, which is safe: the engine reads what the run finished again
/// once it holds the lane's lock.
pub(in crate::workspace) struct ResumeSite {
    cwd: PathBuf,
    tree: daruda_flow::lock::CanonicalTree,
    lock_dir: PathBuf,
}

impl ResumeSite {
    /// Read the run in `run_dir` as a resume continues it, with
    /// [`process_is_alive`] — the predicate the request built from it uses.
    /// Blocking file reads: call it off the UI thread.
    pub(in crate::workspace) fn read(
        self,
        run_dir: &Path,
    ) -> Result<ReadResume, daruda_flow::resume::ResumeError> {
        let is_alive: fn(u32) -> bool = process_is_alive;
        let resumed = daruda_flow::resume::prepare(run_dir, Some(&self.lock_dir), &is_alive)?;
        Ok(ReadResume {
            cwd: self.cwd,
            tree: self.tree,
            resumed,
        })
    }
}

/// An interrupted run as [`ResumeSite::read`] found it, with the lane's
/// directory and tree that read resolved, so the request built from it
/// continues the run in the worktree its read named.
pub(in crate::workspace) struct ReadResume {
    pub cwd: PathBuf,
    pub tree: daruda_flow::lock::CanonicalTree,
    pub resumed: daruda_flow::resume::Resumed,
}

/// A request and the stream that narrates it. The receiver is handed back
/// separately because the sender lives inside the request, and the two are
/// created together exactly once.
pub(in crate::workspace) struct FlowSubmission {
    /// The lane's folder resolved: the key the run is held under, so every
    /// window's spelling of that folder finds it.
    pub tree: daruda_flow::lock::CanonicalTree,
    pub request: RunRequest,
    /// Which flow file this run is of, when that is knowable ([`FlowSource`]).
    pub source: FlowSource,
    pub node_install_dir: PathBuf,
    pub events: smol::channel::Receiver<RunNotice>,
    /// Where the run's permission questions arrive. A second pump rather
    /// than a `FlowEvent` variant: wiring this is what declares the app can
    /// answer, and the engine's validation reads exactly that field.
    pub asks: smol::channel::Receiver<daruda_flow::runner::PendingAsk>,
}

impl Workspace {
    /// Assemble everything one run needs and refuse anything the static
    /// rules reject. `execute` checks the same rules again, and should:
    /// the engine cannot trust a host to have done it. But it reports by
    /// *returning*, and a run that never started emits no event — so a
    /// host that only watched the stream would show the user nothing.
    pub(in crate::workspace) fn build_flow_request(
        &mut self,
        lane_ref: daruda_store::project::LaneRef,
        flow_path: &Path,
        profile: Option<&str>,
        selection: &FlowSelection,
        cx: &mut Context<Self>,
    ) -> Result<FlowSubmission, FlowSubmitError> {
        let submission = self.assemble_flow_request(
            lane_ref,
            FlowText::File(flow_path),
            profile,
            selection,
            FlowPurpose::Run,
            cx,
        )?;
        let issues = daruda_flow::request::validate_request(&submission.request);
        if issues.is_empty() {
            Ok(submission)
        } else {
            Err(FlowSubmitError::Invalid(issues))
        }
    }

    /// Every static problem this flow has, found without running it.
    ///
    /// `load` alone is not the whole of request validation: a missing `prompt_file`,
    /// a prompt file reaching for a non-ancestor's output, and an agent id
    /// the catalog lacks are only knowable with the request's own context
    /// (`validate_request`). Checking less here than `Run` checks would
    /// mean "no problems found" followed by a refusal.
    ///
    /// Takes no lock and creates no run directory: assembling a request is
    /// pure path arithmetic, and `FlowPurpose::Validate` keeps it that way
    /// by skipping the account-directory preparation a real run does.
    pub(in crate::workspace) fn check_flow(
        &mut self,
        lane_ref: daruda_store::project::LaneRef,
        flow_path: &Path,
        profile: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<Vec<daruda_flow::error::ValidationIssue>, FlowSubmitError> {
        // The whole flow, always: ✓ answers "could this file run", and a
        // question about a slice of it is not that question.
        self.check_flow_source(lane_ref, FlowText::File(flow_path), profile, cx)
    }

    /// [`Self::check_flow`] for text that is not saved: `path` is where it
    /// would live, so relative prompt files resolve as they would there.
    pub(in crate::workspace) fn check_flow_text(
        &mut self,
        lane_ref: daruda_store::project::LaneRef,
        text: &str,
        path: &Path,
        profile: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<Vec<daruda_flow::error::ValidationIssue>, FlowSubmitError> {
        self.check_flow_source(lane_ref, FlowText::Draft { text, path }, profile, cx)
    }

    fn check_flow_source(
        &mut self,
        lane_ref: daruda_store::project::LaneRef,
        source: FlowText<'_>,
        profile: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<Vec<daruda_flow::error::ValidationIssue>, FlowSubmitError> {
        let submission = self.assemble_flow_request(
            lane_ref,
            source,
            profile,
            &FlowSelection::default(),
            FlowPurpose::Validate,
            cx,
        )?;
        Ok(daruda_flow::request::check_request(&submission.request))
    }

    /// Where a run kept by `lane_ref` is read from for a resume.
    ///
    /// The tree is decided **once, here**, for the question, the lock it
    /// implies and the request built from it. Two answers can disagree, and
    /// then the panel offers a resume that the lock immediately refuses as
    /// held.
    pub(in crate::workspace) fn resume_site(
        &self,
        lane_ref: daruda_store::project::LaneRef,
    ) -> Result<ResumeSite, FlowSubmitError> {
        let (cwd, tree) = self.lane_tree(lane_ref)?;
        // Where this lane's lock lives, outside the tree. Told to look
        // anywhere else, `prepare` reads every run as `Unknown` — which is
        // to say unresumable, silently.
        let lock_dir = daruda_flow::lock::lock_dir_for(&self.lock_root, &tree);
        Ok(ResumeSite {
            cwd,
            tree,
            lock_dir,
        })
    }

    /// Whether `lane_ref` still resolves to the tree `read` was read in. The
    /// lane can be removed while its run is read off the UI thread, and a
    /// resume must not start in the folder it left behind.
    pub(in crate::workspace) fn still_resumes(
        &self,
        lane_ref: daruda_store::project::LaneRef,
        read: &ReadResume,
    ) -> Result<(), FlowSubmitError> {
        let (_, tree) = self.lane_tree(lane_ref)?;
        if tree == read.tree {
            Ok(())
        } else {
            Err(FlowSubmitError::NoLane)
        }
    }

    /// Assemble a request that **continues** the run `read` names, rather
    /// than starting a new one, with the liveness predicate the read used,
    /// [`process_is_alive`].
    pub(in crate::workspace) fn resume_request(
        &mut self,
        lane_ref: daruda_store::project::LaneRef,
        run_dir: &Path,
        read: ReadResume,
        cx: &mut Context<Self>,
    ) -> Result<FlowSubmission, FlowSubmitError> {
        let ReadResume { cwd, tree, resumed } = read;
        let is_alive: fn(u32) -> bool = process_is_alive;
        let catalog = self.flow_agent_catalog(
            lane_ref,
            &cwd,
            FlowPurpose::Run,
            &referenced_agents(&resumed.loaded, resumed.until.as_ref()),
            cx,
        )?;
        let node_install_dir = daruda_store::persistence::node_install_dir();
        let (tx, rx) = smol::channel::unbounded();
        let (ask_tx, ask_rx) = smol::channel::unbounded();

        // Only what the run fixed when it started. What it had finished is
        // read by the engine once it holds the lane's lock, since another
        // process could still be writing it before then.
        let continuation = resumed.continuation();
        let request = RunRequest {
            history: daruda_flow::session::HistoryContext {
                source: None,
                accounts: catalog.accounts,
            },
            started_by: resumed.started_by,
            until: None,
            pinned: Vec::new(),
            // The engine reads the values the run recorded at its start.
            inputs: Default::default(),
            loaded: resumed.loaded,
            run_dir: run_dir.to_path_buf(),
            // The root, not this lane's own directory: the engine names the
            // per-tree one, so the app and the engine share one derivation.
            lock_dir: self.lock_root.clone(),
            // The run's own directory: `run.yaml` inlined every file-backed
            // prompt and hint when it was written, so a resumed run resolves
            // nothing against the flow file's directory — which may not even
            // hold that flow any more.
            flow_dir: run_dir.to_path_buf(),
            cwd: cwd.clone(),
            agents: catalog.launches,
            node_install_dir: node_install_dir.clone(),
            // A fresh ceiling, measured from now. What the earlier half
            // spent is carried by the run log, not by this.
            budget: budget_from(&self.config_flow()),
            is_alive: Box::new(is_alive),
            git_status: Some(git_status(crate::git_host::GitHost::global(cx))),
            events: Some(tx),
            ask: Some(ask_tx),
            resume: Some(continuation),
            // Filled by `RunsHost::start_flow` from the run ledger.
            held_runs: Default::default(),
        };
        Ok(FlowSubmission {
            tree,
            request,
            source: FlowSource::Resumed {
                run_dir: run_dir.to_path_buf(),
            },
            node_install_dir,
            events: rx,
            asks: ask_rx,
        })
    }

    /// The lane's working directory, and the tree every lock for it is
    /// keyed off.
    ///
    /// **The one place either refusal is decided**, so starting a run and
    /// picking one up answer "this lane's folder will not resolve" the same
    /// way. The resume path had a localized refusal and the fresh one had
    /// none: the engine resolves the tree too, but a submission that was
    /// never made then reported as a run that failed on I/O, in the
    /// filesystem's words rather than daruda's.
    ///
    /// Validation refuses on it as well. A flow that would run in a folder
    /// nothing can read is not a flow with no problems, and a green ✓
    /// followed by that refusal is the worse of the two answers.
    fn lane_tree(
        &self,
        lane_ref: daruda_store::project::LaneRef,
    ) -> Result<(PathBuf, daruda_flow::lock::CanonicalTree), FlowSubmitError> {
        let Some(cwd) = self.lane_for(lane_ref).map(|lane| lane.path.clone()) else {
            return Err(FlowSubmitError::NoLane);
        };
        match daruda_flow::lock::CanonicalTree::resolve(&cwd) {
            Ok(tree) => Ok((cwd, tree)),
            Err(_) => Err(FlowSubmitError::LaneUnresolvable { path: cwd }),
        }
    }

    fn assemble_flow_request(
        &mut self,
        lane_ref: daruda_store::project::LaneRef,
        source: FlowText<'_>,
        profile: Option<&str>,
        selection: &FlowSelection,
        purpose: FlowPurpose,
        cx: &mut Context<Self>,
    ) -> Result<FlowSubmission, FlowSubmitError> {
        // The engine derives its lock from `cwd`; resolving it here is the
        // pre-flight refusal, and the key the run is held under.
        let (cwd, tree) = self.lane_tree(lane_ref)?;
        let (text, flow_path) = match source {
            FlowText::File(path) => {
                let text = std::fs::read_to_string(path).map_err(|e| FlowSubmitError::Read {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                })?;
                (std::borrow::Cow::Owned(text), path)
            }
            FlowText::Draft { text, path } => (std::borrow::Cow::Borrowed(text), path),
        };
        let sources = self.flow_sources_for(lane_ref);
        let loaded = super::flow_include::load_flow(&text, profile, sources.as_ref())
            .map_err(FlowSubmitError::Load)?;

        // Here and not at the surface that asked: which past run holds a
        // pinned output depends on the profile, and the profile is only
        // settled by the time this runs.
        let pinned = self.resolve_flow_pins(lane_ref, flow_path, profile, &selection.pinned, cx);

        let catalog = self.flow_agent_catalog(
            lane_ref,
            &cwd,
            purpose,
            &referenced_agents(&loaded, selection.until.as_ref()),
            cx,
        )?;
        let node_install_dir = daruda_store::persistence::node_install_dir();
        let (tx, rx) = smol::channel::unbounded();
        let (ask_tx, ask_rx) = smol::channel::unbounded();

        let request = RunRequest {
            history: daruda_flow::session::HistoryContext {
                source: daruda_core::path::strip_root(flow_path, &cwd),
                accounts: catalog.accounts,
            },
            started_by: Some(daruda_flow::request::StartedBy::Desktop),
            until: selection.until.clone(),
            pinned,
            inputs: selection.inputs.clone().unwrap_or_default(),
            loaded,
            run_dir: super::flow_paths::runs_dir(&cwd).join(
                selection
                    .run_id
                    .clone()
                    .unwrap_or_else(|| self.next_run_id(cx)),
            ),
            lock_dir: self.lock_root.clone(),
            flow_dir: flow_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| cwd.clone()),
            cwd: cwd.clone(),
            agents: catalog.launches,
            node_install_dir: node_install_dir.clone(),
            budget: budget_from(&self.config_flow()),
            is_alive: Box::new(process_is_alive),
            git_status: Some(git_status(crate::git_host::GitHost::global(cx))),
            events: Some(tx),
            ask: Some(ask_tx),
            // This assembles a run that is starting. Picking one up reads
            // the directory it left instead, and builds its own request.
            resume: None,
            // Filled by `RunsHost::start_flow` from the run ledger.
            held_runs: Default::default(),
        };
        Ok(FlowSubmission {
            tree,
            request,
            source: FlowSource::File(flow_path.to_path_buf()),
            node_install_dir,
            events: rx,
            asks: ask_rx,
        })
    }

    fn config_flow(&self) -> daruda_config::flow::FlowConfig {
        self.mirrors.flow_config.clone()
    }
}

/// The deadline is an `Instant` because the engine only ever compares; the
/// host is the one that knows when the run started.
fn budget_from(config: &daruda_config::flow::FlowConfig) -> Budget {
    Budget {
        deadline: config.timeout().map(|d| {
            std::time::Instant::now()
                .checked_add(d)
                .unwrap_or_else(std::time::Instant::now)
        }),
        max_node_runs: config.max_node_runs(),
        max_cost: config
            .cost_limit()
            .map(|(amount, currency)| CostLimit { amount, currency }),
    }
}

/// The engine never asks the OS whether a pid is alive; the host answers.
/// The picker asks the same question of the lock holder.
pub(in crate::workspace) fn process_is_alive(pid: u32) -> bool {
    daruda_core::process::is_alive(pid)
}

/// The working tree at a node's `cwd`, as `git status --porcelain` sees it.
///
/// Scoped to that directory with `-- .`, which is what makes the note mean
/// anything once nodes run in directories of their own: `git status` is
/// otherwise repository-wide, and every node in one repo would report the
/// same list however different their directories.
fn git_status(git: Arc<dyn Git>) -> AskGitStatus {
    Arc::new(move |cwd: &Path| git.open(cwd).status_scoped().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A user who configured nothing still runs under both enforceable
    /// ceilings — `Budget::unlimited()` reaching the engine by accident is
    /// the failure this guards.
    #[test]
    fn an_unconfigured_budget_is_not_an_unlimited_one() {
        let budget = budget_from(&daruda_config::flow::FlowConfig::default());
        assert!(budget.deadline.is_some());
        assert!(budget.max_node_runs.is_some());
        assert!(budget.max_cost.is_none(), "a cost limit needs a currency");
    }
}
