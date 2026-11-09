//! Phone relay — `Workspace` ops that push agent-chat pings out through
//! every remote channel (`crate::remote_channel`) and route
//! phone-tapped replies / permission decisions back into the triggering pane.
//!
//! Sibling of [`super::agent_chat_ops`], whose `maybe_notify_agent_event`
//! (permission wait) and `fire_activity_completion` (turn completion) tee points
//! call into this file's `relay_*` methods; both are `impl Workspace` blocks.

use std::time::Instant;

use gpui::{App, Context};

use crate::remote_channel::bridge::MessageTail;
use crate::remote_channel::trace;
use crate::surface::strings as s;
use crate::workspace::Workspace;
use crate::workspace::main_area::pane_tree::PaneId;

mod diff_preview;
mod inbound;
mod questions;

pub(in crate::workspace) use daruda_chat::phone_turn::{FirstResponseOutcome, PhoneTurn};

/// Leading/trailing characters kept when a response is truncated — head carries
/// the ask, tail carries the result; the elided middle is usually tool-output
/// detail already visible in the app. Their sum is the threshold below which
/// text goes verbatim.
const REMOTE_PREVIEW_HEAD_CHARS: usize = 1000;
const REMOTE_PREVIEW_TAIL_CHARS: usize = 1000;
/// Maximum time a phone-triggered turn can stay silent before the phone receives
/// the fixed fallback acknowledgement.
pub(in crate::workspace) const FIRST_RESPONSE_FALLBACK_SECS: u64 = 60;

/// The phone's slice of an agent response: the first
/// [`REMOTE_PREVIEW_HEAD_CHARS`] and last [`REMOTE_PREVIEW_TAIL_CHARS`]
/// characters around `marker` (the caller's localized "…(truncated)…" string).
///
/// This file owns the *budget* and the localized marker; the elision itself is
/// `daruda_control_types::agent_text`'s, shared with the orchestrator's own reader —
/// which keeps a different amount and a fixed English marker.
fn preview_for(text: &str, marker: &str) -> String {
    daruda_control_types::agent_text::elide_middle(
        text,
        REMOTE_PREVIEW_HEAD_CHARS,
        REMOTE_PREVIEW_TAIL_CHARS,
        marker,
    )
}

/// The finished run in one line — how long it worked and what its tool calls
/// did, by category. `None` when there is nothing to say.
fn run_summary_line(summary: &daruda_chat::view::RunSummary) -> Option<String> {
    let segments: Vec<String> =
        summary
            .worked_for
            .map(s::notification::format_duration_compact)
            .into_iter()
            .chain(summary.tools.iter().map(|&(category, count)| {
                daruda_chat::labels::group_category(category.token(), count)
            }))
            .collect();
    (!segments.is_empty()).then(|| s::control::remote_run_summary(&segments))
}

/// Compose the permission-wait ping's tail: the localized "waiting for input"
/// line, then optional tool-title and raw-input-summary lines (the same
/// `daruda_acp::PermissionItem` fields the in-app card is built from) so the
/// phone message names *what* is being asked. An empty string is treated as
/// absent (defensive; the source already filters it).
fn permission_wait_text(tool_title: Option<&str>, raw_input_summary: Option<&str>) -> String {
    let mut tail = s::notification::agent_waiting();
    for line in [tool_title, raw_input_summary] {
        if let Some(line) = line.filter(|l| !l.is_empty()) {
            tail.push('\n');
            tail.push_str(line);
        }
    }
    tail
}

/// The permission body, with an optional bounded file-change preview. Plain
/// administrative lines are escaped before the tail becomes Markdown; only
/// the preview is intentionally formatted, as a fenced `diff` block.
fn permission_wait_tail(
    prompt: &daruda_acp::PermissionItem,
    diffs: &[daruda_acp::DiffView],
) -> MessageTail {
    let text = permission_wait_text(
        prompt.tool_title.as_deref(),
        prompt.raw_input_summary.as_deref(),
    );
    let Some(preview) =
        diff_preview::diff_preview(diffs, daruda_chat::labels::diff_fallback_truncated)
    else {
        return MessageTail::Plain(text);
    };
    MessageTail::Markdown(format!(
        "{}\n\n{}",
        diff_preview::escape_markdown_text(&text),
        diff_preview::fenced_diff(&preview)
    ))
}

/// Diffs on the tool call a permission card belongs to. Empty when the
/// adapter did not send the tool card before asking, or the card is synthetic.
fn permission_diffs<'a>(
    view: &'a daruda_chat::view::AgentChatView,
    tool_call_id: &str,
) -> &'a [daruda_acp::DiffView] {
    view.items()
        .iter()
        .rev()
        .find_map(|item| match item {
            daruda_acp::ChatItem::ToolCall(call) if call.id == tool_call_id => {
                Some(call.diffs.as_slice())
            }
            _ => None,
        })
        .unwrap_or_default()
}

/// Compose the "went straight to a tool call" first-response ack's tail: the
/// fixed i18n label, plus the tool's title on its own line when the agent
/// supplied a non-empty one.
fn first_tool_ack_tail(tool_title: Option<&str>) -> String {
    let mut tail = s::notification::remote_first_tool_ack();
    if let Some(title) = tool_title.filter(|t| !t.is_empty()) {
        tail.push('\n');
        tail.push_str(title);
    }
    tail
}

/// Build one phone button per permission choice, in the same order and with
/// the same labels the in-app card uses (the same `daruda_acp::PermissionChoice`
/// list). A richer option set (e.g. codex-acp's "Allow Once" / "Allow for
/// Session" / execpolicy amendment, alongside Reject) stays fully choosable from
/// the phone — no collapsing to a single Allow/Reject pair. Each `*Once`/
/// `*Always` kind maps to the same `Allow`/`Reject` wire outcome (the kind only
/// picks in-app styling; the outcome is carried by `option_id`). Empty `Vec`
/// only when the agent supplied no options (caller then skips the relay).
fn permission_buttons(
    options: &[daruda_acp::PermissionChoice],
) -> Vec<(String, crate::remote_channel::bridge::PermissionDecision)> {
    use crate::remote_channel::bridge::PermissionDecision;
    use daruda_acp::PermissionKindView as Kind;
    options
        .iter()
        .map(|o| {
            let decision = match o.kind {
                Kind::AllowOnce | Kind::AllowAlways => {
                    PermissionDecision::Allow(o.option_id.clone())
                }
                Kind::RejectOnce | Kind::RejectAlways => {
                    PermissionDecision::Reject(o.option_id.clone())
                }
            };
            (o.name.clone(), decision)
        })
        .collect()
}

impl Workspace {
    /// The owning project's display name, excluding workspace-owned chats
    /// and panes whose lane or project has gone away.
    fn project_name_for_pane(&self, pane_id: PaneId) -> Option<String> {
        let project_id = self.lane_ref_for_pane(pane_id)?.project;
        self.project_for(project_id).map(|p| p.name.clone())
    }

    /// The header line(s) for a pane's phone pings: project name + agent name
    /// when the pane's view is live, else the pane title. Shared by the
    /// completion and ack relays so they all read identically.
    pub(in crate::workspace) fn remote_header(
        &self,
        pane_id: PaneId,
        cx: &Context<Self>,
    ) -> String {
        self.remote_header_at(pane_id, self.project_name_for_pane(pane_id), cx)
    }

    /// [`Self::remote_header`] with `project_line` as its first line.
    fn remote_header_at(
        &self,
        pane_id: PaneId,
        project_line: Option<String>,
        cx: &Context<Self>,
    ) -> String {
        match self.agent_chat_view(pane_id) {
            Some(view) => {
                let agent = view.read(cx).agent_name().to_owned();
                let agent = match self.pane_tab_name(pane_id) {
                    Some(tab) => s::control::agent_with_tab(&agent, &tab),
                    None => agent,
                };
                match project_line {
                    Some(project) => format!("{project}\n{agent}"),
                    None => agent,
                }
            }
            None => self.pane_title(pane_id, cx),
        }
    }

    /// Compose the turn-completion ping's header + tail: the plain header, and
    /// the agent's last response ([`preview_for`] truncates past 2000 chars) as
    /// a [`MessageTail::Markdown`] tail so the phone shows what the agent
    /// actually said with its markdown rendered. Falls back to a
    /// [`MessageTail::Plain`] "finished responding" tail when the turn produced
    /// no assistant text (e.g. tool-only) or the view is gone — that fallback is
    /// plain i18n copy, not agent markdown, so it must not be parsed as such.
    ///
    /// `None` when the sender already has this answer: a phone-dispatched turn
    /// whose last message is the very one the first-response relay sent has
    /// nothing left to report, and repeating it puts the same text on the
    /// phone twice.
    ///
    /// A pure query, so asking is free and asking twice is the same as asking
    /// once. [`Self::close_phone_turn`] is what ends the turn, and
    /// `fire_activity_completion` calls it whether or not this returned
    /// something to send.
    pub(super) fn remote_completion_parts(
        &self,
        pane_id: PaneId,
        cx: &Context<Self>,
    ) -> Option<(String, MessageTail)> {
        let mut header = self.remote_header(pane_id, cx);
        if let Some(line) = self
            .agent_chat_view(pane_id)
            .and_then(|view| view.read(cx).last_run_summary())
            .and_then(|summary| run_summary_line(&summary))
        {
            header.push('\n');
            header.push_str(&line);
        }
        let Some(view) = self.agent_chat_view(pane_id) else {
            return Some((
                self.pane_title(pane_id, cx),
                MessageTail::Plain(s::notification::agent_completed()),
            ));
        };
        let view = view.read(cx);
        // A phone turn reports only what *it* said. Without the anchor the
        // scan runs back through the whole transcript, so a turn that
        // produced no text of its own hands the sender an earlier turn's
        // answer as though it were this one's. A turn the phone did not send
        // has no anchor to bound it by — that gap is the same one every
        // reader of this transcript has, and closing it needs a per-turn
        // anchor kept for every turn, not just these.
        let anchor = view.phone_turn().map_or(0, PhoneTurn::items_anchor);
        // Skips a message with no text for the same reason `first_response`
        // does: it would put an empty preview under the notification header.
        let last_response = view.items()[anchor.min(view.items().len())..]
            .iter()
            .rev()
            .find_map(|item| match item {
                daruda_acp::ChatItem::AssistantText {
                    text, message_id, ..
                } if !text.trim().is_empty() => Some((text.as_str(), message_id.as_deref())),
                _ => None,
            });
        match last_response {
            Some((_, message_id))
                if view
                    .phone_turn()
                    .is_some_and(|turn| turn.already_sent(message_id)) =>
            {
                None
            }
            Some((text, _)) => Some((
                header,
                MessageTail::Markdown(preview_for(
                    text,
                    &s::notification::remote_truncated_marker(),
                )),
            )),
            None => Some((
                header,
                MessageTail::Plain(s::notification::agent_completed()),
            )),
        }
    }

    /// End the turn's phone conversation. Separate from
    /// [`Self::remote_completion_parts`] because that one is a question and
    /// this is the effect: composing a ping must not be what retires the turn,
    /// or a second reader of the same question would silently retire it.
    pub(super) fn close_phone_turn(&self, pane_id: PaneId, cx: &mut Context<Self>) {
        let Some(view) = self.agent_chat_view(pane_id).cloned() else {
            return;
        };
        view.update(cx, |v, _| v.end_phone_turn());
    }

    /// Relay a permission-wait ping with one button per option the agent
    /// offered — skips the relay entirely (not a broken partial ping) if
    /// the agent supplied no options at all (shouldn't happen for a real
    /// permission request, but nothing to build a keyboard from if it
    /// somehow does). The prompt is the same `daruda_acp::PermissionItem`
    /// the in-app card is built
    /// from — see [`permission_wait_tail`] — so the phone ping names the
    /// actual action awaiting approval instead of just "waiting for your
    /// input". A permission wait during a phone-started turn bypasses
    /// presence — the first visible response directly here, later ones
    /// through the ledger check in [`Self::relay_when_presence_allows`].
    /// The tail stays plain unless a file diff is available. With a diff, the
    /// administrative text is escaped and the preview alone is fenced.
    pub(super) fn relay_permission_wait_to_remote(
        &mut self,
        pane_id: PaneId,
        prompt: &daruda_acp::PermissionItem,
        cx: &mut Context<Self>,
    ) {
        let is_phone_first_response = self
            .agent_chat_view(pane_id)
            .is_some_and(|view| view.read(cx).is_phone_turn_waiting());
        let buttons = permission_buttons(&prompt.options);
        if buttons.is_empty() {
            if is_phone_first_response {
                self.relay_first_response_fallback_to_remote(pane_id, cx);
            }
            return;
        }
        let tail = self
            .agent_chat_view(pane_id)
            .map(|view| {
                let view = view.read(cx);
                permission_wait_tail(prompt, permission_diffs(view, &prompt.tool_call_id))
            })
            .unwrap_or_else(|| permission_wait_tail(prompt, &[]));
        let header = self.remote_header(pane_id, cx);
        let permission = Some(crate::remote_channel::bridge::PingPrompt::Permission(
            crate::remote_channel::bridge::PermissionPromptRef {
                session: self
                    .agent_chat_view(pane_id)
                    .map(|view| view.read(cx).permission_session())
                    .unwrap_or_default(),
                perm_id: prompt.id,
                buttons,
            },
        ));
        let told = if is_phone_first_response {
            self.relay_to_remote(pane_id, header, tail, permission, cx)
        } else {
            self.relay_when_presence_allows(pane_id, header, tail, permission, cx)
        };
        if told {
            self.mark_permission_told_to_phone(pane_id, prompt.id, cx);
        }
    }

    /// Record that the phone has been shown this permission, so the periodic
    /// re-ask leaves it alone.
    fn mark_permission_told_to_phone(&self, pane_id: PaneId, perm_id: u64, cx: &mut Context<Self>) {
        let Some(view) = self.agent_chat_view(pane_id).cloned() else {
            return;
        };
        view.update(cx, |v, _| v.mark_permission_told_to_phone(perm_id));
    }

    /// Re-offer permission prompts and questions the phone has never been
    /// shown, for every pane whose request is still outstanding.
    ///
    /// This is not the deferral queue coming back. A completion ping reports a
    /// past event, so holding one and flushing it later delivers something
    /// stale; a permission request is a *live blocking state* — the agent is
    /// stopped until someone answers. A request still outstanding when absence
    /// is first detected is as true then as when it fired, so relaying it is a
    /// fresh ping about the present, not a late one about the past. Nothing is
    /// stored but the request id: the text is recomposed from the live card,
    /// so a resolved request simply stops appearing.
    ///
    /// Level-triggered on the periodic pump, mirroring
    /// [`Self::flush_remote_first_response_fallbacks`].
    pub(crate) fn relay_outstanding_permissions(&mut self, cx: &mut Context<Self>) {
        let panes: Vec<PaneId> = self.every_agent_chat().map(|(id, _)| id).collect();
        for pane_id in panes {
            let Some(view) = self.agent_chat_view(pane_id).cloned() else {
                continue;
            };
            if let Some(exec) =
                crate::runs::RunsHost::pane_execution(self.run_pane(pane_id), cx).cloned()
            {
                let waiting = match view.read(cx).activity_state() {
                    daruda_chat::view::ActivityState::AwaitingPermission => {
                        Some(crate::runs::result::WaitingOn::Permission { tool: None })
                    }
                    daruda_chat::view::ActivityState::AwaitingAnswer => view
                        .read(cx)
                        .pending_question_summary()
                        .map(|question| crate::runs::result::WaitingOn::Answer { question }),
                    _ => None,
                };
                crate::runs::RunsHost::observe_task_waiting(
                    crate::runs::result::RunKey::Task(exec),
                    waiting,
                    cx,
                );
            }
            let untold = view.update(cx, |v, _| v.take_permissions_untold_to_phone());
            for prompt in untold {
                self.relay_permission_wait_to_remote(pane_id, &prompt, cx);
            }
            self.relay_untold_questions(pane_id, cx);
        }
    }

    /// Presence-gated entry point for the completion and permission relays.
    /// Asks `app_presence` whether the user is away and acts on the answer
    /// immediately: sent, or dropped for good. Nothing is queued, so a
    /// ping's send time is always its settle time — a later absence does not
    /// resurrect a ping this call declined.
    ///
    /// First-response acks and first-response permission waits deliberately
    /// bypass this and call [`Self::relay_to_remote`] directly: the phone
    /// asked for those, so presence is not what decides them. The same
    /// reasoning covers the rest of a phone-started turn — while the pane's
    /// [`PhoneTurn`] ledger is open, every relay here is solicited too, and
    /// goes out the same way. Presence decides only the unsolicited.
    ///
    /// Returns whether a channel took the ping, so a caller tracking a live
    /// state (an outstanding permission) knows whether the phone has been
    /// told — a refused send leaves it for the next sweep.
    pub(super) fn relay_when_presence_allows(
        &mut self,
        pane_id: PaneId,
        header: String,
        tail: MessageTail,
        prompt: Option<crate::remote_channel::bridge::PingPrompt>,
        cx: &mut Context<Self>,
    ) -> bool {
        // The ledger closes at the completion tee, after the completion relay
        // has run — so a completion and a second permission wait both land
        // here with it still open.
        let away = crate::app_presence::is_away(cx);
        let delivery = crate::remote_channel::delivery::remote_delivery(
            self.phone_turn_open(pane_id, cx),
            away,
        );
        if matches!(delivery, crate::remote_channel::global::Delivery::Explicit) {
            trace::delivery("relay.solicited", || {
                format!("pane={pane_id} text={}", trace::tail_digest(&tail))
            });
            return self.relay_ping(pane_id, header, tail, prompt, delivery, cx);
        }
        // Asked unconditionally, so the trace records what presence actually
        // was even where a channel's opt-out decided — the alternative logs a
        // sample from the last pump tick.
        let state = crate::app_presence::snapshot(cx);
        let away_secs = state.away_secs(Instant::now());
        // The verdict that precedes `relay.send`, and the only record of a
        // presence drop. The rule is logged alongside the readings because it
        // is read live from config — a log reader has no other way to recover
        // what they were compared to.
        let rule = crate::app_presence::rule(cx);
        trace::delivery("relay.presence", || {
            format!(
                "pane={pane_id} away={away} \
                 away_secs={away_secs:?} idle_secs={} away_grace_secs={} \
                 away_idle_secs={} away_idle_foreground_secs={} {} text={}",
                trace::opt(state.idle().map(|i| i.as_secs())),
                rule.grace.as_secs(),
                rule.idle_bar.as_secs(),
                rule.foreground_idle_bar.as_secs(),
                trace::presence(
                    away_secs.is_none(),
                    self.lane_ref_for_pane(pane_id),
                    self.active_ref()
                ),
                trace::tail_digest(&tail)
            )
        });
        self.relay_ping(pane_id, header, tail, prompt, delivery, cx)
    }

    /// Whether the pane's in-flight turn was started from the phone. Any
    /// ledger state counts, not just `Waiting`: an answered first response
    /// does not make the rest of the turn unsolicited. The ledger lives as
    /// long as the activity span the phone started, so a desk prompt queued
    /// behind it shares that span and its delivery — the span settles once,
    /// and the phone hears how it ended.
    pub(crate) fn phone_turn_open(&self, pane_id: PaneId, cx: &App) -> bool {
        self.agent_chat_view(pane_id)
            .is_some_and(|view| view.read(cx).phone_turn().is_some())
    }

    /// Relay a ping every channel hears, whatever presence says: the phone
    /// asked for it. Returns whether any channel took the ping.
    pub(super) fn relay_to_remote(
        &self,
        pane_id: PaneId,
        header: String,
        tail: MessageTail,
        prompt: Option<crate::remote_channel::bridge::PingPrompt>,
        cx: &Context<Self>,
    ) -> bool {
        self.relay_ping(
            pane_id,
            header,
            tail,
            prompt,
            crate::remote_channel::global::Delivery::Explicit,
            cx,
        )
    }

    /// Hand a pane's ping to the remote-channel facade, which applies each
    /// channel's own `only_when_away` and pairing. Whether any channel took it.
    fn relay_ping(
        &self,
        pane_id: PaneId,
        header: String,
        tail: MessageTail,
        prompt: Option<crate::remote_channel::bridge::PingPrompt>,
        delivery: crate::remote_channel::global::Delivery,
        cx: &Context<Self>,
    ) -> bool {
        let text = trace::is_on().then(|| trace::tail_digest(&tail));
        let sent = crate::remote_channel::send_ping(
            crate::remote_channel::bridge::BridgePing {
                pane: crate::remote_channel::bridge::PaneRef {
                    workspace: self.uuid(),
                    pane: pane_id,
                },
                header,
                tail,
                prompt,
            },
            delivery,
            cx,
        );
        trace::delivery("relay.send", || {
            format!(
                "pane={pane_id} delivery={delivery:?} sent={sent} {} text={}",
                trace::presence(
                    crate::platform::attention::is_app_active(),
                    self.lane_ref_for_pane(pane_id),
                    self.active_ref()
                ),
                trace::opt(text)
            )
        });
        sent
    }

    /// Relay an unsolicited, pane-less notice under each channel's presence
    /// rule. Hook notifications use this shape because a terminal permission
    /// cannot be answered through the hook protocol, so no button is valid.
    pub(in crate::workspace) fn relay_presence_notice_to_phone(
        &self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let away = crate::app_presence::is_away(cx);
        let digest = trace::is_on().then(|| trace::digest(&text));
        let sent = crate::remote_channel::send_notice(
            text,
            crate::remote_channel::global::Delivery::Presence { away },
            cx,
        );
        trace::delivery("relay.presence_notice", || {
            format!("away={away} sent={sent} text={}", trace::opt(digest))
        });
    }

    /// The ping a turn that ended in an error sends: the usual who-is-this
    /// header, then the failure lead line and why. Our own copy, so plain.
    pub(super) fn remote_failure_parts(
        &self,
        pane_id: PaneId,
        cx: &Context<Self>,
    ) -> (String, MessageTail) {
        let mut tail = s::control::turn_failed();
        if let Some(reason) = self
            .agent_chat_view(pane_id)
            .and_then(|view| view.read(cx).failure_reason())
        {
            tail.push('\n');
            tail.push_str(&preview_for(
                &reason,
                &s::notification::remote_truncated_marker(),
            ));
        }
        (self.remote_header(pane_id, cx), MessageTail::Plain(tail))
    }

    /// Send the "queued behind the current turn" notice — fires the instant
    /// `AgentChatView::send_prompt_text_from_phone` reports
    /// [`PromptDispatch::Queued`], since a queued reply hasn't reached the
    /// agent yet and there is nothing to watch a first response for. Plain
    /// tail (fixed i18n copy). Goes through [`Self::relay_to_remote`], so
    /// the bridge gate applies but presence does not.
    ///
    /// [`PromptDispatch::Queued`]: daruda_chat::view::PromptDispatch::Queued
    pub(super) fn relay_queued_notice_to_remote(&self, pane_id: PaneId, cx: &Context<Self>) {
        let header = self.remote_header(pane_id, cx);
        self.relay_to_remote(
            pane_id,
            header,
            MessageTail::Plain(s::notification::remote_reply_queued()),
            None,
            cx,
        );
    }

    /// Send the "queue is full" notice. Same shape as the queued notice next
    /// door, but a different fact: that one says "later", this one says "not
    /// at all".
    pub(super) fn relay_queue_full_notice_to_remote(&self, pane_id: PaneId, cx: &Context<Self>) {
        let header = self.remote_header(pane_id, cx);
        self.relay_to_remote(
            pane_id,
            header,
            MessageTail::Plain(s::notification::remote_queue_full()),
            None,
            cx,
        );
    }

    /// Relay a phone-triggered turn's resolved first response: the agent's own
    /// text (markdown, truncated like the completion ping) or a fixed
    /// "checking via tool" note naming it. Bypasses the presence-defer gate
    /// (calls `relay_to_remote` directly) — the whole point of this relay is
    /// a phone-originated interaction, so instant delivery is what the sender
    /// wants regardless of whether the app happens to be foreground right now.
    pub(super) fn relay_first_response_to_remote(
        &self,
        pane_id: PaneId,
        outcome: FirstResponseOutcome,
        cx: &Context<Self>,
    ) {
        let header = self.remote_header(pane_id, cx);
        let tail = match outcome {
            FirstResponseOutcome::Text { text, .. } => MessageTail::Markdown(preview_for(
                &text,
                &s::notification::remote_truncated_marker(),
            )),
            FirstResponseOutcome::Tool { tool_title } => {
                MessageTail::Plain(first_tool_ack_tail(tool_title.as_deref()))
            }
        };
        self.relay_to_remote(pane_id, header, tail, None, cx);
    }

    /// Relay the fixed fallback ack for a phone-triggered turn that went 60s
    /// without producing text or a tool call, or settled with nothing having
    /// appeared at all. Plain tail (fixed i18n copy, not agent-authored
    /// markdown) — the same copy the old always-immediate ack used. Gated by
    /// `relay_to_remote`.
    pub(super) fn relay_first_response_fallback_to_remote(
        &self,
        pane_id: PaneId,
        cx: &Context<Self>,
    ) {
        let header = self.remote_header(pane_id, cx);
        self.relay_to_remote(
            pane_id,
            header,
            MessageTail::Plain(s::notification::remote_reply_ack()),
            None,
            cx,
        );
    }

    /// Forget, in every chat, which waiting requests the phone was shown: the
    /// identity that showed them no longer serves those cards
    /// ([`crate::remote_channel::offer_unseen_cards_again`]).
    pub(crate) fn forget_told_to_phone(&mut self, cx: &mut Context<Self>) {
        for (_, view) in self.every_agent_chat() {
            view.update(cx, |view, _| view.forget_told_to_phone());
        }
    }

    /// Periodic safety net for phone-triggered turns that produce no completed
    /// assistant text or tool call within [`FIRST_RESPONSE_FALLBACK_SECS`].
    /// The view moves each turn it takes to `Answered`, so a pane can emit
    /// this fallback at most once per phone-dispatched turn.
    pub(crate) fn flush_remote_first_response_fallbacks(&mut self, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let panes: Vec<PaneId> = self.every_agent_chat().map(|(id, _)| id).collect();

        for pane_id in panes {
            let Some(view) = self.agent_chat_view(pane_id).cloned() else {
                continue;
            };
            let overdue = view.update(cx, |v, _| {
                v.take_phone_fallback_if_overdue(now, FIRST_RESPONSE_FALLBACK_SECS)
            });
            if overdue {
                self.relay_first_response_fallback_to_remote(pane_id, cx);
            }
        }
    }

    /// The workspace's persisted identity — needed by cross-cutting
    /// App-level services (e.g. the Telegram bridge,
    /// `crate::telegram::global`) that route by `WorkspaceUuid` since
    /// `PaneId` alone is only unique within one workspace, not across
    /// all open windows.
    ///
    /// Also how the open paths avoid giving two windows one persisted
    /// record — see `WindowRegistry::workspace_window_for_uuid`.
    pub(crate) fn uuid(&self) -> daruda_store::project::WorkspaceUuid {
        self.uuid
    }
}

#[cfg(test)]
mod notify_tests;
#[cfg(test)]
mod question_tests;
#[cfg(test)]
mod tests;
