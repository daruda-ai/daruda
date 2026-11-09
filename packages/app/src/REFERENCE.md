# App Reference

Maintained layout, subsystem contracts, and extension recipes. Read the
[App Instructions](AGENTS.md) for workflow and safety rules; use the relevant
sections below when changing a subsystem.

## Directory layout

```
app/src/
├── (top-level)           # App entry, window/menu lifecycle, PTY, config watcher, slot actions
├── startup/              # Desktop startup, the quit hook, optional replay/capture/smoke flows
├── agent/                # Agent-side data models — MCP, skills — plus GPUI-free account + ACP launch resolution
├── runs/                 # Process-wide task log, live flows, result snapshots, delivery ledger, and host-installed effects
├── surface/              # App-shell constants — name, shortcuts, strings, keybinding action map
├── title_bar/            # Window chrome — who draws the frame (GPUI-free `policy.rs`), the drag strip, the app-drawn caption controls, the application-menu button
├── ui/                   # Facade over `daruda_ui` (packages/ui/) + localized and agent-domain widgets
└── workspace/            # Workspace entity — projects, tabs, panes, docks
    ├── command/          # Command palette + history picker
    ├── group_ops.rs      # Group CRUD (add/rename/recolor/collapse/delete + move_project_to_group)
    ├── project_ops/      # Project CRUD (add/close/delete-on-disk/rename + window_open_policy)
    ├── project_palette_ops.rs  # Palette handlers: New Group / Rename Project / Move Project to Group…
    ├── layout/           # Dock entities (left/right/bottom), drag/toggle ops, snapshots
    ├── left_dock/        # Left-dock views — lanes/"Worktrees" (2-level tree), git changes, files
    ├── main_area/        # TabBar + PaneTree runtime
    │   ├── bottom_dock/  # Macro grid, terminal input, tab strip
    │   └── file_view_pane/  # File-viewer data + renderers
    ├── pages/            # Workspace-level Tasks / Flows pages, beside the lanes
    │   ├── tasks/editor/ # The Task editor — the Tasks page's detail
    │   └── flows/        # The Flows list, and its detail — a flow's graph or a past run's report
    ├── render/           # GPUI render — `impl Render` for Workspace
    ├── right_dock/       # Right-dock views — usage, skills, tasks, tools
    ├── sync/             # Background pumps — PTY, limits, MCP, skills watchers
    ├── toast_layer/      # Toast overlay entity rendered above workspace
    └── tests/            # Lifecycle + pure-op tests
```

`crate::project` and `crate::lane` are re-exports of `daruda_project`
(`packages/project/`), which owns the runtime Project/Lane model and the
GPUI-free git CLI wrappers.

## Top-level (`app/src/*.rs`)

`main()` delegates to `startup::run`: helper commands exit before environment
hydration or GUI initialization, desktop cleanup exits before instance ownership,
and the desktop flow initializes the first window before scheduling inspection.

App-shell glue — process entry, native menu bar + Open Recent, window lifecycle (every window is a Workspace, wrapped in `gpui_component::Root`, with double-open guards), live config-reload watcher, PTY spawn + I/O threads, and tab/lane slot-action macros (`tab_slot_table!` / `lane_slot_table!` — the latter generates `ActivateLane*` actions, displayed as "Activate Worktree N" in the menu).

## Layered config (user → project)

1. **Defaults** — `Config::default()`
2. **User** — `<config_dir>/daruda/config.toml`
3. **Project** — `<config_dir>/daruda/projects/<basename>-<fnv1a_hash>/config.toml`

`Config::resolve(&ProjectConfig)` merges (project section replaces user section wholesale). `Workspace::reload_config` applies at runtime; `new_with_project` applies at construction. Status bar shows a dot when the project layer exists.

## `agent/`

Agent-side data models for the right dock — MCP servers and skills. Each submodule splits responsibility into a GPUI-free state core (state struct, snapshot, disk scan, persistence writers, parsers) and a sibling GPUI Global wrapper. Personal/plugin layers are user-global; project layers are keyed per-lane.

Not every submodule is a right-dock model with a Global wrapper: `account.rs` (`PreparedAccount`) and `launch_resolve.rs` (`resolve_launch` and its pure helpers) are a GPUI-free pair resolving what an ACP launch needs to spawn — command line, working directory, env, and session-host write-back — shared by the agent-chat connect pump and the shell-pane spawn funnel. `adapter_cli.rs` prepares an agent CLI run (login, auth status) through the same rules and installer, so it runs the adapter a session would.

## `runs/`

`RunsHost` owns the process-wide `TaskStore`. User commands enter through
`TaskIntent`; observed execution facts are proposed as `TaskEvent`. Each reply
follows its durable commit and foreground projection update. `GlobalTasks`
exposes the resulting `TasksBook` through read-only accessors, and windows
observe it instead of reloading task files. The desktop installs `RunsEffects`
callbacks for pane operations; this module does not depend on Workspace.
Execution bindings are process-owned and identify both workspace and pane.
The process owns one CLI tracker and event pump; closing a window removes its
pane registrations but retains observed processes until their exit is confirmed.
Each spawned terminal also registers a fresh hook terminal ID. Hook events
resolve that ID through `RunsHost` before the owning workspace binds the
session; a missing agent PID does not prevent routing or session attachment.
Every Task starts as an Agent chat or a flow; no start creates a Claude CLI
run. CLI runs already in the log keep their `LanePrepared` terminal ID (schema
7; older preparation records without it still replay). That terminal belongs
to an earlier launch whose routed status files the first desktop startup
deletes, so only an unrouted session-keyed hook, an observed PID exit, or
startup reconciliation settles such a run. Their chats open read-only.
No pane is ever bound to a CLI run, so a hook reaches one only through the
session it already recorded. The task log uses schema 9: task specs and edits
carry no execution surface or auto-execute flag, and the copies older logs,
snapshots and `tasks.json` hold are ignored on read.
Only observed PIDs advance CLI process state. The tracker polls those PIDs;
it has no pane discovery, process-tree walk or session-directory watcher.
On restart, an already recorded Claude PID is verified against its account's
single session file. No other PID is discovered by that check.
A confirmed exit marks a still-running run interrupted by the restart; a
completed run or pending result read keeps its own outcome.
Routed status and lock files use terminal IDs, while session IDs remain the
run attachment key. Session replacement retires the old in-memory status;
SessionEnd retains a final marker and removes the live session from memory,
so a session ending before the watcher's first read is still seen to end.
The next SessionStart overwrites that marker. Pane teardown clears memory, and the first
desktop startup sweeps retired terminal files without unlinking lock paths.
Forwarded interrupt keys clear a busy terminal locally without a completion.
The JSONL module only reads execution answers; it supplies no status fallback.
The in-memory status identity is `StatusKey::Terminal(TerminalId)` or
`StatusKey::LegacySession`; lookup never guesses between those namespaces.
Conversation queries return all observations, or one only when unambiguous.
Turn-completion and notification deduplication use the same typed identity.
The workspace hook receiver owns file ingestion and pane observation. Task-specific hook
consumption lives in `right_dock/task_hook_ops.rs`, with execution facts owned
by `RunsHost::record_task_cli_hook`. Shared `HookEnd` semantics are interpreted
by the GPUI-free `runs/cli/hook_policy.rs` for both live and PID-exit recovery.
Task-independent terminal history lives in `daruda_store::terminal_sessions`
(schema 1), separate from the task log. Each terminal has a runlog below the
profile-routed `status/history/` directory. Terminal journals compact after
64 records to bound per-hook replay; snapshots retain every execution and its
latest observation. The shared runlog default and byte limit keep their policy.
Recovery logs identify each failed terminal and its redacted journal path.
Hook processes commit observations
under the terminal's stable status lock before publishing the live status file;
watcher coalescing cannot replace an execution's history. Journals retain
provider/configuration-home/session conversation identity and numbered physical
executions, plus transcript references and PID observations, without tool inputs
or transcript copies. `runs/cli/history.rs` serializes desktop context and
teardown facts off the UI thread. Context includes workspace, full LaneRef,
account selection and initial cwd. Pane deletion keeps these journals; first
startup captures old terminal identities and reconciles them off the UI thread
as `ObservationLost`, which does not prove process exit. `hooks::history::{terminals, read}` provides discovery
without a live pane or task. History does not imply resume eligibility and does
not launch agents. Existing task history and ACP Continue keep their own policy.

`CliSessionEnded` retains the original terminal's end reason in the current
execution and its ended-history snapshot: agent goodbye, observed process exit,
pane closure, or restart reconciliation. A delayed goodbye can refine a process
exit without replacing a deliberate pane closure or restart reason.
CLI history continues through an exact ACP session load after confirmed exit.
RunsHost grants one process-wide session lease, held by the destination pane
through loading and interactive use. Failed loads, ended streams, session resets,
and pane teardown release it. Other windows' routed sessions also block takeover
when no PID was available. Restored interactive CLI conversations claim the same
lease before connecting; snapshots remain read-only until load succeeds.
`TaskStore` imports an existing `tasks.json` once and never rewrites it.
Task commands target only the task log; older builds read the preserved,
potentially stale legacy file. `startup/quit.rs` registers the single quit
hook: it signals flows, starts phone card edits, flushes the task and
terminal-history queues, then waits on the cards and the flows, all within
one deadline (`runs::QUIT_FLUSH_DEADLINE`).

`RunsHost` also owns every flow run in flight (`runs/flows/`), keyed by
worktree: its thread, stop switch, and one pump that folds the run's
notices and questions into a `RunView`. Open questions are a set keyed by
the log's ask id, and node states are checked against the run log at the
run's start and end. Windows read the view through the lanes they hold and
are told of changes through `RunsEffects::flow_changed`, deferred past the
update that made them; only windows holding the worktree repaint, and one of
them speaks for the run's end. Closing a window leaves its runs going.

### Execution results and provenance

`runs/result.rs` defines the run-owned `RunKey`, `RunSnapshot`, progress,
waiting state, end reason and artifact pointers. `control/mcp/runs.rs` resolves
external handles and converts these types to `daruda_control_types`; `runs/`
does not import `control/`, the control protocol or chat views, except
`daruda_control_types::agent_text`: answer bounds are control-contract policy
that run results share with live chat replies. Workspace installs
`RunsEffects::locate_flow_run`, returning `FlowRunLocation { run_dir, lock_dir }`,
and `RunsEffects::run_event`, receiving the run-owned `RunEvent`.

Phone and MCP starts carry `control::exec::CommandOrigin::{Phone, Orchestrator}`
into each domain's own `StartedBy`. `Orchestrator` names the tool caller,
not the person's location. Desktop starts pass `StartedBy::Desktop`
(a flow, `FlowSubmitter::Desktop`) directly. Flow `RunStarted` and Task `DispatchRequested` persist this provenance;
resume and Task reopen preserve it. Start responses construct handles from the
actual returned run id or `ExecutionRef`: `flow:<run-id>` or
`task:<task-id>/<execution-id>`. `task:<task-id>` resolves the latest execution
once per result call. `control::mcp::runs::{run_handle, run_key}` is the one
conversion between a `RunKey` and its handle; notices print the handle's
`Display`, and `runs/` orders keys with `RunKey`'s own `Ord`. An optional
`daruda_flow_stop.run` rejects a mismatched current run instead of stopping a
replacement in the same worktree.

Solicitation is separate, process-local metadata captured when a request enters,
before approval or asynchronous preparation: `CommandOrigin::capture` yields a
`control::exec::Caller` (`Phone`, or `Orchestrator { solicited }`, solicited
while the orchestrator's phone turn is open), and the command carries that
value, never an origin beside a bool. `Caller::scope_run` is the one place an
orchestrator start or resume scopes its run key with that solicitation. A flow
episode is submitted as the run-owned `runs::FlowSubmitter` (`Desktop`,
`Phone`, `Orchestrator { solicited }`): its questions use `solicited()`, the
run key takes `solicitation()` (`Desktop` keeps the key's), and only `Phone`
is owed the end as a remote answer. A flow-backed Task's run submits as the
Task start's submitter (`Workspace::begin_task_start_for`), so an
orchestrator's Task started during a phone turn asks its questions on the
phone; a start that names only a `StartedBy` is unsolicited
(`FlowSubmitter::of_task`). Task registration installs the
solicitation before replaying a fast cached completion. Every `RunEvent` stamps
its own solicitation so a desktop resume cannot relabel an earlier held end.

`RunsHost::run_query` stages foreground facts; `RunQuery::read` performs disk
reads and report enrichment off-thread. Flow state comes from the live run
first, then a retained ended snapshot, then strict read-only run-log inspection
at a known or host-located directory. The process's `FlowEnded` remains
authoritative even if writing its log or report failed; `ShutDown` alone is
not an end. Task state comes from the task book and execution history, with
retained ended snapshots as a fallback. Unknown flow location yields `Unknown`,
a missing known directory or Task execution yields `Gone`, and failed log
inspection yields `Unreadable` with a reason. Ended results include bounded
text and pointers to reports, outputs, sessions or CLI transcripts when known.
`what` names the flow or Task; `why` carries the Task prompt prefix or flow
`until` and pinned-node context. Resume metadata carries the original pinned
selection for this description without applying the outputs as new pins.
A flow's `what`, `why` and output pointers come from one
`runs::flow_description::FlowDescription`, built from a submission's facts
and from a run directory alike: the flow's name, else the stem of the source
its log recorded (`SourceRecorded`), else the run id. A live end and the same
end read after a restart therefore compare equal.

Only `TaskEvent::Settled` carries a textual result. AgentChat captures its final
answer at `fire_activity_completion`, after activity has settled; CLI settling
marks the execution while a background reader finds the last assistant text
in a bounded transcript tail. The execution is rechecked before commit, and
missing or malformed transcripts yield no text. Text preserves up to 2,000
leading and 2,000 trailing characters around a truncation marker. Every terminal
path records execution identity, provenance, end state and pointers in the
Task's five-entry history; reopen retains that history. `Completed` describes
normal Task session termination, so work success must be judged from the result.

### Human delivery, approvals and questions

`remote_channel::delivery::remote_delivery(solicited, away)` maps solicited
work to `Delivery::Explicit`; other work uses `Delivery::Presence { away }`
and each channel's `only_when_away`. Telegram honors the same delivery policy
as Slack and Discord. Solicitation is captured before approval or asynchronous
work; presence is read when the message is actually offered. `control_say`
opens a `PhoneTurn` only for Phone input; Orchestrator input uses InApp with
the same slash-command and clear handling. A solicited chat announcement
selects the chat and records its ping. An unsolicited announcement is a Notice,
which changes neither selection nor reply-to/last-ping routing.
`Delivery::reaches(only_when_away)` is the one test of whether a delivery
reaches a channel; each channel applies it to its own live setting. Hosts send
through `remote_channel::send_ping` / `send_notice`, which fan out to Telegram,
Slack and Discord and report whether any channel accepted the message. A
permission or question is marked told to the phone only when one did; each
channel's card ledger separately keeps one card per identity.

The remote-channel facade fans out Flow questions and card settlement across
Telegram, Slack and Discord. Card ledgers belong to a remote identity (the bot
and its recipient), not a session: reconnects, inbox recovery, token revisions
and same-bot credential rewrites keep sent cards editable and are not
re-offered. Up to 64 settlement receipts per identity survive an unavailable
editor or failed edit; they retry on reconnection and every 15 seconds until
successful or evicted. An edit the platform refuses for good (message, chat or
bot access gone, or a malformed request) is dropped and logged once instead.
A different bot, or Telegram unpairing or disabling, drops the ledger, pending
edits and callback tombstones with its live tokens; a waiting approval is then
offered once to the new identity. The 64 most recent settled callbacks
identify taps even before a send returns its message ids; the 64 most recent
settled message ids prevent competing stale edits. A stale tap on an untracked
message loses its buttons. Workspace only updates its pane-local permission
presentation.

An agent's question reaches the phone one field per card
(`CardId::Question { pane, question, step }`), offered at arrival and by the
same level-triggered sweep as a permission wait. A tap, or a text reply to the
card, answers the field the card names (`respond_bot_question`), never the
desktop's focus, composer binding or uncommitted text. An optional field's
card can leave just that field blank ("Skip this one"); "Skip all" declines
the form. A taken answer retires the question's cards and sends the next
field's; the last one sends the form through the desktop's gate. A reply to
a retired card is refused as stale. The view's single settle path makes the first
answer win, and `AgentChatEvent::QuestionSettled` edits every card left with
how it ended, naming the desktop when it answered first. A phone-started turn
keeps its question for the phone; a task run the orchestrator or phone started
still declines it.

A chat opened at the phone's or the orchestrator's request — `task_start`,
`task_open` (when it opens a new pane), `chat_new`, `worktree_create`, and a
phone `/task` — runs in its agent's `delegated_mode`
(`daruda_config::effective_delegated_mode`; unset means the policy default,
Claude `bypassPermissions` and Codex `agent-full-access`, else the agent's
`default_mode`). A desktop Start or menu chat, the orchestrator's own pane
and flows use `default_mode` or the flow's mode. The split is one function,
`workspace::mode_for(StartedBy)`, which both the op and its card read. The
pane persists the result as `ModeChoice` (`FollowDefault`,
`FollowDelegated`, `Picked`), set before its first connect; a live config
edit moves a following session to the new mode, and a person's pick stands.
The agent is resolved when the card is built and pinned as
`ChatAgent::Pinned`; the pane open resolves it again
(`Workspace::resolve_chat_agent`), so an agent removed meanwhile is refused as
`agent_unavailable` rather than replaced. Found after git, the worktree is
kept as a lane with nothing open and the refusal is
`agent_unavailable_worktree_kept`, carrying its handle, so the caller can
open a chat there rather than retry a branch name git now refuses; it spends
the agent's worktree budget like a success. A recorded task start fails as
`AgentMissing` either way. A card discloses the agent, the requested mode,
the task title and the prompt through `agent_text`: titles and prompts are
flattened to one line, their quote marks neutralized, a long one shows its
start and end around the count it leaves out, and the invisible characters
dropped (bidi, zero-width, tag) are counted, since the agent still receives
them. A mode name the adapter advertised that daruda's seed does not match
is shown beside the mode id. Phone `/task` rows quote titles the same way.
A flow task's start card quotes the prompt too unless the flow was found to
declare no input for it (`TaskFlowApproval::takes_prompt`); like a chat
task's announcement, the phone `/task` notice names the flow and its modes,
not the prompt, since its sender chose the start.
A card for a chat already open names no mode, since that pane keeps its
own.

`control/approval` owns the process-wide pending decision. The orchestrator's
host-owned card strip remains answerable without a remote channel. Desktop and
eligible remote cards share the first answer; settlement edits remote cards and
defers the desktop update, notifying cached views and all window chips. Stop,
MCP cancellation and disconnect withdraw outstanding approvals. The initial
deadline is 300 seconds; a freshly queued or delivered phone card extends it to
the later of the current deadline and that moment plus 120 seconds. A 15-second
presence pump offers each outstanding approval once to each owner/channel that
has never queued it, keeping a solicited request explicit and re-reading
presence for the rest. Settled cards name the surface that answered (desktop,
Telegram, Slack or Discord). Timeout means no answer, never consent or
permission to retry or bypass the gate. Remote card ledgers retain settlement
while a send is in flight, then edit its returned message ids. Transports
report each send as delivered (message ids), unconfirmed (it left but no
receipt came back, which includes every transport error because none
distinguishes a refusal from a lost reply) or not sent (the session was gone
before it left). The ledger classifies the completion as ignored, delivered,
unconfirmed, released or requiring an edit. An unconfirmed card keeps its
ledger entry and live tokens, so the pump does not offer it again and a tap on
it still answers; settling it forgets the tokens without a tombstone, since
no edit will own its message, so a later stale tap strips the buttons unless
an earlier tap on that message answered it. A released card drops its claim
and tokens together and may be offered again. The common coordinator extends
approval deadlines only for a fresh delivery. Duplicate or retired completions neither extend deadlines nor discard
current buttons. Each remote identity's `cards::CardDesk` owns its card ledger
and callback tokens together: a released or unreceipted card forgets its
tokens, a settled card leaves bounded tombstones, and a change of identity
drops both. Live tokens are bounded at 64 per card kind (approval, flow
question, permission, question), oldest evicted first. Transports keep their
own receipts and message-edit implementations.

`permission = ask` flows can start through control; profile selection is still
refused. Runtime questions appear in desktop modals or inline flow rows and on
eligible remote channels with every original choice. Tokens identify the full
`run_dir` and `ask_id`; a fresh directory can restart ids, while resume in the
same directory retains the durable monotonic counter. The first answer wins (a
later one is refused as already answered). The bounded settled callback table
keeps late flow taps routable to their original question after its live tokens
are removed, and `FlowChanged::AskClosed` settles
remote cards and closes stale desktop questions. Modal Stop also checks that
its original question is still open. Process-owned dispatch can deliver and
answer questions without a window. Waiting state updates chips in all windows
where the orchestrator feature is on, including non-host windows. Clicking a
non-host chip reveals the host's tab and activates that window; switching the
feature off hides chips even while a session exists. Native waiting notifications obey
preferences and visibility without stealing focus.

App quit cleans up phone cards best-effort within the quit deadline
(`remote_channel::cards::quit`). Every card a serving identity can still edit
is closed: a queued settlement edit keeps its result, and a delivered,
unanswered card reads "Cancelled — the app quit" without buttons. The
foreground blocks during quit, so the snapshot is taken first and the edits
run on their own threads; edits still running at the deadline are logged once
with their count. Cards without a receipt (still sending or unconfirmed), and
cards of an identity not serving at quit, are left. Those, and every card after
a crash or forced kill, settle only when tapped: a new process cannot tell a
restart from an earlier answer, so the tap reads "No longer valid — handled
already or the app restarted". Removing settled OS alerts and restart
durability remain outside this contract.

### Result queries, waits and delivery

The orchestrator's MCP tools include `daruda_run_status` and
`daruda_run_wait`. A gated tool's approval terms come from its `Gate`
(`Tool::described`), and the briefing names the gated tools and the wait cap
from the tool table, so neither restates them. These result tools are
MCP-only; phone commands retain their existing surface. Status reads progress
or results without consuming events.
Both tools accept explicit handles from any origin, including older persisted
runs. Omitting handles selects successful orchestrator starts in this process
plus unfinished orchestrator Task executions captured before startup
reconciliation, including Running CLI executions, plus the runs the durable
run ledger restores. Other already-terminal Tasks and flows from earlier
processes do not enter that omitted scope. Older flows remain explicitly
queryable, and restored runs with no recorded end are judged at startup (below).

`RunWatch`, inside `RunsHost`, owns that process-wide scope, snapshots, waiters
and delivery ledger. It survives orchestrator pane or session replacement and
gate-token rotation. Each end or waiting episode has a `(RunKey, sequence)` identity.

The run ledger (`daruda_store::run_ledger`, `<data_dir>/run-ledger/`, written
through `runs/ledger.rs` and taken back by `runs/ledger/restore.rs`) keeps that scope and its unreported ends across a
restart. Each entry is `Open { owed }` (running), `Unreported { end, owed }` or
`Reported { end, owed }`; `owed` lists every earlier end (from before the run
resumed) that no agent was handed, oldest first, so a report can never precede
its end. Schema 4 snapshots kept one owed end (`null` or an end) and read as a
list of at most one; an `owed` of any other shape fails replay with an error
naming the owed ends' accepted shapes and the value found. The watch queues `Owned` when a key joins the scope,
`Reopened` when a scoped run resumes (an unreported end joins `owed`), `Ended` with a ledger-wide episode
for each scoped end (again when its text is read), and, after a socket
receipt, one `Reported` per recorded end it handed over. A report marks only
the end with its episode, current or owed, so an explicit wait that took a
run's newer end leaves the owed one owed. Once per key after the receipt,
`Released` follows only when the key's run is over (its current end reported
and not a Task's flow interruption, or the run gone), and none of its recorded ends is still unreported or
in flight. A report of an episode the key no longer has (released, evicted or
resumed past) is a stale no-op; only one no end has used is refused. Logs
written before reports were per end (schema 3 and older) replay the same way,
so an end an older build reported as covered stays owed and is delivered once
more after an upgrade. In-flight
reservations are never written, so a result whose receipt never came is
unreported after a restart. Writes are queued without waiting and flushed at
quit; open or write failures leave delivery memory-only with one deduplicated
warning and never affect tasks, and the committed directory holds stay in
force. A refused record is logged as an error (`run_ledger.rejected`) and the
ledger keeps writing. The ledger compacts itself once its log passes the
runlog's budget.
At most 200 unreported ends, owed ones included, are kept ledger-wide; a newer
end evicts the oldest, its own key's earlier ends too, and the `Ended` record
names each evicted end's key once per end. An eviction drops only that end: a
run that is open, or whose current end was reported, stays owned and an open
flow's directory stays held; a run whose only unreported end was its current
one is let go. Each evicting commit logs one warning
(`run_ledger.evicted`) with counts by kind, never run ids. `Ended` carries the proposer's clock
(`ended_at_ms`), so replay never reads one. A re-record never moves a date: the
app re-records read text with none, and a re-record that carries one only dates
an undated end. Ends from logs written before ends were dated are dated by the
first restore that keeps them, so their directory hold runs from that restore.
Retention spares what the ledger still owes: `RunsHost::start_flow` fills
`RunRequest::held_runs` from the committed ledger, and the engine's start sweep
(`marker::sweep_old_runs`) neither deletes nor counts those run directories.
A flow is held while it is owned with no end, or while any unreported end is
undated (only until the restore dates it) or under `DIRECTORY_HOLD` (7 days)
old, judged against the clock at each start; nothing is written when a hold
lapses. Another profile's sweep cannot read this ledger, so each dated hold of
a flow whose location the ledger records is mirrored into the run directory as
a `HELD` file holding the deadline in Unix milliseconds
(`marker::hold`/`release_hold`, through `runs::run_ledger_hold_markers`). The
ledger's writer thread writes and removes it after each commit that changes
the hold, in commit order; `sweep_old_runs` in any profile spares a run whose
`HELD` deadline has not passed. An undated hold (an open run) writes none: an
open run has no outcome marker, so no sweep takes it. An unreadable deadline
holds nothing, and a crash between a commit and its marker write leaves the
marker to the hold's next change, so a marker never pins a run past its deadline. A lapsed hold frees only the
directory: the end stays in the ledger until reported, and a release
or eviction ends the hold. A restored flow open longer than `DIRECTORY_HOLD`
(dated by `Owned`/`Reopened`) that the startup read still cannot judge is
released `Expired` with one warning (`run_ledger.unjudgeable`), deleting
nothing. Task ends are kept by the ledger alone. At install, before task history is
compared, restored keys rejoin the scope with their saved solicitation (a
carried Task takes it from there), and unreported ends, owed ones included,
return as `Recovered` episodes, as do the ends task history adds for a restored
or carried key, such as a carried Task startup reconciliation settled. A
reopened entry is judged from disk like any open one. An entry whose latest end
was already reported with nothing owed (a crash came before its `Released`) is
released, unless it is a Task's flow interruption whose execution the log still
holds: the Task's own end is still to come. A Task whose execution is gone with
nothing to deliver is released too; one that still owes ends rejoins as gone
(`RestoredEnd::Gone`): its owed ends are delivered as `Recovered` episodes, no
wait waits on it, and it is released once they are reported. When the task log cannot be read, no Task
entry is judged or released; an open Task that owes an end rejoins with that end
as a `Recovered` episode and its run read as unknown, so no wait waits on it. Recovered results carry
`recovered: true` in `daruda_run_wait`/`daruda_run_status` (per delivered
episode, so a live end of the same key in one answer is not marked), and their
notice line says they were requested before daruda restarted.
A restored open entry, or a carried key with nothing observed, is marked
unjudged on its `RunEntry` (`Judgement::Pending`). Its first read judges it,
whichever path made it: an end settles as a `Recovered` episode (so a wait or
status read that lands before the startup read still marks it `recovered` and
fires the auto-start), a gone run is released as below, and any other state
releases a lapsed flow. A start in this process clears the mark.
After install, each unjudged key is read once
off the UI thread (`RunsHost::judge_restored`), through `run_query`'s own disk
readers; a key that changed meanwhile, or that another read judged, is skipped.
A start proposes its records before its run writes anything: `start_flow`
records `Reopened` for a scoped key, then spawns the run after the caller's
update (so an orchestrator caller's `Owned` is proposed first), and the run
thread waits up to `LEDGER_BEFORE_RUN` (2 s) for every earlier ledger record
to be durable, logging `run_ledger.before_run` if it starts without. When a
restored run's first read is an end equal to its latest owed end (kind, what,
why and pointers; read text is ignored), the resume that superseded that end
never ran: no episode is opened, and the owed end is the run's current,
resumable end again. A flow ended on disk is
delivered with its report as a `Recovered` episode. A flow `marker::run_status`
reads as `Crashed` (a quit leaves its lock with a dead holder) or `Stalled` is
an end too, with `resumable: true` on the wire, `resumable` in its notice, and,
without a report, fixed English text naming how many nodes passed. Only an end
no later episode superseded carries the hint (`RunEvent::resumable`): a crash
still owed after a resume is delivered without it.
`daruda_flow_resume` takes the handle of any result marked resumable: a flow's,
or a Task's, which names the flow run its current execution runs as. A Task
owns the run only while it is `Running` (its flow was interrupted); a Task
handle of a Task that ended is refused as `flow_run_not_resumable` with
`ended`, and a flow handle then resumes under the flow's own key. One function
picks the owner: a key already in the scope keeps the run (the running Task's
first), otherwise the key the handle named; an orchestrator resume scopes only
that owner, so one run never ends under two keys. The handle is resolved once,
before the card, into the run and the execution it names (`TaskLatest` cannot
name another execution after the card); after approval only that run's
resumability and the Task's lifecycle are checked again, and the owner is
picked against the scope as it then stands. It finds the run through
`RunsHost::flow_location` (the watch's recorded location, else
`RunsEffects::locate_flow_run`, the lookup result queries use) and the window
whose lane keeps its directory. The run is read once before the card into the
plan the card quotes (passed nodes come from `resume::prepare`'s replay; all
passed has its own wording), and once more after approval, since the worktree
can change while the card waits; that read (`Workspace::read_flow_resume`,
which resolves the lane's tree and lock once, as the Flows panel's Continue
does) is the one the resume request is built from. Refusals no answer can
change come before the card: `flow_run_not_found`, `flow_locked` naming the
holder (checked first through `lane_busy`, the check a start makes, since
another run's lock hides this run's status; a live holder of this same run is
`running` instead), `flow_not_started` with `lane_unresolvable` for a worktree
whose folder no longer resolves, and `flow_run_not_resumable` with `running`,
`ended`, `never_started` or `unknown`. A profile is never asked for:
`run.yaml` already folds the start's profile in. The approved resume goes
through the Flows panel's `submit_flow_resume`, submitted by `Orchestrator`
with the solicitation captured before the card; the log keeps its first
starter, and the resumed episode's end is delivered like any other. A flow whose
holder is alive stays pending for wait refreshes; a missing run directory is
retained as `Gone` and released `Expired`, or, while the flow still owes ends,
released after they are reported, so a crash before then loses none. A running Task whose flow no process
here runs reads its result through the run directory and the profile's flow
lock root (`RunQuery::TaskFlow`): a resumable flow end is the Task's end while
the Task stays `Running`. The watch keeps it as an `Interrupted` end: reporting
it does not release the Task's ledger entry, a restart keeps it in scope, and
the Task's own end, whether a resume (or `start_flow`) settles it or the Task
is canceled or abandoned without one, is delivered as a new episode. A live
`Stalled` end gives the Task the same result. Resumes, including a desktop one
and one seen on disk, keep the key's solicitation; only an orchestrator or
phone start brings its own.
Flow questions and level comparisons of the exact Running AgentChat Task's
permission state produce waiting episodes. Closing a block or ending the run
supersedes an unreported block; a resumed flow can produce a new end episode
under the same handle.
Held flow ends carry a separate episode identity. Resuming announces the earlier
held end before opening the new episode; a late report enriches only its own
still-unreported end, even across consecutive resumes.

Wait requires an integer `timeout` in seconds: zero polls, and values above
`RUN_WAIT_TIMEOUT_MAX` (110) are clamped with requested/actual metadata. One
omitted-handles waiter may be active (`WaiterBusy` for another); explicit
waiters can coexist. Eligible events are delivered end-first, FIFO within each class, at most twenty per
answer with a `more` count. Explicit waits return previously reported ends in
`already_reported` and unknown, gone or unreadable entries in `not_running`;
those entries do not end a wait while another selected execution is running.
`TimedOut` and `Nothing` are normal outcomes. After repeated timeouts, inspect
`daruda_run_status`. Externally running disk flows are refreshed off-thread
every two seconds independently of unrelated run wakeups. Per-key revisions
come from a watch-wide monotonic clock, so deleting and recreating a key cannot
let an old background read overwrite a newer lifecycle, including resume.

An event moves `Unreported -> InFlight -> Reported`. Reservation keeps its
waiter active through the socket receipt phase. Only successful `write_all`
of the complete JSON response and its newline marks it reported; failed
writes, cancellation and disconnect roll it back and withdraw the waiter.
The read loop announces disconnect before waiting for its writer, so pending
waits do not block reconnection. This receipt confirms a socket write, not
that the model read or acknowledged the result. Status can recover retained
results after such a loss. The 200-entry reported retention limit never
evicts running, held, unreported or in-flight snapshots, even when Task history
has rolled past them or an earlier episode of the same key was reported.
After snapshot eviction, up to 1000 cheap reported-terminal identities remain
to prevent a disk/history lookup from delivering the same end again. A scoped
key whose run is not over (a Task's reported flow interruption) is never
retired: its own end is still to come. The oldest
retired identities are then forgotten with their scope membership and cached
flow locations; explicitly querying a forgotten end may deliver it again.
Separately, a sweep keeps the newest 1000 reconstructible settled observations
that no waiter or event follows. Sweeps allow 256 additional keys between passes
so retained identities and active runs do not force a scan on every progress
update. Active runs, followed results and unreported local flow ends are protected
independently of these limits: a local end can contain facts absent from disk.

### Orchestrator run notices

The `RunWatch` event channel is drained on the foreground, outside its state
mutation, into `RunsEffects::run_event`. Workspace's effect forwards to
`control/notices`; its deferred flush avoids re-entering an active Workspace
update. Notices scan the delivery ledger afresh instead of retaining duplicate
result payloads. They select only omitted-scope, unreported episodes that no
active waiter covers, including coverage during the socket receipt phase.
Releasing a waiter re-announces still-unreported events with their original
identities after obsolete blocked episodes are pruned, so an idle replacement
session can retry them without another run event.

A notice can dispatch immediately when an event arrives at an idle orchestrator,
at `fire_activity_completion` after a busy orchestrator settles, or after pane
creation or session replacement rotates its gate epoch. No pane or a rejected
prompt leaves episodes eligible for a later opportunity; failures use deduplicated
`LogWriter` reporting. One pointer prompt contains at most twenty lines with
handles, names, end/wait summaries and `call daruda_run_wait`, plus `+k more`.
Accepted `(RunKey, sequence)` identities are recorded for the current notice
audience, `Audience { pane, session }` (`control/notices.rs`), for every eligible
entry the batch represents. Each batch prunes the record to still-unreported
episodes, and a different audience starts from an empty record, so a
replacement pane or session is told again.
Waiter-covered and in-flight entries stay eligible later. Accepting a notice
never marks a run result reported.

`AgentChatView::try_send_internal_prompt_text` uses `PromptOrigin::Internal`
for these literal prompts. It never queues: it sends only to a connected session
with no turn in flight, no open Stop/cancel-ack window and no queued or parked
prompts. `NotNow` and `ReadOnly` refusals leave composer, queued-edit and phone
state untouched; `Unavailable` is a failed handoff and reports the dead
transport like any send (queued prompts dropped, handle detached, error shown).
A refused notice stays eligible; the host observes the pane until
`accepts_internal_prompt_now` holds and flushes again. Prompts parked by Stop
count as waiting, so notices hold until the user resumes, removes or discards
the parked queue; that is deliberate, since a notice never passes a user prompt.
An explicit `daruda_run_wait` call is unaffected.
The host reconciles any owed completion before dispatch and reconciles the new
activity afterward, routing both edges through `fire_activity_completion`.
It does not interpret slash commands or arm a phone first-response watch.
A batch is solicited if any eligible episode is solicited. After accepted
dispatch, such a notice arms `PhoneTurnMode::CompletionOnly`: no received ACK,
first-response relay or fallback, and one explicit completion or failure even
while the user is present. Refusals preserve the previous phone state. Desktop
notices retain each channel's presence policy. Only direct Phone flow submissions
keep the direct flow outcome notice; Orchestrator flows answer through their
notice turn. Phone starts do not join the omitted-handles scope.

Notices reach only a running orchestrator, which starts lazily. After a
restart, `orchestrator/recovered.rs` starts it at most once per process for
results the phone is still owed. When the first Workspace registers
(`WindowRegistry::register`), any solicited, unreported episode in the omitted
scope starts it: before a window exists, every such episode predates this
process. If nothing was
owed then, only a later `Recovered` solicited announcement starts it, because
`judge_restored` ends arrive after the first window; a live result in ordinary
operation never does. Both checks are deferred out of the current update. The
start goes through `orchestrator::ensure`, so a disabled feature starts
nothing, and the notice turn arms `CompletionOnly` from the episode's
solicitation, which the ledger restores. Results the desktop asked for never
start it; the next ordinary start delivers them. Each start attempt is logged
once: an info line with the count and outcome (started, already running, or
disabled), or, for any other `EnsureError`, a warning with the count and reason
that is also toasted in the window a start hosts the orchestrator in (the
active workspace, else the first), as a chip start failure is (the log alone
if that window is gone). `ensure` itself logs nothing: each caller reports a
failed attempt once. A failed start leaves the results
unreported and is not retried in that process.

An active Orchestrator-started Task's matching child chat suppresses its own
completion/error relay, even before run-scope registration: TaskRecord
provenance and the active execution establish ownership. Native completion
notifications and permission requests are unchanged. An actual open child
`PhoneTurn`, direct Phone/Desktop Tasks and manual continuation after a Task
is terminal retain their relay. This prevents the child final answer and the
orchestrator summary from both reaching the phone. Direct `/task` does not gain
an omitted-scope result or a new completion-delivery guarantee.

That manual continuation is recorded as the task's next run. When a person's
prompt (typed, or relayed from their phone) reaches the wire in the pane of an
Agent Chat run that ended `Done` or was stopped by a person or the usage limit,
`RunsHost::person_prompt_sent` commits `TaskEvent::Continued` and rebinds the
pane to the new run once it is committed; a restored pane is matched by the
run's session. The new run is started by Desktop or Phone, so its relay and
watch scope are those of a run that person started; the ended run and its
result stay as they ended. Orchestrator-sent prompts and host notices never
continue a run. Claude CLI runs left in the log are not continued.

## `surface/`

App-shell constants and binding glue — app/process name, user-visible labels (menu / landing / dock), keyboard shortcut string constants, the binding→display formatter, and the config-override → GPUI `Action` resolver. Pure strings + no sibling imports (except the action map, which reaches into workspace action types).

## `ui/`

The facade over `daruda_ui` (`packages/ui/`), which holds the reusable widget primitives in two layers. The facade itself only adds localized wrappers and the agent-domain widgets:

- **Wrappers** — thin factories / re-exports over `gpui_component::*` with daruda chrome (palette, `xsmall` defaults, modal tab-spec) baked in. Canonical post-migration shape.
- **Preserved daruda widgets** — kept where `gpui_component` lacks a clean swap: bespoke hover chains, multi-purpose macro tile, absolute-positioned context menu, multi-line input panel composite, procedural status indicator, layout helpers, and `picker_row` / `picker_empty` (`packages/ui/src/picker_row.rs`) — the shared row chrome of every search-and-pick overlay, which owns the focus-accent reservation, the label truncation, the trailing column's `flex_none`, and the row height `theme::PALETTE_MAX_HEIGHT` is derived from.

Theme overlay (`apply_daruda_palette`) is the only place inline `hsla()` / `px(N)` literals are allowed (G4 exempt). See [UI rules](../../ui/AGENTS.md) and the [wrapper recipe](../../ui/REFERENCE.md#adding-a-new-wrapper).

### Tab navigation in modals

GPUI's tab cycle is viewport-wide and ordered by `(tab_group path,
tab_index, insertion order)`. The wrapper layer enforces a single
rule so each modal only has to pick the right number:

**Every input-style wrapper takes a `tab` argument.** Two intents,
shared `XxxTabSpec` trait across wrappers:

- **`isize`** — join the cycle at this index.
- **`()`** — skip the cycle (mouse-only).

| Wrapper | Signature |
|---------|-----------|
| `input` / `checkbox` / `radio` / `select` | `(…, tab)` — `tab: isize \| ()` |
| `button*` | `.tab_stop(false)` baked — opt-in to cycle via `.tab_stop(true).tab_index(n)` |

**Modal-level rules**

- Modal entity's root `div` must call `.tab_group()`. Cycle
  containment inside an active dialog is enforced by a
  `gpui_component::Root` patch — escapes wrap back to the topmost
  dialog's focus handle.
- Initial focus is scheduled via `cx.defer` **after** `open_dialog`
  (Root focuses its own handle synchronously; the defer lets the
  primary input win on the next update cycle). Handled by
  `dialog_helpers::*`.

**Numbering convention** — `tab_index` runs in visual order across
mixed widget types without resetting; reuse the same index pool
across chip-driven branches so the user lands on the same logical
slot regardless of which fields are visible.

## `project/` (`daruda_project::project`)

Runtime [`Project`] — the workspace-visible counterpart to
`daruda_store::project::SerializedProject`. Owns the project root, its
non-empty `Vec<Lane>`, the `last_active_lane_id` snap target,
and visual metadata (color, `tab_order`, `group_id`). GPUI-free. The
dependency order is `workspace/ → project/ → lane/`; the package
cannot import from the app at all.

## `workspace/`

The Workspace entity and its subsystems.

- **Entity & actions** — `Workspace` struct, construction (`new_with_project`), config apply, `on_*` action shims, command palette + history picker, modal openers (`dialog_helpers`), persistence (save/restore/rebuild + `LaneRuntime`), status-bar snapshot, lane create/remove/activate. Holds `projects: Vec<Project>` + `groups: Vec<SerializedGroup>` + `active: LaneRef`; `lane_scoped: HashMap<LaneRef, LaneScoped>` owns per-lane git state, file data/watchers, and input history.
- **`project_ops/` / `group_ops.rs` / `project_palette_ops.rs`** — Project CRUD (add / close / delete-on-disk / rename + `window_open_policy`), Group CRUD (add/rename/recolor/collapse/delete + `move_project_to_group`), and palette dialog plumbing (`New Group`, `Rename Project`, `Move Project to Group…`).
- **`layout/`** — `Dock` entity (left/bottom/right; named `left_dock`/`right_dock`/`bottom_dock` on `Workspace`), divider + dock drag ops, plain-data snapshots for re-entrancy-safe render.
- **`main_area/`** — TabBar + recursive PaneTree runtime. Houses `MainAreaContext`, the pure pane split-tree, pane structs + PTY spawn, directional navigation, tab lifecycle, viewport resize propagation, and the recursive `PaneLayout` renderer. `pane/mod.rs` owns pane models and view coordination; `pane/accounts/model.rs` resolves account selection without GPUI; `pane/accounts/mod.rs` owns workspace queries and preparation; `pane/terminal.rs` starts PTYs; `pane/output.rs` drains output and owns the adaptive poll policy. Sub-domains: `file_view_pane/` (file viewer), `bottom_dock/` (macro grid + terminal input + tab strip + macro data ops).
- **`left_dock/`** — Lanes view (displayed as "Worktrees" tab) rendered as a 2-level tree (`TopRow::Group(GroupId)` / `TopRow::UngroupedProject(ProjectId)` at the top rank, expanding into project headers and lane rows). Group/Project drag payloads share a single `Vec<TopRow>` ordering pool with 0..N renumbering on drop; intra-project lane DnD stays within its project. Also git-changes view, files view, lazy file-tree context + walker, git-status fetch.
- **`pages/`** — Tasks and Flows, shown in place of the lane's tabs and belonging to no lane. What a page shows on top of its list lives in `Workspace.pages` and survives the page being hidden; only `leave_page_detail_then` (`pages/detail.rs`) clears it, asking first about unsaved edits. `tasks/editor/` is the Task editor; `flows/graph/` is the flow graph, which runs in the worktree it was opened for, not the active one.
- **`right_dock/`** — Usage / skills / tasks / tools views.
- **`render/`** — `impl Render for Workspace`. Reads only plain-data snapshots — no re-entrant entity reads.
- **`sync/`** — Background event pumps: PTY drain (tick), HTTP usage/limits poll, MCP filesystem watcher, skills filesystem watcher.
- **`tests/`** — Async `#[gpui::test]` lifecycle tests and sync `#[test]` pure-op tests (layout, branch sanitization, etc.).

## `lane/` (`daruda_project::lane`)

**Concept**: "one Claude Code session per `Lane`". A `Lane` is a workspace unit — typically a git worktree (its own directory + HEAD + branch) but a plain non-git directory for repos that aren't initialized. Multiple agents run concurrently in the same window without branch-switching or `target/` cache thrashing. Lanes are permanent until the user clicks ×. Path stays as visible sibling (`<repo>-<branch>`), not hidden. The user-facing label remains "Worktree".

Runtime `Lane` model (id / path / status / `base_ref` / description) plus a GPUI-free blocking git-CLI layer (probe, list/add/remove git worktrees, branch ops). All git calls run on `background_executor`.

## Conventions (enforced)

- **No magic numbers / ad-hoc strings** — pixel sizes in `daruda_terminal::ux::theme`; strings in `surface::strings` or `daruda_terminal::ux::strings`.
- **No GPUI re-entry in `render()`** — snapshot into a plain struct first (see `StatusBarData`).
- **`pub(in crate::workspace)` for workspace internals.**
- **Git CLI on background executor** — `lane::git` is blocking; wrap with `window.spawn` + `background_executor`.
- **Re-entry guards** — `OPEN_IN_PROGRESS` around folder picker; modal key handlers must `cx.stop_propagation()`.
- **Tests alongside code** — new modules add `#[cfg(test)] mod tests`.
- **Errors and GPUI update Results** follow [Error Reporting](AGENTS.md#error-reporting).

## Extension recipes (quick reference)

| Recipe | Key touch points |
|--------|-----------------|
| New dock view | `LeftDockView` variant (or `RightDockView`) → `view_tabs.rs` → `left_dock/<view>/mod.rs` (export `render`) → `mod.rs` action + handler → `render.rs` match arm → test |
| New global action + keybinding | `actions!()` → handler in ops file → `surface/keybindings.rs` const → `main.rs` bind_keys → `action_map.rs` arm → `command/palette.rs` entry |
| New dock panel | `PlaceholderKind` variant → `Workspace::new_with_project` push → renderer module → `render.rs` dock match arm → persistence if needed |
| New modal | See [G9](AGENTS.md#g9--modals-go-through-gpui_componentdialog). `impl ModalView` + `.tab_group()` on root; open via `dialog_helpers::*`. |
| New pane content kind (plain-struct) | `PaneContent` variant + struct in `main_area/pane/mod.rs` → match arms (title/cwd/focus_handle/resize) → `main_area/mod.rs` walker arm (free-fn `render(&content, cx)`) → `daruda_project` persistence mirror + `#[serde(default)]` → `create_*_pane` constructor → `workspace/tests` round-trip. Rendered inline under `Workspace::render`, so its `cx.notify()` dirties the whole window. Fine for small, rarely-updating panes (File). |
| New pane content kind (entity-backed / cached) | For a pane needing **scroll/perf isolation or its own internal state** (Terminal, AgentChat): hold a thin `XxxContent { view: Entity<XxxView>, cached_title, cwd }` wrapper; `XxxView: Render + Focusable`, its `render` calls `track_focus(&self.focus_handle)` and `cx.notify()`s itself; walker arm embeds `AnyView::from(view.clone()).cached(StyleRefinement::default().size_full().flex())`; `wrapper_focus_handle → None`; `Pane::{title,cwd}` read the wrapper (cx-free), `focus_handle(cx)` reads `view.read(cx)`. A `cx.notify()` on the view then dirties only its subtree — siblings keep their cached paint. |
| Virtualized list inside a pane (variable-height) | Use gpui core `list(state, cx.processor(\|this, ix, win, cx\| …))` + `ListState` (`Top` align + `FollowMode::Tail` for chat-like append) — **not** `crate::ui::list` (that's the gpui_component delegate/searchable list) and not `uniform_list` (fixed height). Keep `list_state` count in sync after each items mutation; `remeasure_items(tail)` on streaming grow, `remeasure()` after any visible item's height changes (fold, async image/diff landing). Thumb + at-bottom: `crate::ui::scrollbar::{vertical_thumb_for_list, list_at_bottom}` (the thumb scrolls the `ListState` it reads). Synthetic fold headers (turn ⊃ response ⊃ tool-group ⊃ block) are not stored — `rows::project(items, &fold)` derives a stable `Vec<RenderRow>` (header rows + `hidden`/`indent` flags) each rebuild; the list virtualizes over rows and `rebuild_rows` diff-splices so a fold toggle remeasures in place (no scroll drift). Reference: `packages/chat/src/{view/,render/,rows/,fold.rs}`. |
| Skills / Tools tab feature | Mutate the relevant Global via `cx.update_global::<GlobalSkills\|McpState, _>(...)` → renderer reads through the snapshot in `RightDockSnapshot` → `cx.observe_global` rebroadcasts to every Workspace, which pushes into its open Settings |
| Tasks feature | Dispatch a `TaskIntent` through `RunsHost` → durable commit → read-only `GlobalTasks` projection → each Workspace's `cx.observe_global` notification |
| Worktree drag/context menu | Data ops in `lane/mod.rs` → `LaneDrag` in `layout/ops.rs` → actions in `lane_ops/` → UI in `left_dock/projects/list.rs` |
| New picker (search-and-pick overlay) | Plain shape, no "no list" state (copy lane switcher/palette): `is_open: bool` beside `pub picker: PickerState` on the view state, Workspace handler computes `visible_len` and calls `picker.on_key(...)` directly. Enum shape, has a "no list" state (copy flow picker): openness *is* the enum, `picker` module-private, view calls `FlowPicker::on_key` which answers Escape/Enter through the shared `picker::overlay_key` and delegates the rest only when there is a list. Either way: `PickerState` field → `ui::picker_row`/`picker_empty` → thin `on_*_key` handler in the view's `*_ops.rs` (next to its `execute_*`), opening with `let Some((key, ch)) = picker_keystroke(ev) else { return };` (`command/picker_key.rs` — returning early *without* `stop_propagation` is what keeps `Cmd+W` working) → `.when(is_open, capture_key_down(...))` stays in `render/workspace_frame.rs` |

## Where things go (decision matrix)

| New feature touches… | Lives in |
|---|---|
| Pure data / algorithm, no GPUI | `lane/`, `agent/mod.rs`, `workspace/main_area/pane_tree.rs`, `daruda_project`, `daruda_config` |
| GPUI render only | `agent/<view>.rs`, `workspace/left_dock/<view>/`, `workspace/render/` |
| Workspace action handler | `workspace/mod.rs` (tab/pane/focus) · `workspace/lane_ops/` · `workspace/layout/ops.rs` |
| Workspace data discarded on lane/project teardown | `workspace/lane_scoped.rs::LaneScoped`; runtimes and flow runs retain their own lifecycle containers |
| New pane content kind | `main_area/pane/mod.rs` + `main_area/mod.rs` walker arm + `daruda_project` + `workspace/construct.rs` constructor |
| New modal / text input in modal | See [G9](AGENTS.md#g9--modals-go-through-gpui_componentdialog). |
| Reusable widget | `crate::ui`. Never inline `div().flex().hover(...).on_mouse_down(...)` at call site. |
| Right-click menu on any element | `workspace/root_menu.rs::root_context_menu`. Never the vendored `.context_menu(...)` — it renders inside the caller's subtree, where an ancestor clip cuts the menu *and* its hit-testing. |
| Save/restore logic | `workspace/persistence/` |
| App-global action | `main.rs` (`on_action`) |
| Window lifecycle | `windows.rs` |
| Settings section / field | `packages/settings/src/sections/` + a `spec/` row; opened and closed in `workspace/settings_ops.rs` |
| Menu bar | `menus.rs` |
| Key-binding string | `surface/keybindings.rs` |
| Keybinding-override arm | `surface/action_map.rs` |
| User-visible label | a `locales/en.yml` key (→ `surface::strings::<section>`) or `daruda_terminal::ux::strings` |
| User-tunable value | `daruda_config` |
| Pixel/color constant | `daruda_terminal::ux::theme` |
| Blocking subprocess | `lane/git.rs` style — GPUI-free, callers wrap with `background_executor` |
| Cross-session persistence | `daruda_project` schema + `migrate_legacy` + `#[serde(default)]` |

## Crate Dependency Graph

```
daruda (app)  →  daruda_terminal  →  ghostty_vt  →  ghostty_vt_sys
             |                   →  daruda_core
             →  daruda_config     →  daruda_store  →  daruda_core
             →  daruda_store
             →  daruda_agent      →  daruda_store
             →  daruda_acp        →  daruda_core    # GPUI-free ACP client core
             →  daruda_flow       →  daruda_acp, daruda_runlog, daruda_core  # flow runs kept in a run log
             →  daruda_flow_edit  →  daruda_flow   # YAML edits that keep comments and order
             →  daruda_project    →  daruda_config, daruda_store, daruda_core  # GPUI-free Project/Lane
             →  daruda_content    →  merman, resvg, image   # GPUI-free image + mermaid pipeline
             →  daruda_control_types  →  daruda_store   # GPUI-free control contract, PaneRef
             →  daruda_ui         →  daruda_terminal, daruda_config, gpui_component, ferrum_flow  # widgets
             →  daruda_transcript →  daruda_acp, daruda_config, daruda_ui   # transcript presentation, shared by chat + Settings
             →  daruda_chat       →  daruda_transcript, daruda_acp, daruda_content, daruda_ui, daruda_config, daruda_store, daruda_core  # the chat pane view
             →  daruda_settings   →  daruda_transcript, daruda_agent, daruda_project, daruda_config, daruda_ui  # Settings + the config store
             →  daruda_core                         # shared, GPUI-free
             →  daruda_update
             →  gpui, gpui_component, merman, portable-pty

gpui_component  →  daruda_core                         # vendored; shares the text primitives
daruda_store    →  daruda_runlog → daruda_core          # GPUI-free task state machine and run log
```

The app can reach every package; GPUI-free packages cannot reach the app.
Shared logic below that boundary follows [Core admission](../../core/AGENTS.md).
Configuration and agent packages use `daruda_store`'s persistence APIs; see
[profile isolation](../../store/AGENTS.md). Shared image/Mermaid and the two
Markdown stacks are documented in [Content](../../content/AGENTS.md).
Vendored dependency rules are in [UI](../../ui/AGENTS.md#vendor-dependencies).

## UI Component Hierarchy

```
Workspace
├── TitleBar                         — shared by Workspace / Settings; off macOS also carries the app-menu button and the caption controls (`app/src/title_bar/`)
├── SettingsView                     — replaces BodyLayout while open, docks included (`packages/settings/`)
│   ├── SettingsSidebar              — back button + search + section nav
│   └── SettingsSection              — one BuiltinSection body at a time
├── BodyLayout                       — shown when SettingsView is not
│   ├── LeftDock
│   │   ├── ViewSwitcher             — Worktrees / Git / Files tab strip
│   │   ├── WorktreesView            — 2-level tree: Group ▸ Project ▸ Worktree
│   │   │   ├── GroupHeader          — collapsible accordion (color, name, ▼/▶, context menu)
│   │   │   ├── ProjectHeader        — project row (inside a group, or ungrouped at top rank)
│   │   │   └── WorktreeRow          — leaf (create / delete / merge actions, Claude badges)
│   │   ├── GitChangesView
│   │   └── FilesView
│   ├── MainArea
│   │   ├── TabBar
│   │   ├── PaneTree                 — one per tab; recursive split tree (single pane = 1-leaf root)
│   │   │   └── Pane                 — leaf node, single content slot
│   │   │       ├── PaneHeader       — per-pane title bar, visible in split mode only
│   │   │       └── PaneContent
│   │   │           ├── TerminalPane — PTY terminal (TerminalTextElement)
│   │   │           ├── FileViewPane — file viewer (toolbar + virtual list)
│   │   │           └── AgentChatView — cached chat entity (packages/chat/)
│   │   ├── Page                     — Tasks / Flows, in place of TabBar + PaneTree; belongs to no lane
│   │   │   ├── PageList             — the page's list
│   │   │   └── PageDetail           — one at a time, over the list: TaskEditor · FlowGraph · RunDetail
│   │   └── BottomDock
│   │       ├── DockSwitcher         — BottomDock top tab row
│   │       ├── TerminalInputDock    — multiline input + submit / action buttons
│   │       └── MacroDock            — N-column macro key grid
│   │           └── MacroKey         — macro key (icon or text mode)
│   └── RightDock
│       ├── ViewSwitcher             — Usage / Skills / Tasks / Tools tab strip
│       ├── UsageView
│       ├── SkillsView
│       ├── TasksView
│       └── ToolsView
├── StatusBar
├── Dialog / Sheet / Notification    — gpui_component::Root overlay layers
└── ToastLayer                       — app error notifications (workspace/toast_layer/)
```

**Concept-to-code mapping** — identifiers match the hierarchy names above:

| Hierarchy name | Code identifier | Location |
|---|---|---|
| `LeftDock` | `left_dock` field / `Dock` entity (position=Left) | `workspace/left_dock/`, `workspace/layout/mod.rs`, `render/workspace_frame.rs` |
| `RightDock` | `right_dock` field / `Dock` entity (position=Right) | `workspace/right_dock/`, `workspace/layout/mod.rs`, `render/workspace_frame.rs` |
| `ViewSwitcher` | `render()` in `view_tabs.rs` (both docks) | `left_dock/view_tabs.rs`, `right_dock/view_tabs.rs` |
| `DockSwitcher` | `PanelTabStrip` | `main_area/bottom_dock/tab_strip.rs` |
| `TerminalInputDock` | `TerminalInputPanel` | `main_area/bottom_dock/terminal_input.rs` |
| `MacroKey` | `MacroKey` | `ui/macro_key.rs` |
| `PaneTree` | `PaneLayout` enum | `workspace/main_area/pane_tree.rs` |
| `Pane` | `PaneLayout::Pane` | `workspace/main_area/pane_tree.rs` |
| `TerminalPane` | `PaneContent::Terminal` | `workspace/main_area/pane/mod.rs` |
| `FileViewPane` | `PaneContent::File` | `workspace/main_area/pane/mod.rs` |
| `Page` | `workspace_page: Option<PageState>` | `workspace/pages/mod.rs` |
| `PageDetail` | `pages: PageDetails`, left only through `leave_page_detail_then` | `workspace/pages/detail.rs` |
| `TaskEditor` | `TasksPage.detail` | `workspace/pages/tasks/editor/` |
| `FlowGraph` / `RunDetail` | `FlowsPage.detail` (`FlowDetailBody`) | `workspace/pages/flows/graph/`, `workspace/pages/flows/run_detail.rs` |
| `ToastLayer` | `toast_layer: Entity<ToastLayer>` | `workspace/toast_layer/mod.rs` |
| `SettingsView` | `settings: Option<SettingsHost>` — `Some` *is* settings mode | `packages/settings/`, opened/closed in `workspace/settings_ops.rs` |
| Project (runtime) | `crate::project::Project` (re-export of `daruda_project::project`) | `packages/project/src/project/mod.rs` |
| Group (runtime) | `daruda_store::project::SerializedGroup` (used directly — no separate runtime newtype) | `packages/store/src/project/` + `workspace/group_ops.rs` |
| Lane (runtime) | `crate::lane::Lane` (re-export of `daruda_project::lane`) — UI label remains "Worktree" | `packages/project/src/lane/mod.rs` |
| Lane (persisted) | `daruda_store::project::SerializedLane` + `LaneKind { Git { .. }, Default }` | `packages/store/src/project/lane.rs` |
| Active focus ref | `daruda_store::project::LaneRef { project, lane }` — JSON keys remain `worktree` via `#[serde(rename = "worktree", alias = "lane")]` | `packages/store/src/project/`; per-lane caches keyed by ref in `workspace/mod.rs` |
| `ProjectsView` 2-level tree | `TopRow` enum dispatch + `group_header_row` / `project_header_row` / `worktree_row` (function name retained — UI affordance) | `workspace/left_dock/projects/rows/{headers,worktree}.rs` |
| Multi-project DnD | `DragPayload { Worktree | Project | Group }` + `dnd_ops.rs` reorder pool | `workspace/left_dock/projects/drag.rs`, `workspace/dnd_ops.rs` |
