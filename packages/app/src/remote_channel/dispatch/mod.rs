//! Execute a routed action once, independently of its delivery transport.

use super::{
    bridge::{BotPermissionOutcome, BotQuestionOutcome, InboundAction, PaneRef, QuestionReply},
    command::{self as commands, CommandState, RenderedReply},
};
use crate::control::{result::ControlError, spec::ParseError};
use crate::surface::strings as s;
use gpui::App;

mod command;
pub(crate) use command::{select_live, stop_pane};
pub(crate) mod target;
pub(crate) use target::Aimed;
use target::{dispatch_to_workspace, permission_feedback};

#[derive(Clone)]
pub enum Target {
    Telegram,
    Remote(String),
}

impl Target {
    /// `None` once the bridge global is gone or the connection was revoked —
    /// an answer with no table to resolve against is dropped, not panicked on.
    fn state<'a>(&self, cx: &'a mut App) -> Option<&'a mut CommandState> {
        match self {
            Self::Telegram => {
                if !cx.has_global::<crate::telegram::global::TelegramBridge>() {
                    return None;
                }
                Some(
                    cx.global_mut::<crate::telegram::global::TelegramBridge>()
                        .command_state_mut(),
                )
            }
            Self::Remote(id) => {
                if !cx.has_global::<super::global::RemoteChannels>() {
                    return None;
                }
                Some(
                    cx.global_mut::<super::global::RemoteChannels>()
                        .connections
                        .get_mut(id)?
                        .core
                        .command_state_mut(),
                )
            }
        }
    }

    /// The surface a tap on this recipient's card came from. `None` once the
    /// connection is gone, like [`Self::state`].
    fn answerer(&self, cx: &App) -> Option<crate::control::approval::Answerer> {
        use crate::control::approval::Answerer;
        match self {
            Self::Telegram => Some(Answerer::Telegram),
            Self::Remote(id) => {
                let kind = cx
                    .try_global::<super::global::RemoteChannels>()?
                    .connections
                    .get(id)?
                    .config
                    .kind;
                Some(match kind {
                    daruda_config::remote::ChannelKind::Slack => Answerer::Slack,
                    daruda_config::remote::ChannelKind::Discord => Answerer::Discord,
                })
            }
        }
    }

    /// Point this recipient's commands at `pane`, then hand back the table the
    /// reply renders against. `None` like [`Self::state`].
    fn select<'a>(&self, pane: Option<PaneRef>, cx: &'a mut App) -> Option<&'a mut CommandState> {
        match self {
            Self::Telegram => {
                let state = self.state(cx)?;
                state.select(pane);
                Some(state)
            }
            Self::Remote(id) => super::global::RemoteChannels::select(id, pane, cx),
        }
    }

    /// Forget a gone pane on this recipient's routing, then hand back the
    /// table the reply renders against.
    fn forget<'a>(
        &self,
        pane: PaneRef,
        cx: &'a mut App,
    ) -> Option<(commands::Absorbed, &'a mut CommandState)> {
        match self {
            Self::Telegram => {
                if !cx.has_global::<crate::telegram::global::TelegramBridge>() {
                    return None;
                }
                let bridge = cx.global_mut::<crate::telegram::global::TelegramBridge>();
                let absorbed = bridge.forget_target(pane);
                Some((absorbed, bridge.command_state_mut()))
            }
            Self::Remote(id) => {
                if !cx.has_global::<super::global::RemoteChannels>() {
                    return None;
                }
                let core = &mut cx
                    .global_mut::<super::global::RemoteChannels>()
                    .connections
                    .get_mut(id)?
                    .core;
                let absorbed = core.forget(pane);
                Some((absorbed, core.command_state_mut()))
            }
        }
    }
}

pub enum Effect {
    None,
    Reply(RenderedReply),
    Feedback {
        label: String,
        edit: Edit,
    },
    /// A reply owed once the work it waits on has landed; the inbound loop
    /// awaits it through [`Effect::settled`] before taking the next update.
    Later(gpui::Task<Option<RenderedReply>>),
}

impl Effect {
    /// A plain answer, with no buttons.
    pub(crate) fn text(text: String) -> Self {
        Self::Reply(RenderedReply {
            text,
            keyboard: None,
        })
    }

    /// The effect once any wait is over.
    pub async fn settled(self) -> Self {
        match self {
            Self::Later(reply) => reply.await.map_or(Self::None, Self::Reply),
            now => now,
        }
    }
}

#[derive(PartialEq, Eq)]
pub enum Edit {
    /// The buttons stay usable, as on a listing.
    KeepButtons,
    /// The card's ledger edits it when its decision settles.
    ManagedCard,
    /// The token named nothing live. Unless a settlement edit owns the
    /// message, its buttons are taken off so they stop inviting taps.
    StaleCard,
}

pub(crate) fn handle_in_app(aimed: Aimed, target: &Target, cx: &mut App) -> Effect {
    let Aimed { action, adopted } = aimed;
    match action {
        InboundAction::RunCommand { command: requested } => {
            command::run_command(requested, target, cx)
        }
        InboundAction::ReportParseError { error } => {
            Effect::Reply(commands::render_parse_error(&error))
        }
        InboundAction::NoTarget => {
            command::render_outcome(&Err(ControlError::NoTargetSelected), target, cx)
                .map_or(Effect::None, Effect::Reply)
        }
        InboundAction::InjectPrompt { pane, text } => {
            deliver_prompt(pane, text, adopted, target, cx)
        }
        InboundAction::UnknownSlash {
            pane,
            name,
            text,
            suggestion,
        } => {
            let mut agents = true;
            dispatch_to_workspace(cx, pane.workspace, |ws, cx| {
                agents = ws.agent_takes_slash_command(pane.pane, &name, cx);
            });
            if agents {
                deliver_prompt(pane, text, adopted, target, cx)
            } else {
                Effect::Reply(commands::render_parse_error(&ParseError::Unknown {
                    input: name,
                    suggestion,
                }))
            }
        }
        InboundAction::UnclaimedSlash { name, suggestion } => {
            let ours = crate::control::resolve::slash_claim(cx, &name).rules_out();
            if ours {
                Effect::Reply(commands::render_parse_error(&ParseError::Unknown {
                    input: name,
                    suggestion,
                }))
            } else {
                command::render_outcome(&Err(ControlError::NoTargetSelected), target, cx)
                    .map_or(Effect::None, Effect::Reply)
            }
        }
        InboundAction::RespondPermission {
            pane,
            session,
            perm_id,
            decision,
        } => {
            let mut outcome = BotPermissionOutcome::Gone;
            dispatch_to_workspace(cx, pane.workspace, |ws, cx| {
                outcome =
                    ws.respond_bot_permission(pane.pane, session, perm_id, decision.clone(), cx);
            });
            Effect::Feedback {
                label: permission_feedback(&decision, outcome),
                edit: Edit::ManagedCard,
            }
        }
        InboundAction::RespondQuestion {
            pane,
            question,
            reply,
        } => {
            let mut outcome = BotQuestionOutcome::Gone;
            dispatch_to_workspace(cx, pane.workspace, |ws, cx| {
                outcome = ws.respond_bot_question(pane.pane, question, reply.clone(), cx);
            });
            question_effect(&reply, outcome)
        }
        InboundAction::SelectTarget { pane } => {
            command::select_target(pane, target, cx).map_or(Effect::None, |label| {
                Effect::Feedback {
                    label,
                    edit: Edit::KeepButtons,
                }
            })
        }
        InboundAction::ResolveApproval { id, choice } => {
            let Some(by) = target.answerer(cx) else {
                return Effect::None;
            };
            let applied = crate::control::approval::resolve(id, choice, by, cx);
            let label = match (applied, choice) {
                (false, _) => s::control::approval_already_answered(),
                (true, crate::control::approval::ApprovalChoice::Approved) => {
                    s::control::approval_allowed()
                }
                (true, crate::control::approval::ApprovalChoice::Refused) => {
                    s::control::approval_refused()
                }
            };
            Effect::Feedback {
                label,
                edit: Edit::ManagedCard,
            }
        }
        InboundAction::ResolveFlowAsk {
            worktree,
            run_dir,
            ask_id,
            decision,
        } => {
            let decision = match decision {
                super::bridge::PermissionDecision::Allow(option_id) => {
                    daruda_acp::PermissionDecision::Allow { option_id }
                }
                super::bridge::PermissionDecision::Reject(option_id) => {
                    daruda_acp::PermissionDecision::Reject { option_id }
                }
            };
            let answered =
                crate::runs::RunsHost::answer_flow_ask(&worktree, &run_dir, ask_id, decision, cx);
            Effect::Feedback {
                label: flow_answer_label(answered),
                edit: Edit::ManagedCard,
            }
        }
        InboundAction::StaleApproval => Effect::Feedback {
            label: s::control::approval_already_answered(),
            edit: Edit::StaleCard,
        },
        InboundAction::StaleQuestion => Effect::text(s::notification::remote_question_stale()),
        InboundAction::StaleListing => Effect::Feedback {
            label: s::control::listing_stale(),
            edit: Edit::KeepButtons,
        },
        InboundAction::Ignore | InboundAction::Unsupported | InboundAction::Paired { .. } => {
            Effect::None
        }
    }
}

/// What the phone is told about its answer to an agent's question. A tap is
/// answered on its button; a text reply that was taken needs no word, since
/// the next card or the card's settlement says it, but one that was refused
/// gets a reply so it does not read as accepted.
fn question_effect(reply: &QuestionReply, outcome: BotQuestionOutcome) -> Effect {
    let label = match outcome {
        BotQuestionOutcome::Next => s::notification::remote_question_next(),
        BotQuestionOutcome::Updated => s::notification::remote_question_updated(),
        BotQuestionOutcome::Submitted => s::notification::remote_question_answered(),
        BotQuestionOutcome::Skipped => s::notification::remote_question_skipped(),
        BotQuestionOutcome::AlreadySettled => s::notification::remote_question_stale(),
        BotQuestionOutcome::TextNotTaken => s::notification::remote_question_text_not_taken(),
        BotQuestionOutcome::Gone => s::notification::remote_permission_gone(),
    };
    match (reply, outcome) {
        (QuestionReply::Button(_), _) => Effect::Feedback {
            label,
            edit: Edit::ManagedCard,
        },
        (
            QuestionReply::Text(_),
            BotQuestionOutcome::Next
            | BotQuestionOutcome::Updated
            | BotQuestionOutcome::Submitted
            | BotQuestionOutcome::Skipped,
        ) => Effect::None,
        (QuestionReply::Text(_), _) => Effect::text(label),
    }
}

/// What a tap on a flow question's button is told.
fn flow_answer_label(answered: Result<(), crate::runs::AnswerRefused>) -> String {
    use crate::runs::AnswerRefused;
    match answered {
        Ok(()) => s::flow::ask_answered(),
        Err(AnswerRefused::AlreadyAnswered) => s::flow::ask_already_answered(),
        Err(AnswerRefused::NotOpen | AnswerRefused::Closed) => s::flow::ask_closed(),
    }
}

/// Put a phone-sent prompt on its pane, and answer for it when the sender
/// cannot see what happened.
///
/// Two of those cases. The pane is gone, so nothing was delivered; or it was
/// delivered to a chat `aim` lent rather than one the sender picked, which is
/// silent in exactly the way that starts a turn on the wrong agent.
fn deliver_prompt(
    pane: PaneRef,
    text: String,
    adopted: Option<PaneRef>,
    target: &Target,
    cx: &mut App,
) -> Effect {
    let mut delivered = false;
    dispatch_to_workspace(cx, pane.workspace, |ws, cx| {
        delivered = ws.inject_bot_reply(pane.pane, text.clone(), cx);
    });
    if !delivered {
        return command::report_target_gone(pane, target, cx).map_or(Effect::None, Effect::Reply);
    }
    adopted
        .and_then(|lent| target::lent_target_reply(lent, cx))
        .map_or(Effect::None, Effect::Reply)
}

#[cfg(test)]
pub(crate) mod tests;
