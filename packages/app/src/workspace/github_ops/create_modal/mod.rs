//! The Create PR form: title (the last commit's subject until edited),
//! description, base branch and draft, and above them what it publishes
//! where. The workspace does the pushing and the `gh` call; this owns the
//! fields and what the form shows meanwhile.

mod creation;
mod render;
pub(crate) mod summary;
pub(crate) mod templates;

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AppContext as _, Context, Entity, FocusHandle, Focusable, SharedString, Subscription, Task,
    WeakEntity, Window,
};

use summary::{Against, Publication, TargetRead};
use templates::Templates;

use daruda_store::project::LaneRef;

use super::create::CreatePrRequest;
use crate::lane::git::GitRepo;
use crate::surface::strings::git as s;
use crate::ui::{InputEvent, InputState, TextareaState, WindowExt as _};
use crate::workspace::ModalView;
use crate::workspace::Workspace;

/// How long typing in the base settles before the summary is read again.
const SUMMARY_AFTER: Duration = Duration::from_millis(400);

/// Visible lines of the description box.
const BODY_ROWS: usize = crate::ui::theme::palette::CREATE_PR_BODY_ROWS;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CreateState {
    Editing,
    Creating,
    Failed(SharedString),
    /// The branch already has this pull request: open it, not a second.
    Exists(super::lane_pr::CreatedPr),
}

pub(crate) struct CreatePrModal {
    panel_focus: FocusHandle,
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    base: Entity<InputState>,
    draft: bool,
    state: CreateState,
    workspace: WeakEntity<Workspace>,
    /// The lane the form was opened for; the PR is for it even if another
    /// lane is shown by the time it is submitted.
    lane: LaneRef,
    repo: Arc<dyn GitRepo>,
    /// Where it pushes and what stays behind; `None` until read, and
    /// nothing is created before.
    publication: Option<Publication>,
    /// What the branch takes over the base it was read for.
    against: Option<(Option<String>, Against)>,
    /// The `against` read in flight; replaced (so dropped) by a newer one.
    against_read: Option<Task<()>>,
    /// The worktree's templates and the push remote's branches, read with
    /// the target.
    templates: Templates,
    remote_branches: Vec<String>,
    /// The base the form opened with, first among suggestions.
    default_base: Option<String>,
    /// The template the description was last filled from.
    template: Option<String>,
    /// Typed in by the user, clearing included: a late commit subject or
    /// template never replaces it.
    title_edited: bool,
    body_edited: bool,
    base_edited: bool,
    draft_edited: bool,
    /// The template was chosen by the user, not filled in by default.
    template_chosen: bool,
    /// Whose branch the fields are kept under; `None` until the target and
    /// the signed-in account are known, and nothing is kept before.
    creation_owner: Option<daruda_store::review_drafts::CreationOwner>,
    /// The fields came back from a draft left unsent.
    restored: bool,
    _subscriptions: [Subscription; 3],
}

impl CreatePrModal {
    pub(super) fn new(
        workspace: WeakEntity<Workspace>,
        lane: LaneRef,
        repo: Arc<dyn GitRepo>,
        base: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title = cx.new(|cx| InputState::new(window, cx).placeholder(s::pr_placeholder_title()));
        let body = crate::ui::make_markdown_prose_state(
            "",
            s::pr_placeholder_body(),
            BODY_ROWS,
            window,
            cx,
        );
        let default_base = base.clone();
        let base_value = base.unwrap_or_default();
        let base = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(s::pr_placeholder_base())
                .default_value(base_value)
        });
        let on_input = |state: &Entity<InputState>, window: &mut Window, cx: &mut Context<Self>| {
            cx.subscribe_in(
                state,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    InputEvent::Change => {
                        this.clear_failure(cx);
                        if *input == this.base {
                            this.base_edited = true;
                            this.read_against(Some(SUMMARY_AFTER), window, cx);
                        } else {
                            this.title_edited = true;
                        }
                        this.keep_creation(cx);
                    }
                    InputEvent::Focus | InputEvent::Blur => {}
                },
            )
        };
        let on_body = cx.subscribe_in(&body, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.body_edited = true;
                this.clear_failure(cx);
                this.keep_creation(cx);
            }
        });
        let _subscriptions = [
            on_input(&title, window, cx),
            on_input(&base, window, cx),
            on_body,
        ];

        // The last commit's subject is the usual title; read off the UI
        // thread and used only if the user has not typed one meanwhile.
        let title_repo = repo.clone();
        cx.spawn_in(window, async move |this, cx| {
            let repo = title_repo;
            let message = cx
                .background_executor()
                .spawn(async move { repo.head_message() })
                .await;
            let Ok(message) = message else {
                return;
            };
            let subject = message.lines().next().unwrap_or_default().trim().to_owned();
            // SILENT-OK: the form may have closed before the commit was read.
            let _ = this.update_in(cx, |this, window, cx| {
                this.prefill_title(subject, window, cx)
            });
        })
        .detach();

        let mut this = Self {
            panel_focus: cx.focus_handle(),
            title,
            body,
            base,
            draft: false,
            state: CreateState::Editing,
            workspace,
            lane,
            repo,
            publication: None,
            against: None,
            against_read: None,
            templates: Templates::default(),
            remote_branches: Vec::new(),
            default_base,
            template: None,
            title_edited: false,
            body_edited: false,
            base_edited: false,
            draft_edited: false,
            template_chosen: false,
            creation_owner: None,
            restored: false,
            _subscriptions,
        };
        this.read_publication(window, cx);
        this
    }

    /// Where the branch pushes and what stays behind, then what it takes
    /// over the base typed.
    fn read_publication(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        cx.spawn_in(window, async move |this, cx| {
            let (read, templates, branches) = cx
                .background_executor()
                .spawn(async move {
                    let read = summary::read_publication(repo.as_ref());
                    let templates = templates::find_templates(repo.path());
                    let branches = match &read.target {
                        // SILENT-OK: suggestions only; a failed read offers
                        // none and the base can still be typed.
                        TargetRead::Pushes { shown, .. } => {
                            repo.remote_branches(&shown.remote).unwrap_or_default()
                        }
                        TargetRead::NoTarget | TargetRead::Unreadable => Vec::new(),
                    };
                    (read, templates, branches)
                })
                .await;
            // SILENT-OK: the form may have closed before git answered.
            let _ = this.update_in(cx, |this, window, cx| {
                this.publication = Some(read);
                this.remote_branches = branches;
                this.templates = templates;
                // What was left unsent for this branch comes back first,
                // where the user has not typed since the form opened.
                this.restore_creation(window, cx);
                // Typed, or typed and cleared, before it arrived, or a
                // template brought back: kept.
                if let Some(default) = this.templates.default.clone()
                    && !this.body_edited
                    && this.template.is_none()
                {
                    this.apply_template(&default.path, window, cx);
                }
                // Typed before the target was read: kept now it is.
                this.keep_creation(cx);
                this.read_against(None, window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The template button: fill the description from the template at
    /// `path`, unless the user has written one, and keep the choice.
    pub(super) fn use_template(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.apply_template(path, window, cx) {
            self.template_chosen = true;
            self.keep_creation(cx);
        }
    }

    /// Fill the description from the template at `path` unless the user
    /// has written one: their text is never replaced. `true` if filled.
    pub(super) fn apply_template(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.keeps_description(cx) {
            return false;
        }
        let Some(template) = self.templates.all().find(|t| t.path == path).cloned() else {
            return false;
        };
        self.body.update(cx, |input, cx| {
            input.set_value_silently(template.body, window, cx)
        });
        // What is there now is the template's, not the user's.
        self.body_edited = false;
        self.template = Some(template.path);
        cx.notify();
        true
    }

    /// The user wrote a description: a template does not replace it.
    pub(super) fn keeps_description(&self, cx: &gpui::App) -> bool {
        self.body_edited && !self.body.read(cx).value().trim().is_empty()
    }

    /// Branches of the push remote whose name holds what is typed in the
    /// base, the head left out; at most a few.
    pub(super) fn base_suggestions(&self, cx: &gpui::App) -> Vec<String> {
        const SHOWN: usize = 6;
        let typed = self.base_value(cx).unwrap_or_default().to_lowercase();
        let head = self
            .publication
            .as_ref()
            .and_then(|read| read.target.head().map(str::to_owned));
        let mut matching: Vec<&String> = self
            .remote_branches
            .iter()
            .filter(|branch| Some(branch.as_str()) != head.as_deref())
            .filter(|branch| {
                let name = branch.to_lowercase();
                name != typed && name.contains(&typed)
            })
            .collect();
        // Names starting with what is typed first; the default branch
        // leads an empty search.
        let default = self.default_base.as_deref();
        matching.sort_by_key(|branch| {
            (
                Some(branch.as_str()) != default,
                !branch.to_lowercase().starts_with(&typed),
            )
        });
        matching.into_iter().take(SHOWN).cloned().collect()
    }

    /// Take a suggested base, as if typed.
    pub(super) fn choose_base(
        &mut self,
        branch: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.base
            .update(cx, |input, cx| input.set_value(branch, window, cx));
    }

    /// What the branch takes over the base typed now, after `delay` when
    /// typing; an older read still running is dropped.
    fn read_against(
        &mut self,
        delay: Option<Duration>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(head) = self
            .publication
            .as_ref()
            .map(|read| read.target.head().map(str::to_owned))
        else {
            // Read once the target is known.
            return;
        };
        let base = self.base_value(cx);
        let repo = self.repo.clone();
        self.against_read = Some(cx.spawn_in(window, async move |this, cx| {
            if let Some(delay) = delay {
                cx.background_executor().timer(delay).await;
            }
            let asked = base.clone();
            let read = cx
                .background_executor()
                .spawn(async move {
                    summary::read_against(repo.as_ref(), base.as_deref(), head.as_deref())
                })
                .await;
            // SILENT-OK: the form may have closed before git answered.
            let _ = this.update(cx, |this, cx| {
                this.against = Some((asked, read));
                this.against_read = None;
                cx.notify();
            });
        }));
    }

    /// The comparison, when it was read for the base typed now.
    pub(super) fn current_against(&self, cx: &gpui::App) -> Option<&Against> {
        let (asked, against) = self.against.as_ref()?;
        (*asked == self.base_value(cx)).then_some(against)
    }

    fn base_value(&self, cx: &gpui::App) -> Option<String> {
        let base = self.base.read(cx).value().trim().to_owned();
        (!base.is_empty()).then_some(base)
    }

    fn prefill_title(&mut self, subject: String, window: &mut Window, cx: &mut Context<Self>) {
        // Typed, or typed and cleared: the user's choice stands.
        if subject.is_empty() || self.title_edited {
            return;
        }
        self.title.update(cx, |input, cx| {
            input.set_value_silently(subject, window, cx)
        });
        cx.notify();
    }

    fn clear_failure(&mut self, cx: &mut Context<Self>) {
        if matches!(self.state, CreateState::Failed(_)) {
            self.state = CreateState::Editing;
            cx.notify();
        }
    }

    fn toggle_draft(&mut self, cx: &mut Context<Self>) {
        self.draft = !self.draft;
        self.draft_edited = true;
        self.clear_failure(cx);
        self.keep_creation(cx);
        cx.notify();
    }

    /// The request the fields describe, or `None` while the title is blank.
    pub(super) fn request(&self, cx: &gpui::App) -> Option<CreatePrRequest> {
        let title = self.title.read(cx).value().trim().to_owned();
        if title.is_empty() {
            return None;
        }
        Some(CreatePrRequest {
            title,
            body: self.body.read(cx).value().to_string(),
            base: self.base_value(cx),
            draft: self.draft,
            // The target does not depend on the base: the one shown holds.
            expected: self
                .publication
                .as_ref()
                .map_or(summary::Expected::Unchecked, |read| read.target.expected()),
        })
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state == CreateState::Creating {
            return;
        }
        // Nothing is created before the form has shown where it goes.
        if self.publication.is_none() {
            return;
        }
        let Some(request) = self.request(cx) else {
            self.state = CreateState::Failed(s::pr_err_title_required().into());
            cx.notify();
            return;
        };
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        self.state = CreateState::Creating;
        cx.notify();
        let me = cx.weak_entity();
        let handle = window.window_handle();
        let lane = self.lane;
        workspace.update(cx, |ws, cx| ws.create_pr(lane, request, me, handle, cx));
    }

    /// [`Self::new`] for workspace tests, which build the form without
    /// opening a dialog.
    #[cfg(test)]
    pub(in crate::workspace) fn new_for_test(
        workspace: WeakEntity<Workspace>,
        lane: LaneRef,
        repo: Arc<dyn GitRepo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(workspace, lane, repo, None, window, cx)
    }

    /// Type `title` and `body` as the user would (Change events included).
    #[cfg(test)]
    pub(crate) fn type_for_test(
        &mut self,
        title: &str,
        body: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.title
            .update(cx, |input, cx| input.set_value(title, window, cx));
        self.body
            .update(cx, |input, cx| input.set_value(body, window, cx));
    }

    #[cfg(test)]
    pub(crate) fn fields_for_test(&self, cx: &gpui::App) -> (String, String) {
        (
            self.title.read(cx).value().to_string(),
            self.body.read(cx).value().to_string(),
        )
    }

    /// Choose a template and a base as the buttons do.
    #[cfg(test)]
    pub(crate) fn choose_for_test(
        &mut self,
        template: &str,
        base: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.use_template(template, window, cx);
        self.choose_base(base.to_owned(), window, cx);
    }

    #[cfg(test)]
    pub(crate) fn toggle_draft_for_test(&mut self, cx: &mut Context<Self>) {
        self.toggle_draft(cx);
    }

    /// The base and the draft flag as a request would carry them.
    #[cfg(test)]
    pub(crate) fn request_for_test(&self, cx: &gpui::App) -> (Option<String>, bool) {
        (self.base_value(cx), self.draft)
    }

    #[cfg(test)]
    pub(crate) fn restored_for_test(&self) -> bool {
        self.restored
    }

    #[cfg(test)]
    pub(crate) fn discard_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.discard_creation(window, cx);
    }

    #[cfg(test)]
    pub(crate) fn failure(&self) -> Option<&str> {
        match &self.state {
            CreateState::Failed(message) => Some(message.as_ref()),
            CreateState::Editing | CreateState::Creating | CreateState::Exists(_) => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn existing(&self) -> Option<&super::lane_pr::CreatedPr> {
        match &self.state {
            CreateState::Exists(created) => Some(created),
            _ => None,
        }
    }

    /// The branch already has a pull request: the form offers to open it.
    pub(super) fn found_existing(
        &mut self,
        created: super::lane_pr::CreatedPr,
        cx: &mut Context<Self>,
    ) {
        self.state = CreateState::Exists(created);
        cx.notify();
    }

    /// Open the pull request the branch already has, and close the form.
    fn open_existing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let CreateState::Exists(created) = &self.state {
            cx.open_url(&created.url);
            window.close_dialog(cx);
        }
    }

    /// The workspace reports a failure; the form stays open with it.
    pub(super) fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self.state = CreateState::Failed(message.into());
        cx.notify();
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut gpui::App) {
        window.close_dialog(cx);
    }
}

impl Focusable for CreatePrModal {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.title.focus_handle(cx)
    }
}

impl ModalView for CreatePrModal {}

#[cfg(test)]
mod tests;
