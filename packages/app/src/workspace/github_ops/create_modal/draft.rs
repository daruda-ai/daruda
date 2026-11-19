//! The form's Write button: the lane's agent writes the title and the
//! description from what the branch takes over its base.
//!
//! Only on the button, never by itself: it is a paid call. The agent's CLI
//! runs once without tools (`crate::agent::text_run`), given the commit
//! subjects, the changed files, the patch and the template in its prompt.
//! Its answer fills the two fields and nothing else; what they held can be
//! put back. A failure is one line in the form, which stays usable.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use daruda_github::prompt::draft::{DraftFile, DraftInput, DraftText, parse_pr_draft, pr_draft};
use gpui::{Context, SharedString, Task, Window};

use super::summary::Against;
use super::{CreatePrModal, Touched};
use crate::agent::text_run::{TextRun, TextRunFailure, TextRunner};
use crate::lane::git::GitRepo;
use crate::lane::git::base::BaseProblem;
use crate::surface::strings::git as s;

/// Commit subjects handed to the agent.
const SUBJECTS: usize = 50;

pub(super) enum DraftState {
    Idle,
    Running {
        /// Held for its `Drop`, which ends the run.
        _run: Running,
    },
    Failed(SharedString),
    /// The fields hold what the agent wrote; `Before` is what they held.
    Filled(Box<Before>),
}

/// A run in flight. Dropping it (Stop, Start over, the form closing) ends
/// the agent's process, not just the wait for it.
pub(super) struct Running {
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

/// The title and the description as they were before the agent's text.
pub(super) struct Before {
    title: String,
    body: String,
    touched: Touched,
}

/// Whether the button can be pressed now, and why not otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DraftOffer {
    Ready,
    /// The lane's agent has no text-only run.
    NoAgent,
    /// The base is not one git knows, so there is nothing to compare.
    NoBase,
    /// The branch has no commit over its base.
    NothingOver,
}

/// Why a draft was not written.
enum DraftFailure {
    Cancelled,
    /// What the branch takes could not be read; what git said, when it
    /// was git that failed.
    Changes(Option<String>),
    Agent(String),
    /// The agent answered, but not with a title and a description.
    Unreadable,
}

impl CreatePrModal {
    /// The run the Write button uses; without one it says why it cannot.
    pub(in crate::workspace::github_ops) fn written_by(mut self, run: Option<TextRun>) -> Self {
        self.draft_run = run;
        self
    }

    pub(super) fn draft_offer(&self, cx: &gpui::App) -> DraftOffer {
        if self.draft_run.is_none() {
            return DraftOffer::NoAgent;
        }
        match self.current_against(cx) {
            Some(Against::Counted { commits: 0, .. }) => DraftOffer::NothingOver,
            Some(Against::Counted { .. }) => DraftOffer::Ready,
            Some(
                Against::DefaultBase
                | Against::BaseNotFound(_)
                | Against::OnBase
                | Against::Unknown,
            )
            | None => DraftOffer::NoBase,
        }
    }

    /// Ask the agent for a title and a description of what the branch
    /// takes over the base typed now.
    pub(super) fn write_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_sent() || self.draft_offer(cx) != DraftOffer::Ready {
            return;
        }
        let (Some(run), Some(base)) = (self.draft_run.clone(), self.base_value(cx)) else {
            return;
        };
        let head = self
            .publication
            .as_ref()
            .and_then(|read| read.target.head().map(str::to_owned));
        let template = self
            .template_path()
            .and_then(|path| self.templates.all().find(|t| t.path == path))
            .or(self.templates.default.as_ref())
            .map(|template| template.body.clone());
        let repo = self.repo.clone();
        let runner = crate::text_run_host::TextRunHost::global(cx);
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let written = cx
                .background_executor()
                .spawn(async move {
                    write(
                        repo.as_ref(),
                        runner.as_ref(),
                        &run,
                        &base,
                        head.as_deref(),
                        template.as_deref(),
                        &flag,
                    )
                })
                .await;
            // SILENT-OK: the form may have closed while the agent wrote.
            let _ = this.update_in(cx, |this, window, cx| this.take_draft(written, window, cx));
        });
        self.written = DraftState::Running {
            _run: Running {
                cancelled,
                _task: task,
            },
        };
        cx.notify();
    }

    fn take_draft(
        &mut self,
        written: Result<DraftText, DraftFailure>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Stopped since, or the form went on to create: nothing is filled.
        if !matches!(self.written, DraftState::Running { .. }) {
            return;
        }
        self.written = match written {
            Ok(text) => DraftState::Filled(Box::new(self.fill(text, window, cx))),
            Err(DraftFailure::Cancelled) => DraftState::Idle,
            Err(DraftFailure::Changes(None)) => DraftState::Failed(s::pr_write_no_changes().into()),
            Err(DraftFailure::Changes(Some(reason))) => {
                DraftState::Failed(s::pr_write_git_failed(reason).into())
            }
            Err(DraftFailure::Agent(reason)) => {
                DraftState::Failed(s::pr_write_failed(reason).into())
            }
            Err(DraftFailure::Unreadable) => DraftState::Failed(s::pr_write_unreadable().into()),
        };
        cx.notify();
    }

    /// Put the agent's text in the two fields, as the user's own from now
    /// on: kept for the branch, and not replaced by a template or a commit
    /// subject read later.
    fn fill(&mut self, text: DraftText, window: &mut Window, cx: &mut Context<Self>) -> Before {
        let before = Before {
            title: self.title.read(cx).value().to_string(),
            body: self.body.read(cx).value().to_string(),
            touched: self.touched,
        };
        self.set_fields(text.title, text.body, window, cx);
        (self.touched.title, self.touched.body) = (true, true);
        self.keep_creation(cx);
        before
    }

    /// Put back what the fields held before the agent's text.
    pub(super) fn undo_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let DraftState::Filled(before) = std::mem::replace(&mut self.written, DraftState::Idle)
        else {
            return;
        };
        let before = *before;
        self.set_fields(before.title, before.body, window, cx);
        self.touched = before.touched;
        self.keep_creation(cx);
        cx.notify();
    }

    fn set_fields(
        &mut self,
        title: String,
        body: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.title
            .update(cx, |input, cx| input.set_value_silently(title, window, cx));
        self.body
            .update(cx, |input, cx| input.set_value_silently(body, window, cx));
    }

    /// End a run in flight; the fields stay as they are.
    pub(super) fn stop_draft(&mut self, cx: &mut Context<Self>) {
        if matches!(self.written, DraftState::Running { .. }) {
            self.written = DraftState::Idle;
            cx.notify();
        }
    }

    /// The user typed in a field or named another base. A run in flight
    /// ends: its answer would replace what was just typed, or describe a
    /// base no longer asked about. A failure line, or the offer to put the
    /// old text back, no longer describes the fields either.
    pub(super) fn settle_draft(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.written, DraftState::Idle) {
            self.written = DraftState::Idle;
            cx.notify();
        }
    }

    /// Drop a run in flight and anything said about the last one.
    pub(super) fn reset_draft(&mut self) {
        self.written = DraftState::Idle;
    }
}

/// Read what `head` takes over `base` and have the agent describe it.
/// **Blocking**: git, then the agent's whole run.
fn write(
    repo: &dyn GitRepo,
    runner: &dyn TextRunner,
    run: &TextRun,
    base: &str,
    head: Option<&str>,
    template: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<DraftText, DraftFailure> {
    let changes = repo
        .base_tips(Some(base), head)
        .and_then(|tips| repo.changes_since(tips))
        .map_err(|problem| {
            DraftFailure::Changes(match problem {
                BaseProblem::Git(failure) => Some(failure.to_string()),
                // The base stopped comparing since the button was offered.
                _ => None,
            })
        })?;
    let (from, to) = (&changes.merge_base, &changes.tips.head);
    // One past what is quoted, to know whether there are older ones.
    let mut subjects = repo
        .range_subjects(from, to, SUBJECTS + 1)
        .map_err(|error| DraftFailure::Changes(Some(error.to_string())))?;
    let more_subjects = subjects.len() > SUBJECTS;
    subjects.truncate(SUBJECTS);
    let patch = repo
        .diff_range(from, to, &[])
        .map_err(|error| DraftFailure::Changes(Some(error.to_string())))?;
    let files: Vec<DraftFile> = changes
        .files
        .iter()
        .map(|file| DraftFile {
            path: file.path.to_string_lossy().into_owned(),
            added: file.added,
            removed: file.removed,
        })
        .collect();
    let prompt = pr_draft(&DraftInput {
        head: head.unwrap_or("HEAD"),
        base: &changes.tips.base.label,
        subjects: &subjects,
        more_subjects,
        files: &files,
        patch: &patch,
        template,
    });
    let answer = runner
        .run(run, prompt, cancelled)
        .map_err(|failure| match failure {
            TextRunFailure::Cancelled => DraftFailure::Cancelled,
            TextRunFailure::Failed(reason) => DraftFailure::Agent(reason),
        })?;
    parse_pr_draft(&answer).ok_or(DraftFailure::Unreadable)
}

#[cfg(test)]
mod tests;
