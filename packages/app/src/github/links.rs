//! The process-wide store of how the user tied worktrees to pull requests:
//! what every window reads when it decides a lane's pull request, saved
//! shortly after each change and at quit. Installed by the primary desktop
//! instance; without it (tests) links are kept in memory only.

use std::path::Path;

use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use daruda_store::pr_links::{
    LinkSource, PrLinks, PrRef, WorktreeLinks, load_pr_links_in, pr_links_dir, save_pr_links_in,
};
use gpui::{App, Global};

use super::saved_file::{SAVE_AFTER, SavedFile};

#[derive(Default)]
pub(crate) struct LinkStore {
    links: PrLinks,
    file: SavedFile,
}

impl Global for LinkStore {}

impl LinkStore {
    /// Load the profile's links and save there from now on. A damaged or
    /// newer file is left alone: links are then kept in memory only.
    pub(crate) fn install(cx: &mut App) {
        let dir = pr_links_dir();
        let store = match load_pr_links_in(&dir) {
            Some(links) => Self {
                links,
                file: SavedFile::at(Some(dir)),
            },
            None => Self::default(),
        };
        cx.set_global(store);
    }

    /// Whether what the user ties and unties is kept across restarts.
    pub(crate) fn keeping(cx: &App) -> super::Keeping {
        match cx.try_global::<Self>() {
            // No store installed: tests, where nothing is promised.
            None => super::Keeping::Saved,
            Some(store) => store.file.keeping(),
        }
    }

    pub(crate) fn of(worktree: &Path, branch: &str, cx: &App) -> Option<WorktreeLinks> {
        cx.try_global::<Self>()?.links.of(worktree, branch).cloned()
    }

    /// Change the links with `edit`, then save them shortly. A lane's
    /// links change through `Workspace::edit_lane_links`, which also brings
    /// the lane up to date.
    pub(crate) fn edit(cx: &mut App, edit: impl FnOnce(&mut PrLinks)) {
        let store = cx.default_global::<Self>();
        edit(&mut store.links);
        store.links.prune();
        if !store.file.changed() {
            return;
        }
        let write = cx.spawn(async move |cx| {
            cx.background_executor().timer(SAVE_AFTER).await;
            cx.update(Self::flush);
        });
        cx.default_global::<Self>().file.scheduled(write);
    }

    /// Write what changed now: when a burst settles, and at quit.
    pub(crate) fn flush(cx: &mut App) {
        let Some(store) = cx.try_global::<Self>() else {
            return;
        };
        let Some(dir) = store.file.due() else {
            return;
        };
        let written = save_pr_links_in(&dir, &store.links);
        cx.default_global::<Self>().file.written(&written);
        match written {
            Ok(()) => {}
            Err(e) => {
                LogWriter::log(
                    ErrorReport::new(crate::surface::strings::pull_requests::links_save_failed())
                        .severity(ErrorSeverity::Error)
                        .from_error(&e)
                        .at(file!(), line!())
                        .dedup("github.pr_links.save")
                        .build(),
                );
            }
        }
    }

    /// Point a test's store at `dir`.
    #[cfg(test)]
    pub(crate) fn install_at(dir: std::path::PathBuf, cx: &mut App) {
        let links = load_pr_links_in(&dir).unwrap_or_default();
        cx.set_global(Self {
            links,
            file: SavedFile::at(Some(dir)),
        });
    }
}

/// `repo#number` as the store names it.
pub(crate) fn pr_ref(repo: &daruda_github::RepoId, number: u64) -> PrRef {
    PrRef {
        host: repo.host().to_owned(),
        owner: repo.owner().to_owned(),
        name: repo.name().to_owned(),
        number,
    }
}

/// The repository and number a stored reference names.
pub(crate) fn pr_key(pr: &PrRef) -> (daruda_github::RepoId, u64) {
    (
        daruda_github::RepoId::new(&pr.host, &pr.owner, &pr.name),
        pr.number,
    )
}

/// Re-exported so lane code names the source without the store crate.
pub(crate) type Source = LinkSource;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_names_the_same_pull_request_back() {
        let repo = daruda_github::RepoId::new("github.com", "o", "r");
        assert_eq!(pr_key(&pr_ref(&repo, 7)), (repo, 7));
    }
}
