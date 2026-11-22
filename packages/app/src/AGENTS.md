# App Instructions

GPUI-side of daruda: app entry, windows, Workspace, panes, and host integration.
These guardrails apply to app source. Detailed maps and recipes live in
[App Reference](REFERENCE.md); read the matching section before changing it.

## Required Reading

| Working on | Read |
|---|---|
| File placement, module ownership, or a new affordance | [Layout](REFERENCE.md#directory-layout), [recipes](REFERENCE.md#extension-recipes-quick-reference), [decision matrix](REFERENCE.md#where-things-go-decision-matrix) |
| Config precedence, agent data, or workspace lifecycle | [Subsystem contracts](REFERENCE.md#layered-config-user--project) through the workspace and lane sections |
| Modals, focus, or inputs | G9 below, [tab navigation](REFERENCE.md#tab-navigation-in-modals), [UI rules](../../ui/AGENTS.md) |
| Package dependencies or desktop composition | [Crate graph](REFERENCE.md#crate-dependency-graph), [UI hierarchy](REFERENCE.md#ui-component-hierarchy) |
| Host Settings or chat | [Settings](../../settings/AGENTS.md), [Chat](../../chat/AGENTS.md) |

## Maintenance guardrails

### G1 — Responsibility-driven splits

A file is split when its single responsibility breaks down, not when it crosses a line count. Line count is a *signal* to inspect, not a rule to enforce. A cohesive file with one clear responsibility stays as one file regardless of length.

**Real split triggers** (two or more apply → split):

- Two or more domains coexist in the file.
- Methods cluster into groups by naming prefix or shared private helpers.
- Tests require distinct fixtures for different parts of the file.
- Two engineers cannot work in parallel without merge conflicts.
- `tests.rs` exceeds 1.5× the parent `mod.rs` (test-data clustering signal).

**Line-count signals** — review trigger, not a hard cap:

- `mod.rs` ~800 lines / regular `.rs` ~600 lines → inspect whether responsibility is still single. Split only if a real trigger above is also true.

### G2 — Responsibility fences

- `render.rs` is `impl Render` + leaf UI builders only. No `std::process`, `std::fs`, or multi-field string formatting. Add display methods on data structs (e.g. `Pane::display_cwd()`).
- Data modules (`lane/`, `agent/mod.rs`, `main_area/pane_tree.rs`) must not import `gpui::Element`, `Context<Workspace>`, or `Window`.
- `lane/git.rs` stays GPUI-free. UI callers wrap with `background_executor`.
- Display-string helpers live next to the call site, not as data struct methods.

### G3 — 4-point chain rule

Every user-facing affordance must update all four:
1. Type — `actions!()`
2. Handler — `register_action` or `cx.bind_keys`
3. Constant — `surface/keybindings.rs::SHORTCUT_*` or a `locales/en.yml` key
4. Discoverability — `command::palette::PALETTE_ENTRIES` + `surface/action_map.rs` arm

### G4 — Inline-literal ban

| Anti-pattern | Replace with |
|---|---|
| `gpui::white()` / `gpui::black()` etc. | Named const in `daruda_terminal::ux::theme` |
| `hsla(...)` inline | Named const in `theme.rs` |
| `px(N)` inline | Named metric const in `theme.rs` |
| `"daruda"` literal outside `surface/constants.rs` | `surface::constants::*` |

Enforced by `scripts/lint-inline-literals.sh`. When porting from reference implementations, hoist inline literals to theme constants in the same commit — never defer.

### G5 — Persistence compatibility (forward-only)

- New field → `#[serde(default)]` always.
- Renaming → keep both with `#[serde(alias = "old_name")]` for one release.
- Restructuring → add `migrate_legacy()` branch + regression test.

### G6 — Macro discipline

- `actions!()` calls at module entry point (`mod.rs`) only.
- Code-generating macros live in one file; extend the macro instead of adding parallel `match`/`const` arrays.

### G7 — Dependency direction (one-way)

```
main.rs → menus.rs, windows.rs → workspace/
  → title_bar/ → ui/, surface/
  → runs/, agent/, project/, lane/, surface/, config_watcher.rs

runs/ → daruda_store, daruda_flow, daruda_agent, daruda_terminal::pty,
        daruda_control_types::agent_text

project/ → lane/
agent/ → lane/
```

- `surface/` imports nothing from siblings (except `action_map.rs` → workspace action types).
- `lane/` imports nothing from `workspace/`, `project/`, or `agent/`.
- `project/` imports `lane/` only; nothing from `workspace/` or `agent/`.
- `agent/` imports `lane/` only; nothing from `workspace/` or `project/`.
- `runs/` owns the process-wide task log, execution bindings, CLI tracker,
  flow runs in flight, and `RunWatch` result snapshots, delivery ledger and
  waiters, plus the durable run ledger that carries that delivery across
  restarts. Its GPUI-free domain state uses its own `RunKey`, `RunSnapshot`
  and `RunEvent` types. `runs/` never imports `control/`, chat views, or this
  crate's `workspace/`, `project/` or `lane/`, and imports
  `daruda_control_types` only for `agent_text`: answer bounds are
  control-contract policy that run results must share with live chat
  replies. Workspace installs
  `RunsEffects` callbacks for window operations, flow location and run events;
  control converts run-owned types at the MCP boundary and owns orchestrator
  notices. Workspace reads the task projection; only RunsHost writes it.
  `tasks.json` is read-only migration input.
- Control entries carry `control::exec::CommandOrigin`: Phone or Orchestrator
  names the caller, not the person's location. Convert to domain `StartedBy`
  before dispatch; desktop uses `StartedBy::Desktop`. Persist at start, never
  infer later; resume preserves it. Capture solicitation once, as
  `CommandOrigin::capture` -> `Caller`, before approval/async work; a flow
  receives the caller as the run-owned `runs::FlowSubmitter`, never as a
  `(StartedBy, bool)` pair. Follow the
  [delivery and settlement contract](REFERENCE.md#human-delivery-approvals-and-questions).
- The Settings view is `daruda_settings` (`packages/settings/`); its host is
  `workspace/settings_ops.rs`, and the boundary between them is in
  `packages/settings/AGENTS.md`.

When a function references a lane across module boundaries, pass
the full `daruda_store::project::LaneRef { project, lane }` —
plain `LaneId` is only valid inside `Project::*` methods where the
project context is implicit in `self`.

### G9 — Modals go through `gpui_component::Dialog`

- Every modal entity opens via `crate::workspace::dialog_helpers::*` — `open_form_modal` (entity owns full body), `open_single_field_dialog` (one input + OK/Cancel) — or, for a plain confirmation, `crate::ui::dialog::{open_confirm_dialog, confirm_destructive}` (title + body + OK/Cancel; domain-free, so it lives in `daruda_ui`). All wrap `gpui_component::WindowExt::open_dialog`; Dialog handles backdrop, Escape, and outer chrome.
- New modal entity: `impl ModalView` (supertrait of `Render + Focusable`) for entities passed to `open_form_modal`. No `Modal` trait, no `EventEmitter<DismissEvent>`, no `ModalLayer`.
- `ModalView` applies to `open_form_modal` only; `open_confirm_dialog` / `open_alert_dialog` / `open_single_field_dialog` do not use it.
- Focus is automatically restored to the previously focused element when the dialog closes (handled by `gpui_component::Root`).
- Entity owns only the body (fields, validation banner, footer buttons). Never re-render Dialog's chrome (`bg`/`border`/`p`/`rounded`/title) inside the entity.
- Dismiss via `window.close_dialog(cx)`. In `cx.defer` / async paths without a live `&mut Window`, capture `window.window_handle()` and re-enter via `cx.update_window(handle, ...)`.
- Async continuations: never nest two `entity.update` on the same `app_cx`. Run workspace finalize first, then update modal.
- Text input: `Entity<crate::ui::InputState>` rendered via `crate::ui::input(&state, cx, tab)`. IME is handled by `gpui_component`.
- **Modal callback `&mut Window` is live — use it, don't recapture handles**. When a modal's submit/dismiss callback fires with `(…, &mut Window, &mut App)`, that `Window` is the live workspace window inside its own update cycle. Re-entering `cx.update_window(captured_handle, …)` from the callback triggers a *nested* re-entry on the same window and GPUI returns `"window not found"` — the May-2026 add-project regression. Use the callback's `window` directly with `weak_workspace.update(cx, |ws, cx| ws.action(path, window, cx))`. Captured `AnyWindowHandle` is for `cx.defer` continuations where no `&mut Window` is in scope.

### G10 — Debugging policy

Reference: `superpowers:systematic-debugging` skill (Iron Law: no fixes before root cause).

- **Trace before fix** — `cx.update_window` failures, modal-callback hangs, dock not updating, etc. start with `LogWriter::log(Info)` traces at each component boundary (`prompt_and_open_folder_with_policy` → `handle_picked_folder` → `add_path_to_workspace` → `add_project`). Read `~/.daruda/logs/<profile>/daruda-YYYY-MM-DD.log`, grep the dedup tag prefix, identify the layer that breaks. Only then propose a fix.
- **Fix counter ≥ 2 = stop** — if two consecutive fixes have failed, the architecture is wrong, not the patch. Compare against reference (zed, version-matched to the built rev: `~/.cargo/git/checkouts/zed-a70e2ad075855582/decd239/crates/{workspace,gpui}/`). Don't try a third patch in the same vein.
- **Reference comparison when unclear:** when GPUI lifecycle, window-context, or async re-entry behavior is unclear, consult the version-matched zed source before changing the implementation. Relevant examples include `workspace::update_in`, `cx.spawn_in(window, ...)`, and modal patterns.
- **Callback signature check** — when reading or writing a callback, account for **every** parameter. `_window` / `_cx` underscored prefixes are silent acceptances that the parameter is unused. If the parameter is `&mut Window`, suspect that you should be using it instead of capturing a handle.
- **Domain unfamiliar → admit + read references** — "I don't fully understand X" goes in the message, not pretending. Skip ahead only when you can name the reference you checked.

## MVU-Flavored Guiding Rules

Daruda is not strict MVU. These defaults apply to workspace views; an exception
needs a `// SAFETY:`-style comment naming the reason.

- **View purity:** `render()` and its handler closures carry no model-transition
  logic. Closures dispatch in one line, such as
  `weak_ws.update(cx, |ws, cx| ws.method(args, cx))`. Same model means the same
  screen. Layout-geometry caching inside `canvas()` (bounds, hitboxes, scroll
  offsets) is allowed because GPUI requires it; model changes are not.
- **One-way data flow:** views dispatch; Workspace and `*_ops.rs` own model
  changes (`add`, `remove`, `set`, `insert`, `delete`, `clear`, `toggle`,
  `update`, `open`, `close`). A view does not directly write another entity.
- **One source of truth:** mirrored state has exactly one update site.
  Extend that entry point rather than adding a parallel sync path.

## State Modeling

- Prefer an owning strategy over multiplying `if` branches for quick fixes.
  Keep each type responsible for one reason to change.
- Use existing abstractions rather than bypassing them with direct calls.
- A bool plus conditional data becomes `enum { Inactive, Active { data } }`.
- Replace flag combinations with valid enum states instead of
  `match (a, b) { ... _ => unreachable!() }`.
- Fields always `Some` or `None` together become `Option<(A, B)>` or one struct.
- Repeated multi-field transitions go through one function; seal the fields
  with `pub(super)` rather than repeating the same sequence in call paths.

## Tab Groups

A worktree has two tab groups: its tabs (`LaneRuntime::tabs`, a `TabStrip` in
`main_area/tab_strip.rs`, each tab a split tree) and its side tabs
(`LaneRuntime::side`, `main_area/side_group.rs`), drawn to their right. A side tab is one pane; nothing splits there. Only what
is read beside a chat goes aside: files, diffs and a pull request
(`side_hosts`). Both groups' panes live in `LaneRuntime::panes`.

- **A strip's order and its shown tab change through the strip.** `TabStrip`
  reads as its tabs in order; `push` adds a tab, `open` adds and shows it,
  `remove`, `activate`, `insert` and `move_tab` do the rest, on any worktree's
  runtime. The tabs shown before are kept by id, so nothing is rebased.
- **`focused_pane_id` is always a pane of the active tab**, or none when the
  worktree has no tab. A side pane never becomes it. Splitting and the bottom
  input act on it.
- **Which group the keyboard is in** is `LaneRuntime::keyboard`
  (`KeyboardIn`), written only by `Workspace::focus_pane`. Read it through
  `keyboard_is_aside()`; it is in the side tabs only while they are drawn.
- **Commands about what the user is looking at** (find, save, close, send to
  chat, the focused file view) use `Workspace::keyboard_pane()`, not
  `focused_pane_id`. A panel row that opened a file without moving the
  keyboard uses `shown_file_view()`.
- **Where a pane sits** is `LaneRuntime::place_of(pane) -> PanePlace`. Do not
  search `tabs` alone for a pane that may be a file or a pull request.
- **What is drawn** is `groups_shown()`: a page and an unavailable worktree
  hide the side tabs, a zoom draws one group alone, and a width too narrow for
  both draws the group the keyboard is in.
- **What is zoomed** is `zoom_scope()`: a pane of the tab on show
  (`MainAreaContext::zoomed_pane`, which may name a pane that is not on show)
  or the side tabs (`SideGroup::is_zoomed`). Do not check a zoom against the
  tabs anywhere else. The zoom command acts on `keyboard_pane()`; the tabs
  taking the keyboard give the area back. Pane sizes follow `groups_layout()`, which ignores
  what covers the groups, so a zoom or a page resizes nothing.
- **One open path:** `open_file_pane` gathers the facts and carries out the
  plan: the group (the keyboard already aside, or `file_viewer.open_beside`
  with work shown on the left: a shell or a chat, not files alone) and the
  reuse (`preview_slot`, one scratch tab across both groups). A pull request,
  which only the side tabs hold, needs any tab on the left
  (`side_only_goes_beside`).
- **Where a dragged tab lands** is `plan_tab_drop` (`main_area/tab_drop_plan.rs`),
  for the insertion line and for the drop alike: a drop that would be refused
  claims no slot and draws nothing. The slot a drag is over names its strip
  (`DropSlot`); a strip uses only a slot of its own, else its end. The drag's
  transient state is `MainAreaContext::tab_drag`, taken down by
  `finish_tab_drag` alone.
- **A close among the tabs leaves the keyboard in a side tab** when it is
  there (`refocus_tabs_after_close`): the tabs' focused pane is updated, the
  keyboard is not taken.
- **A pull request pane** (`PaneContent::PullRequest`,
  `pages/pull_requests/pane.rs`) holds one of the window's open details
  (`PrDetails`, `pages/pull_requests/details.rs`) through a `DetailHold` it
  owns. The page showing the same pull request holds the same detail. A detail
  is over when its last hold is dropped, so no path that removes a pane has to
  release it. What an ended detail leaves (its review place, its drafts on
  disk) is settled by `Workspace::settle_pr_details` alone. Code given a
  detail's id finds it with `pr_detail(id)`, never through the one the page
  shows. A worktree has one such pane, retargeted by
  the next pull request. The layout keeps only which pull request it is:
  restored, the pane is `PrPaneDetail::Unread` until its worktree is shown
  and the pull request is read again, and a read lands only on a pane still
  waiting for that pull request.
- **Where an open goes** is decided by `main_area/open_plan.rs` from plain
  facts (`plan_open`, `GroupFacts::goes_beside`); the workspace only carries
  the plan out. A new rule is a row in its table of cases. The plan does not
  predict what a close leaves behind: `OpenPlan::Replace` closes the scratch
  tab and asks `goes_beside` again from the facts that are then true.
- **A pane's id** is `PaneId`, minted from 1: the default names no pane.
- **What is true of every pane kind** (title, focus handle, cwd, unsaved
  edits, running work, account, rail icon, which group hosts it) is
  `PaneBehavior` (`main_area/pane/behavior.rs`), one `impl` per kind with
  defaults for what does not apply. Ask through `pane.content.behavior()` or
  the `Pane` method that wraps it; reach one kind's own data with
  `as_terminal()` / `as_file()` / `as_agent_chat()` / `as_pull_request()`.
  Match on `PaneContent` only where kinds must differ in what they draw or
  save, so the compiler asks about a new kind exactly there.

## Error Reporting

New `eprintln!` is forbidden. Workspace failures go through
`self.report_error(report, cx)`: toast, details modal, and NDJSON log.
GPUI-free or pre-Workspace failures use `LogWriter::log(report)`.

GPUI re-entry (`cx.update_window`, `cx.update`, `entity.update`,
`entity.update_in`) returns a Result because its target can have closed.
Never swallow it with bare `let _ =`. Accepted forms:

- Match `Ok` / `Err` and report the error.
- Propagate with `?` when the enclosing function returns a Result.
- Use `crate::windows::try_update_workspace_window(handle, cx, "site_label", ...)`
  to log failures with a site label.
- An intentional ignore has `// SILENT-OK: <concrete reason>` on the preceding
  line. This is reserved for inconsequential failures such as restoring focus
  to a possibly closed window or a test fixture; an empty reason is invalid.

Enforcement: [update-result lint](../../../scripts/lint-no-silent-update.sh).
References: `workspace/error_ops.rs` and
`packages/store/src/observability/`. Persistence is deferred through
`mark_dirty_and_save`, never direct `persist_state` during an entity update.
See [GPUI render safety](../../ui/AGENTS.md#gpui-render-and-event-safety).
