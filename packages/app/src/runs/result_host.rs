//! Foreground snapshots and background disk inspection for result queries.
use super::result::{self, RunKey, RunSnapshot, RunState, WaitingOn};
use super::{GlobalTasks, RunsHost};
use gpui::{App, BorrowAppContext as _};

impl RunsHost {
    /// The host maps pane activity to run-owned facts; only its exact live chat execution may wait.
    pub(crate) fn observe_task_waiting(key: RunKey, waiting_on: Option<WaitingOn>, cx: &mut App) {
        let RunKey::Task(exec) = &key else {
            return;
        };
        let Some(record) = cx
            .try_global::<GlobalTasks>()
            .and_then(|tasks| tasks.get(&exec.task_id))
        else {
            return;
        };
        if record.current_exec() != Some(exec)
            || !matches!(
                record.lifecycle,
                daruda_store::tasks::Lifecycle::Running { .. }
            )
            || !record.execution.as_ref().is_some_and(|run| {
                matches!(
                    run.source,
                    daruda_store::tasks::TaskExecutionSource::AgentChat
                )
            })
        {
            return;
        }
        let Some(mut snapshot) = result::task_snapshot(record, exec) else {
            return;
        };
        if let RunState::Running {
            waiting_on: current,
            ..
        } = &mut snapshot.state
        {
            *current = waiting_on;
        }
        cx.update_global::<Self, _>(|host, _| host.watch.observe_waiting(snapshot));
    }

    #[cfg(test)]
    pub(crate) fn register_run(key: RunKey, cx: &mut App) {
        Self::register_run_solicited(key, false, cx);
    }

    pub(crate) fn register_run_solicited(key: RunKey, solicited: bool, cx: &mut App) {
        Self::ensure_installed(cx);
        cx.update_global::<Self, _>(|host, _| host.watch.register_solicited(key, solicited));
    }
    /// Whether `key` is in the scope [`Self::run_scope`] lists.
    pub(crate) fn in_run_scope(key: &RunKey, cx: &App) -> bool {
        cx.try_global::<Self>()
            .is_some_and(|host| host.watch.in_scope(key))
    }
    pub(crate) fn run_scope(cx: &App) -> Vec<RunKey> {
        cx.try_global::<Self>()
            .map(|host| host.watch.scope())
            .unwrap_or_default()
    }
    pub(crate) fn latest_task_key(task: &str, cx: &App) -> Option<RunKey> {
        let record = cx.try_global::<GlobalTasks>()?.get(task)?;
        record
            .current_exec()
            .or_else(|| record.ended_executions.last().map(|end| &end.exec))
            .cloned()
            .map(RunKey::Task)
    }
    /// Stage process-owned facts before the caller performs disk reads off-thread.
    pub(crate) fn run_query(key: RunKey, cx: &mut App) -> RunQuery {
        if let Some(host) = cx.try_global::<Self>() {
            if let RunKey::Flow(id) = &key
                && let Some(run) = host.flows.by_id(id)
            {
                return RunQuery::Ready(run.result());
            }
            if let RunKey::Task(exec) = &key
                && let Some(record) = cx
                    .try_global::<GlobalTasks>()
                    .and_then(|tasks| tasks.get(&exec.task_id))
                && let Some(mut snapshot) = result::task_snapshot(record, exec)
            {
                if let RunState::Running { waiting_on, .. } = &mut snapshot.state {
                    *waiting_on = host.watch.waiting_on(&key).cloned();
                }
                return match host.idle_task_flow(record, exec, &snapshot) {
                    Some((run_dir, cwd)) => RunQuery::TaskFlow(TaskFlowQuery {
                        task: snapshot,
                        run_dir,
                        cwd,
                        lock_root: host.flow_lock_root.clone(),
                    }),
                    None => RunQuery::Ready(snapshot),
                };
            }
            if let RunKey::Flow(id) = &key
                && let Some(location) = host.watch.location(id)
            {
                return RunQuery::Disk(DiskQuery {
                    location: location.clone(),
                    fallback: host.watch.get(&key).cloned(),
                    local_end: host.watch.local_end(&key),
                });
            }
        }
        if let RunKey::Flow(id) = &key
            && let Some(dir) = Self::locate_unrecorded_flow(id, cx)
        {
            return RunQuery::Disk(DiskQuery {
                location: dir,
                fallback: None,
                local_end: None,
            });
        }
        if let Some(snapshot) = cx
            .try_global::<Self>()
            .and_then(|host| host.watch.get(&key))
        {
            return RunQuery::Ready(snapshot.clone());
        }
        let state = match &key {
            RunKey::Task(_) => RunState::Gone,
            RunKey::Flow(_) => RunState::Unknown,
        };
        RunQuery::Ready(RunSnapshot {
            key,
            what: String::new(),
            why: None,
            state,
        })
    }

    /// Where flow run `id` lives: where this process recorded it (its start,
    /// a restored ledger entry, an earlier lookup), else wherever the host
    /// finds it in an open worktree. The same lookup a result query makes.
    pub(crate) fn flow_location(id: &str, cx: &mut App) -> Option<result::FlowRunLocation> {
        cx.try_global::<Self>()
            .and_then(|host| host.watch.location(id).cloned())
            .or_else(|| Self::locate_unrecorded_flow(id, cx))
    }

    /// Ask the host's open worktrees for a run this process has no record
    /// of, remembering what it finds.
    fn locate_unrecorded_flow(id: &str, cx: &mut App) -> Option<result::FlowRunLocation> {
        let effects = cx.try_global::<Self>()?.effects.clone()?;
        let location = effects.locate_flow_run(id, cx)?;
        Self::ensure_installed(cx);
        cx.update_global::<Self, _>(|host, _| host.watch.locate(location.clone()));
        Some(location)
    }

    /// Read each owned run with no end once, off the UI thread, as a new
    /// process starts: a run that ended or was interrupted while the app was
    /// closed is reported then, not only when a wait happens to read it.
    pub(super) fn judge_restored(cx: &mut App) {
        let keys = cx.global::<Self>().watch.unjudged();
        let mut reads = Vec::new();
        for key in keys {
            let revision = Self::run_revision(&key, cx);
            match Self::run_query(key, cx) {
                // Nothing on disk to read: what the process knows is the judgment.
                RunQuery::Ready(snapshot) => {
                    cx.update_global::<Self, _>(|host, _| host.watch.judge_restored(snapshot, None))
                }
                query => reads.push((query, revision)),
            }
        }
        if reads.is_empty() {
            return;
        }
        let read = cx.background_executor().spawn(async move {
            reads
                .into_iter()
                .map(|(query, revision)| (query.read_observation(), revision))
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |cx| {
            let observations = read.await;
            cx.update(|cx| {
                for (observation, revision) in observations {
                    // Anything that moved the key meanwhile saw newer facts.
                    if Self::run_revision(&observation.snapshot.key, cx) == revision {
                        cx.update_global::<Self, _>(|host, _| {
                            host.watch
                                .judge_restored(observation.snapshot, observation.disk_episode)
                        });
                    }
                }
            });
        })
        .detach();
    }
}

impl RunsHost {
    /// The run directory and worktree of the flow a running task waits on,
    /// when no flow of this process runs it: only disk can then say whether
    /// it was interrupted. `None` for any other task or a live run.
    fn idle_task_flow(
        &self,
        record: &daruda_store::tasks::TaskRecord,
        exec: &daruda_store::tasks::ExecutionRef,
        snapshot: &RunSnapshot,
    ) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        if !matches!(snapshot.state, RunState::Running { .. })
            || record.current_exec() != Some(exec)
        {
            return None;
        }
        let run = record.execution.as_ref()?;
        let run_dir = super::flow_tasks::task_run_dir(run)?;
        let live = self.flows.by_id(&result::flow_run_id(&run_dir)).is_some();
        (!live).then(|| (run_dir, run.cwd.clone()))
    }
}

/// A running task's flow, read from disk off the UI thread.
pub(crate) struct TaskFlowQuery {
    task: RunSnapshot,
    run_dir: std::path::PathBuf,
    cwd: std::path::PathBuf,
    lock_root: Option<std::path::PathBuf>,
}

impl TaskFlowQuery {
    fn read(self) -> RunSnapshot {
        // Resolved here, not on the UI thread: canonicalizing touches disk.
        let lock_dir = self.lock_root.as_ref().and_then(|root| {
            let tree = daruda_flow::lock::CanonicalTree::resolve(&self.cwd).ok()?;
            Some(daruda_flow::lock::lock_dir_for(root, &tree))
        });
        let flow = super::result_io::flow_observation(&super::result::FlowRunLocation {
            run_dir: self.run_dir,
            lock_dir,
        });
        result::through_flow(self.task, flow.snapshot)
    }
}

pub(crate) struct DiskQuery {
    location: super::result::FlowRunLocation,
    fallback: Option<RunSnapshot>,
    local_end: Option<super::watch::LocalEnd>,
}

pub(crate) enum RunQuery {
    Ready(RunSnapshot),
    Disk(DiskQuery),
    TaskFlow(TaskFlowQuery),
}
pub(crate) struct QueryObservation {
    pub snapshot: RunSnapshot,
    pub disk_episode: Option<u64>,
}
impl RunQuery {
    pub(crate) fn read(self) -> RunSnapshot {
        self.read_observation().snapshot
    }
    pub(crate) fn read_observation(self) -> QueryObservation {
        match self {
            Self::Ready(snapshot) => QueryObservation {
                snapshot: super::result_io::enrich(snapshot),
                disk_episode: None,
            },
            // A task's own episodes follow its lifecycle, never a disk episode.
            Self::TaskFlow(query) => QueryObservation {
                snapshot: query.read(),
                disk_episode: None,
            },
            Self::Disk(query) => {
                let read = super::result_io::flow_observation(&query.location);
                if let Some(fallback) = query.fallback {
                    let local_wins = query.local_end.is_some_and(|authority| {
                        read.resumed_at
                            .is_none_or(|resumed| !authority.superseded_by(resumed))
                    });
                    if local_wins
                        || (matches!(read.snapshot.state, RunState::Gone)
                            && matches!(fallback.state, RunState::Ended { .. }))
                    {
                        return QueryObservation {
                            snapshot: super::result_io::enrich(fallback),
                            disk_episode: None,
                        };
                    }
                }
                QueryObservation {
                    snapshot: read.snapshot,
                    disk_episode: read.resumed_at,
                }
            }
        }
    }
}
#[cfg(test)]
mod tests;
