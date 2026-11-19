//! Putting a pull request in the stack its form asked for: right after
//! it is created, again from the form when that failed, and what is owed
//! when the branch already had one.

use daruda_github::RepoId;
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use gpui::{AnyWindowHandle, Context, WeakEntity};

use super::{CreatePrError, Unstacked, close_modal};
use crate::gh_host::GhHost;
use crate::github::PrPoller;
use crate::surface::strings::error as s;
use crate::workspace::Workspace;
use crate::workspace::github_ops::create_modal::CreatePrModal;
use crate::workspace::github_ops::lane_pr::CreatedPr;

impl Workspace {
    /// Put pull request `number`, created but left out of its stack, on
    /// `parent` again. The stack is read first: an earlier attempt whose
    /// answer was lost may have gone through. The outcome is told here,
    /// not by the form, so one closed meanwhile loses neither the failure
    /// nor the poll that shows the stack.
    pub(in crate::workspace) fn stack_pr_again(
        &mut self,
        repo: RepoId,
        parent: u64,
        number: u64,
        modal: WeakEntity<CreatePrModal>,
        window: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        let gh = GhHost::global(cx);
        let asked = repo.clone();
        cx.spawn(async move |this, cx| {
            let stacked = cx
                .background_executor()
                .spawn(async move {
                    let read = gh.pr_stack(&asked, parent).map_err(|e| e.to_string())?;
                    if in_stack(&read, number) {
                        return Ok(());
                    }
                    let place = read
                        .place_on(parent)
                        .map_err(|why| CreatePrError::NoStacking(parent, why).form_message())?;
                    gh.stack_pr(&asked, place, number)
                        .map_err(|e| e.to_string())
                })
                .await;
            // With the workspace's window closed nobody is left to tell,
            // and the next poll in any window reads the stack.
            // SILENT-OK: the workspace is gone; there is no one to tell.
            let _ = this.update(cx, |ws, cx| match stacked {
                Ok(()) => {
                    PrPoller::nudge(repo, cx);
                    close_modal(modal, window, cx);
                }
                Err(reason) => {
                    let report = ErrorReport::new(s::github_pr_stack_failed())
                        .severity(ErrorSeverity::Error)
                        .message(reason.clone())
                        .at(file!(), line!())
                        .dedup("github.stack_pr")
                        .build();
                    ws.report_error(report, cx);
                    cx.defer(move |cx| {
                        if let Some(modal) = modal.upgrade() {
                            modal.update(cx, |modal, cx| modal.stack_failed(reason, cx));
                        }
                    });
                }
            });
        })
        .detach();
    }
}

/// Whether `read`, a stack, already lists pull request `number`.
fn in_stack(read: &daruda_github::StackRead, number: u64) -> bool {
    match read {
        daruda_github::StackRead::Stacked(stack) => {
            stack.entries.iter().any(|entry| entry.number == number)
        }
        daruda_github::StackRead::NotStacked | daruda_github::StackRead::Unsupported => false,
    }
}

/// What is owed when the branch already had pull request `existing` and
/// the form asked to stack on `stack_on`: `None` when nothing was asked or
/// it is in that stack already. Nothing is stacked here: the pull request
/// was not made by this form, so the user is told and decides.
pub(super) fn existing_unstacked(
    gh: &dyn daruda_github::Gh,
    repo: &RepoId,
    stack_on: Option<u64>,
    existing: Option<u64>,
) -> Option<Unstacked> {
    let parent = stack_on?;
    let reason = match existing {
        None => s::github_pr_stack_no_number(),
        Some(number) => match gh.pr_stack(repo, parent) {
            Ok(read) if in_stack(&read, number) => return None,
            Ok(_) => s::github_pr_stack_existing(),
            Err(error) => error.to_string(),
        },
    };
    Some(Unstacked {
        repo: repo.clone(),
        parent,
        number: existing,
        reason,
    })
}

/// Put the pull request just created at `url` on `parent`. `None` when it
/// is stacked; otherwise what is left to do and why.
pub(super) fn stack_created(
    gh: &dyn daruda_github::Gh,
    repo: &RepoId,
    parent: u64,
    place: daruda_github::StackPlace,
    url: &str,
    head: &str,
) -> Option<Unstacked> {
    let number = CreatedPr::of(url.to_owned(), head).and_then(|pr| pr.number);
    let failed = match number {
        Some(number) => gh.stack_pr(repo, place, number).err()?.to_string(),
        None => s::github_pr_stack_no_number(),
    };
    Some(Unstacked {
        repo: repo.clone(),
        parent,
        number,
        reason: failed,
    })
}

#[cfg(test)]
mod tests;
