//! The Files tab's reads and the person's moves through it.

use super::*;

impl PrDetailState {
    pub(in super::super) fn reveal_file_row(&self) {
        if let Some(at) = self.files.as_ref().and_then(FilesState::shown_index) {
            self.files_scroll
                .scroll_to_item(at, gpui::ScrollStrategy::Nearest);
        }
    }

    /// Bring a reopened detail back to the file it showed, once that file
    /// is shown and the pull request is read, so the tab is drawn at its
    /// full height to scroll in.
    pub(in super::super) fn come_back(&mut self) {
        if !matches!(self.pr.load(), super::super::Load::Ready(_))
            || self.tab != super::super::PrTab::Files
        {
            return;
        }
        if let Some(restored) = self.files.as_mut().and_then(FilesState::take_restored) {
            self.files_scroll
                .scroll_to_item(restored.index, gpui::ScrollStrategy::Center);
            if let Some((x, y)) = restored.offset {
                self.scroll
                    .set_offset(gpui::point(gpui::px(x), gpui::px(y)));
            }
        }
    }

    /// The person moved on: a reopened detail no longer comes back.
    pub(in super::super) fn stop_coming_back(&mut self) {
        if let Some(files) = self.files.as_mut() {
            files.restore = None;
        }
    }

    pub(in super::super) fn begin_files_read(&mut self) -> ReadTicket {
        let ticket = self.tickets.issue();
        self.files = Some(FilesState::reading(ticket));
        ticket
    }

    /// Read the shown files again in place.
    pub(in super::super) fn reread_files(&mut self) -> ReadTicket {
        let ticket = self.tickets.issue();
        match self.files.as_mut() {
            Some(files) => files.reread(ticket),
            None => self.files = Some(FilesState::reading(ticket)),
        }
        ticket
    }

    pub(in crate::workspace) fn files(&self) -> Option<&FilesState> {
        self.files.as_ref()
    }
}

impl Workspace {
    pub(in super::super) fn send_pr_files_read(
        &mut self,
        id: PrDetailId,
        ticket: ReadTicket,
        page: u32,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let (repo, number) = (detail.item.repo.clone(), detail.item.pr.number);
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || gh.pr_files_page(&repo, number, page),
            move |ws, result, cx| ws.land_pr_files(id, ticket, page, result, cx),
        )
        .detach();
    }

    /// Only the page waited on lands; a further page is sent at once.
    pub(in crate::workspace) fn land_pr_files(
        &mut self,
        id: PrDetailId,
        ticket: ReadTicket,
        page: u32,
        result: Result<PrFilesPage, GhError>,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let PrDetailState { files, tickets, .. } = detail;
        let Some(state) = files.as_mut() else {
            return;
        };
        let (answer, error) = match result {
            Ok(read) => (Ok(read), None),
            Err(e) => (Err(e.to_string()), Some(e)),
        };
        // The head as last read, unless a read of it is out: then the files
        // take it when that read lands.
        let head = detail.pr.confirmed().map(|pr| pr.head_sha.as_str());
        let previous = state.shown_index();
        let landed = state.land(ticket, page, answer, head, || tickets.issue());
        if landed == Landed::Stale {
            return;
        }
        if state.shown_index() != previous {
            detail.reveal_file_row();
        }
        detail.come_back();
        if let Some(e) = error {
            self.report_pr_read(&e, "github.pr_files.read", cx);
        }
        if let Landed::Next(ticket, page) = landed {
            self.send_pr_files_read(id, ticket, page, cx);
        }
        self.follow_pr_comment(id, cx);
        cx.notify();
    }

    /// Read the files on from where reading stopped or failed.
    pub(in crate::workspace) fn resume_pr_files(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let ticket = detail.tickets.issue();
        let Some(page) = detail.files.as_mut().and_then(|files| files.resume(ticket)) else {
            return;
        };
        self.send_pr_files_read(id, ticket, page, cx);
        cx.notify();
    }

    /// Show `path`'s diff in detail `id`'s Files tab.
    pub(in crate::workspace) fn show_pr_file(
        &mut self,
        id: PrDetailId,
        path: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let Some(files) = detail.files.as_mut() else {
            return;
        };
        if files.blob_of(path).is_none() {
            return;
        }
        files.restore = None;
        files.show(path);
        detail.comment_location = None;
        detail.reveal_file_row();
        cx.notify();
    }

    pub(super) fn with_pr_files(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut FilesState),
    ) {
        if let Some(detail) = self.pr_detail_mut(id)
            && let Some(state) = detail.files.as_mut()
        {
            detail.comment_location = None;
            change(state);
            cx.notify();
        }
    }

    /// Mark `path` viewed at its listed content, or clear the mark.
    pub(in crate::workspace) fn toggle_pr_file_viewed(
        &mut self,
        id: PrDetailId,
        path: &str,
        displayed_blob: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let key = detail.key();
        let Some(blob) = displayed_blob else { return };
        if detail
            .files()
            .and_then(|files| files.blob_of(path))
            .flatten()
            != Some(blob)
        {
            return;
        }
        self.pages
            .pull_requests
            .progress
            .toggle_viewed(&key, path, Some(blob));
        cx.notify();
    }

    pub(in crate::workspace) fn toggle_pr_file_picker(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        self.with_pr_files(id, cx, |state| state.picker_open = !state.picker_open);
    }

    /// Mark the displayed blob and advance through the listing, wrapping once
    /// to revisit any files skipped earlier or changed since their last mark.
    pub(in crate::workspace) fn finish_pr_file(
        &mut self,
        id: PrDetailId,
        path: &str,
        displayed_blob: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let page = &mut self.pages.pull_requests;
        let Some(detail) = page.details.get(id) else {
            return;
        };
        let Some(files) = detail.files() else { return };
        if files.shown.as_ref().is_none_or(|shown| shown.path != path) {
            return;
        }
        let Some(at) = files.files.iter().position(|file| file.path == path) else {
            return;
        };
        let Some(blob) = displayed_blob else { return };
        if files.files[at].blob.as_deref() != Some(blob) {
            return;
        }
        let key = detail.key();
        let marks = page.progress.marks(&key);
        let next = (at + 1..files.len())
            .chain(0..at)
            .find(|&index| {
                let file = &files.files[index];
                marks.viewed(&file.path, file.blob.as_deref()) != Viewed::Yes
            })
            .map(|index| files.files[index].path.clone());
        if marks.viewed(path, Some(blob)) != Viewed::Yes {
            page.progress.toggle_viewed(&key, path, Some(blob));
        }
        if let Some(path) = next {
            self.show_pr_file(id, &path, cx);
            if let Some(detail) = self.pr_detail_mut(id) {
                detail.scroll.set_offset(gpui::Point::default());
            }
        }
        cx.notify();
    }

    /// Show the file `delta` places from the shown one.
    pub(in crate::workspace) fn show_adjacent_pr_file(
        &mut self,
        id: PrDetailId,
        delta: isize,
        cx: &mut Context<Self>,
    ) {
        self.with_pr_files(id, cx, |state| {
            state.restore = None;
            state.show_adjacent(delta);
        });
        if let Some(detail) = self.pr_detail(id) {
            detail.reveal_file_row();
        }
    }

    pub(in crate::workspace) fn set_pr_diff_mode(
        &mut self,
        id: PrDetailId,
        mode: DiffMode,
        cx: &mut Context<Self>,
    ) {
        self.with_pr_files(id, cx, |state| state.set_mode(mode));
    }

    pub(in crate::workspace) fn set_pr_diff_unchanged(
        &mut self,
        id: PrDetailId,
        unchanged: Unchanged,
        cx: &mut Context<Self>,
    ) {
        self.with_pr_files(id, cx, |state| state.set_unchanged(unchanged));
    }

    /// Select a line of the shown file; `extend` stretches the selection.
    pub(in crate::workspace) fn select_pr_line(
        &mut self,
        id: PrDetailId,
        side: Side,
        line: u32,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        self.with_pr_files(id, cx, |state| state.select_line(side, line, extend));
    }

    pub(in crate::workspace) fn clear_pr_lines(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        self.with_pr_files(id, cx, FilesState::clear_selection);
    }

    /// Copy the selected lines' reference: path, lines, side, revision.
    pub(in crate::workspace) fn copy_pr_lines(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        let copied = self
            .pr_detail_mut(id)
            .and_then(|d| d.files.as_ref())
            .and_then(FilesState::selection_copy);
        if let Some(copied) = copied {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied));
        }
    }

    /// Read the shown file's diff whole: GitHub sent no patch for it.
    pub(in crate::workspace) fn recover_pr_file(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let Some(base_branch) = detail.pr.last_known().map(|pr| pr.base_branch.clone()) else {
            return;
        };
        let ticket = detail.tickets.issue();
        let Some(start) = detail
            .files
            .as_mut()
            .and_then(|files| files.begin_recover(ticket))
        else {
            return;
        };
        let target = RecoverTarget {
            repo: detail.item.repo.clone(),
            base_branch,
            head: start.head,
            merge_base: start.merge_base,
            path: start.path,
            change: start.change,
            lines: start.lines,
        };
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || recover_patch(gh.as_ref(), &target),
            move |ws, result, cx| ws.land_pr_recovered(id, ticket, result, cx),
        )
        .detach();
        cx.notify();
    }

    /// Only the read waited on lands; a `gh` failure is reported too.
    pub(in crate::workspace) fn land_pr_recovered(
        &mut self,
        id: PrDetailId,
        ticket: ReadTicket,
        result: Result<Recovered, RecoverError>,
        cx: &mut Context<Self>,
    ) {
        let Some(files) = self.pr_detail_mut(id).and_then(|d| d.files.as_mut()) else {
            return;
        };
        let (answer, cause) = match result {
            Ok(read) => (Ok(read), None),
            Err(error) => (Err(error.failure), error.cause),
        };
        if !files.land_recovered(ticket, answer) {
            return;
        }
        if let Some(e) = cause {
            self.report_pr_read(&e, "github.pr_file.recover", cx);
        }
        cx.notify();
    }

    /// Draw more of the shown diff's rows.
    pub(in crate::workspace) fn show_more_pr_diff(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.pr_detail_mut(id).and_then(|d| d.files.as_mut()) {
            state.show_more_rows();
            cx.notify();
        }
    }

    pub(in crate::workspace) fn show_earlier_pr_diff(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.pr_detail_mut(id).and_then(|d| d.files.as_mut()) {
            state.show_earlier_rows();
            cx.notify();
        }
    }
}
