//! Searching GitHub rather than the loaded rows: the field's text is read
//! as words and facets once typing pauses, sent for the shown tab alone,
//! and shown as chips that can each be removed. The tab badges keep
//! counting the tabs without it.

use std::time::Duration;

use daruda_github::PrFilter;
use gpui::{Context, Window};

use crate::github::{NO_REFINE, PrList, Refine, RefineProblem};
use crate::workspace::Workspace;

/// How long typing must pause before a GitHub search is sent, so a word
/// typed letter by letter is one request, not one per letter.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::workspace) enum SearchMode {
    /// The search narrows the rows already loaded and asks GitHub nothing.
    #[default]
    Loaded,
    /// The search is sent to GitHub, within the shown repositories.
    GitHub,
}

/// What the shown tab's search is this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum PageSearch<'a> {
    /// Narrow the loaded rows by this text, trimmed.
    Loaded(String),
    /// List what GitHub finds for this refinement; empty lists the tab.
    GitHub(&'a Refine),
    /// The typed search cannot be sent with the shown tab.
    Blocked(RefineProblem),
}

impl Workspace {
    pub(in crate::workspace) fn pr_search_mode(&self) -> SearchMode {
        self.pages.pull_requests.search_mode
    }

    /// The shown tab's search, as the list and its requests read it.
    pub(in crate::workspace) fn pr_page_search(&self, cx: &gpui::App) -> PageSearch<'_> {
        let page = &self.pages.pull_requests;
        match page.search_mode {
            SearchMode::Loaded => {
                PageSearch::Loaded(page.search.read(cx).value().trim().to_owned())
            }
            SearchMode::GitHub => {
                match page.committed.problem(crate::github::audience(page.filter)) {
                    Some(problem) => PageSearch::Blocked(problem),
                    None => PageSearch::GitHub(&page.committed),
                }
            }
        }
    }

    /// The refinement the shown tab is searched with; none while the search
    /// narrows loaded rows or cannot be sent.
    pub(in crate::workspace) fn pr_page_refine(&self) -> &Refine {
        let page = &self.pages.pull_requests;
        if page.search_mode == SearchMode::GitHub
            && page
                .committed
                .problem(crate::github::audience(page.filter))
                .is_none()
        {
            &page.committed
        } else {
            &NO_REFINE
        }
    }

    /// The searches a refresh reads again: every tab bare, for the badges,
    /// and the shown tab with its GitHub search.
    pub(in crate::workspace) fn pr_refresh_searches(&self) -> Vec<(PrFilter, Refine)> {
        let mut searches: Vec<(PrFilter, Refine)> = super::list::FILTERS
            .into_iter()
            .map(|filter| (filter, Refine::default()))
            .collect();
        let refine = self.pr_page_refine();
        if !refine.is_empty() {
            searches.push((self.pr_page_filter(), refine.clone()));
        }
        searches
    }

    /// Search the loaded rows, or GitHub. Switching sends what is typed at
    /// once; the choice is kept with the window.
    pub(in crate::workspace) fn set_pr_search_mode(
        &mut self,
        mode: SearchMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pages.pull_requests.search_mode == mode {
            return;
        }
        self.mutate_durable(cx, |ws, _| ws.pages.pull_requests.search_mode = mode);
        self.sync_pr_search_placeholder(window, cx);
        self.commit_pr_search(cx);
    }

    /// The field says what it searches: the one write of its placeholder
    /// after construction, for a switch and for a restored window alike.
    pub(in crate::workspace) fn sync_pr_search_placeholder(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = match self.pages.pull_requests.search_mode {
            SearchMode::Loaded => crate::surface::strings::pull_requests::search_placeholder(),
            SearchMode::GitHub => {
                crate::surface::strings::pull_requests::search_placeholder_github()
            }
        };
        self.pages.pull_requests.search.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx)
        });
    }

    /// The field changed. Loaded rows narrow at the next frame; a GitHub
    /// search waits for typing to pause.
    pub(in crate::workspace) fn pr_search_edited(&mut self, cx: &mut Context<Self>) {
        match self.pages.pull_requests.search_mode {
            SearchMode::Loaded => cx.notify(),
            SearchMode::GitHub => {
                let task = cx.spawn(async move |ws, cx| {
                    cx.background_executor().timer(SEARCH_DEBOUNCE).await;
                    // The window may have closed while typing paused.
                    // SILENT-OK: a closed window has nothing left to search for.
                    let _ = ws.update(cx, |ws, cx| ws.commit_pr_search(cx));
                });
                self.pages.pull_requests.debounce = Some(task);
            }
        }
    }

    /// Send the field's text now as the GitHub search, or drop the GitHub
    /// search while the loaded rows are searched.
    pub(in crate::workspace) fn commit_pr_search(&mut self, cx: &mut Context<Self>) {
        let page = &mut self.pages.pull_requests;
        page.debounce = None;
        let committed = match page.search_mode {
            SearchMode::Loaded => Refine::default(),
            SearchMode::GitHub => Refine::parse(&page.search.read(cx).value()),
        };
        if page.committed != committed {
            page.committed = committed;
            self.prune_pr_list();
            PrList::wanted(cx);
        }
        cx.notify();
    }

    /// Remove the facet at `index` from the GitHub search, in the field too,
    /// and search again at once.
    pub(in crate::workspace) fn remove_pr_facet(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self
            .pages
            .pull_requests
            .committed
            .without_facet(index)
            .text();
        // Silently: the search is applied here, at once, not after a pause.
        self.pages
            .pull_requests
            .search
            .update(cx, |input, cx| input.set_value_silently(text, window, cx));
        self.commit_pr_search(cx);
    }
}

#[cfg(feature = "screenshot")]
impl Workspace {
    /// The page searching GitHub for `text`, sent as if typing had paused.
    pub(in crate::workspace) fn seed_pr_github_search_for_shot(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_pr_search_mode(SearchMode::GitHub, window, cx);
        self.pages
            .pull_requests
            .search
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.commit_pr_search(cx);
    }
}

#[cfg(test)]
mod tests;
