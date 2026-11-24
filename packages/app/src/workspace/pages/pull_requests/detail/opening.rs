//! Opening a pull request's detail: where it opens (a tab asked for, a
//! linked comment, or the place it was left), reusing the detail already
//! open for that pull request, and what is read once something shows it.

use daruda_github::{PrDetail, PrListItem};
use gpui::{Context, Window};

use super::super::details::DetailHold;
use super::{PrDetailId, PrDetailState, PrTab, TabRead, anchors, progress};
use crate::workspace::Workspace;
use crate::workspace::pages::Page;
use crate::workspace::pages::detail::PageDetailId;

/// What is left of opening a detail once something shows it.
pub(in crate::workspace) enum PrDetailStart {
    /// It was open already: only the tab asked for is shown.
    Shown { at: Option<OpeningTarget> },
    /// It is new: its pull request is read, or the read made to open it
    /// is landed, and the tab it opens at reads what it shows.
    New {
        read: Option<Box<PrDetail>>,
        tab_reads: Vec<TabRead>,
    },
}

#[derive(Debug)]
pub(in crate::workspace) enum OpeningTarget {
    Tab(PrTab),
    Comment(anchors::CommentLink),
}

impl OpeningTarget {
    pub(super) fn tab(&self) -> PrTab {
        match self {
            Self::Tab(tab) => *tab,
            Self::Comment(link) => link.kind.tab(),
        }
    }
}

/// How a detail opens, beyond which pull request.
#[derive(Debug, Default)]
pub(in crate::workspace) struct PrOpening {
    /// The tab to show; `None` keeps where it was left.
    pub(in crate::workspace) at: Option<OpeningTarget>,
    /// The pull request read just now to open it, landed as a new detail's
    /// read. Fresh because a pull request detail never holds unsaved edits,
    /// so its switch is never held behind a prompt. Boxed: a detail is large.
    pub(in crate::workspace) read: Option<Box<PrDetail>>,
}

impl PrDetailState {
    /// Where a detail opens: on `tab` when one was asked for, else where it
    /// was left. A tab asked for that is the one left keeps its place.
    fn open_at(&mut self, place: Option<&progress::Place>, tab: Option<PrTab>) -> Vec<TabRead> {
        match (tab, place) {
            (Some(tab), Some(place)) if place.tab == tab => self.restore(place),
            (Some(tab), _) => self.select_tab(tab),
            (None, Some(place)) => self.restore(place),
            // Never visited: the first tab, read like any other shown.
            (None, None) => self.select_tab(self.tab),
        }
    }

    /// Come back to `place`: its tab, and on the Files tab the file shown,
    /// with where the detail was scrolled if that file did not change.
    /// Return the reads its visible content needs.
    fn restore(&mut self, place: &progress::Place) -> Vec<TabRead> {
        let read = self.select_tab(place.tab);
        if let (Some(files), Some(file)) = (self.files.as_mut(), place.file.as_ref()) {
            files.restore_to(file, place.offset);
        }
        read
    }
}

impl Workspace {
    /// Show `item` over the list. The same pull request already open is
    /// shown as it is; another replaces the detail. Nothing opens while
    /// `[github] enabled` is off, as the list shows no rows then.
    pub(in crate::workspace) fn open_pr_detail(
        &mut self,
        item: PrListItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_pr_detail_as(item, PrOpening::default(), window, cx);
    }

    /// Open `item`'s detail as `opening` asks, applied as the detail is put up.
    pub(in crate::workspace) fn open_pr_detail_as(
        &mut self,
        item: PrListItem,
        opening: PrOpening,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !crate::github::enabled(cx) {
            return;
        }
        self.pages.pull_requests.pending_open = None;
        let searched_alone = self.pages.pull_requests.list_covers_detail();
        self.pages.pull_requests.searching_list = false;
        self.select_pr_row((item.repo.clone(), item.pr.number));
        if !self.github_repos().contains(&item.repo) {
            let project = self.lane_scoped.iter().find_map(|(lane, state)| {
                state
                    .github_push()
                    .filter(|(repo, _)| *repo == item.repo)
                    .map(|_| lane.project)
            });
            if let Some(project) = project {
                self.reveal_page_project(project, cx);
            } else {
                self.set_page_scope(crate::workspace::pages::scope::ProjectScope::All, cx);
            }
        }
        let open = self
            .pages
            .pull_requests
            .detail()
            .filter(|d| d.item.repo == item.repo && d.item.pr.number == item.pr.number)
            .map(|d| d.id);
        if let Some(id) = open {
            self.show_page(Page::PullRequests, cx);
            // The open detail keeps its own reads: `opening.read` started
            // before them could be older than one already landed.
            self.apply_pr_opening(id, opening.at, cx);
            if searched_alone {
                self.focus_shown_page(window, cx);
            }
            cx.notify();
            return;
        }
        self.leave_page_detail_then(Page::PullRequests, window, cx, move |ws, window, cx| {
            ws.install_pr_detail(item, opening, cx);
            // The list that had the keyboard is no longer drawn.
            if ws.active_page() == Some(Page::PullRequests) {
                ws.focus_shown_page(window, cx);
            }
        });
    }

    /// Put `item`'s detail on the page: the one already open for that
    /// pull request (a worktree's pane holds it), else a new one.
    fn install_pr_detail(&mut self, item: PrListItem, opening: PrOpening, cx: &mut Context<Self>) {
        let (hold, reads) = self.pr_detail_for(item, opening, cx);
        let id = hold.id();
        self.pages.pull_requests.show_detail(Some(hold));
        self.settle_pr_details(cx);
        self.start_pr_detail(id, reads, cx);
        cx.notify();
    }

    /// A hold on the open detail of `item`'s pull request, or on a new
    /// one, with what is left to do once it is shown. The detail lives as
    /// long as the hold: the caller gives it to the page or to a pane,
    /// then starts the detail.
    pub(in crate::workspace) fn pr_detail_for(
        &mut self,
        item: PrListItem,
        opening: PrOpening,
        cx: &mut gpui::App,
    ) -> (DetailHold, PrDetailStart) {
        // A detail that ended earlier in this update has not been settled
        // yet: its place is recorded first, so this pull request opened
        // again comes back to it.
        self.settle_pr_details(cx);
        let page = &mut self.pages.pull_requests;
        let open = page
            .details
            .of(&item.repo, item.pr.number)
            .and_then(|id| page.details.hold(id));
        if let Some(hold) = open {
            // The open detail keeps its own reads: `opening.read` started
            // before them could be older than one already landed.
            return (hold, PrDetailStart::Shown { at: opening.at });
        }
        let id = PrDetailId(self.alloc_id());
        let mut detail = PrDetailState::new(id, item);
        let page = &mut self.pages.pull_requests;
        let place = page.progress.place(&detail.key()).cloned();
        let tab_reads = detail.open_at(place.as_ref(), opening.at.as_ref().map(OpeningTarget::tab));
        if let Some(OpeningTarget::Comment(link)) = opening.at {
            detail.comment_location = Some(anchors::CommentLocation::Looking(link));
        }
        let hold = page.details.open(detail);
        (
            hold,
            PrDetailStart::New {
                read: opening.read,
                tab_reads,
            },
        )
    }

    /// Settle what details nothing holds any more leave behind: their
    /// place to come back to, and what was typed in them on disk.
    pub(in crate::workspace) fn settle_pr_details(&mut self, cx: &mut gpui::App) {
        if self.pages.pull_requests.settle_ended_details() {
            crate::github::DraftStore::flush(cx);
        }
    }

    /// Finish opening detail `id`, now that something shows it.
    pub(in crate::workspace) fn start_pr_detail(
        &mut self,
        id: PrDetailId,
        start: PrDetailStart,
        cx: &mut Context<Self>,
    ) {
        match start {
            PrDetailStart::Shown { at } => self.apply_pr_opening(id, at, cx),
            PrDetailStart::New { read, tab_reads } => {
                crate::github::DraftStore::flush(cx);
                match read {
                    // Read just now to open it: shown as is, not asked again.
                    Some(read) => self.land_pr_read(id, *read, cx),
                    None => self.read_pr_detail(id, cx),
                }
                for read in tab_reads {
                    self.send_pr_tab_read(id, read, cx);
                }
            }
        }
    }

    /// `[github] enabled` was switched off: the detail closes with the list,
    /// and so does every worktree's pull request pane, since nothing they
    /// show is kept current and their reads and actions would still run `gh`.
    pub(crate) fn close_pr_detail_while_off(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pages.pull_requests.pending_open = None;
        if let Some(id) = self.pr_detail_id() {
            self.drop_gone_detail(PageDetailId::PullRequest(id), window, cx);
        }
        self.close_pr_panes(window, cx);
    }

    /// Back to the list.
    #[cfg(test)]
    pub(in crate::workspace) fn back_to_pr_list(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_page_detail_then(Page::PullRequests, window, cx, |_, _, _| {});
    }
}

#[cfg(test)]
mod tests {
    use super::super::progress::{Place, PlaceFile};
    use super::super::{PrDetailId, PrDetailState, PrTab};

    fn place(tab: PrTab) -> Place {
        Place {
            tab,
            file: Some(PlaceFile {
                path: "src/a.rs".into(),
                blob: None,
            }),
            offset: (0.0, -40.0),
        }
    }

    /// A tab asked for wins over where the detail was left; asked for the
    /// tab it was left on, it comes back to the file there too.
    #[test]
    fn a_tab_asked_for_opens_and_keeps_the_place_only_on_its_own_tab() {
        let item = crate::workspace::tests::pull_request_detail::item(7, "feat");
        let cases = [
            (
                "no tab: where it was left",
                None,
                PrTab::Files,
                PrTab::Files,
                true,
            ),
            (
                "another tab: that tab",
                Some(PrTab::Checks),
                PrTab::Files,
                PrTab::Checks,
                false,
            ),
            (
                "the same tab: its place too",
                Some(PrTab::Files),
                PrTab::Files,
                PrTab::Files,
                true,
            ),
        ];
        for (label, tab, left_on, shown, restored) in cases {
            let mut state = PrDetailState::new(PrDetailId(1), item.clone());
            state.open_at(Some(&place(left_on)), tab);
            assert_eq!(state.tab, shown, "{label}");
            let chosen = state
                .files
                .as_ref()
                .is_some_and(|files| files.restoring_to("src/a.rs"));
            assert_eq!(chosen, restored, "{label}");
        }
    }
}
