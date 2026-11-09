//! Who answers a `/name` daruda does not own: what each pane's agent
//! advertises, folded across a window.

use gpui::App;

use crate::workspace::Workspace;
use crate::workspace::main_area::pane_tree::PaneId;
use daruda_chat::view::AgentChatView;

/// What an agent says about a `/name` daruda does not own.
///
/// Three-valued because the two questions asked of it want different things
/// from the middle. "The session has not advertised yet" cannot claim the
/// name, but it cannot rule it out either, and folding that into either
/// answer makes one of the callers wrong: as a claim, one cold pane vetoes
/// every name for the whole window — and after a restart every pane but the
/// focused one is cold, since a restored chat stays `Idle` until first focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlashClaim {
    /// The agent advertises the name. It is not ours to answer.
    Claims,
    /// The agent has advertised its list and the name is not on it.
    Disclaims,
    /// Nobody has said — cold, connecting, or no agent at all. An abstention:
    /// it never outvotes an agent that did answer, and on its own it earns no
    /// conclusion in either direction.
    Unsaid,
}

impl SlashClaim {
    /// Fold two answers. A claim anywhere settles it; failing that a
    /// disclaimer counts; silence yields to either.
    pub(crate) fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Claims, _) | (_, Self::Claims) => Self::Claims,
            (Self::Disclaims, _) | (_, Self::Disclaims) => Self::Disclaims,
            (Self::Unsaid, Self::Unsaid) => Self::Unsaid,
        }
    }

    /// Whether this answer earns the typo suggestion: some agent positively
    /// ruled the name out, and none claimed it. Silence alone never does —
    /// the honest answer with no evidence is the missing target.
    pub(crate) fn rules_out(self) -> bool {
        self == Self::Disclaims
    }
}

impl Workspace {
    /// Whether a `/name` from outside should go to this pane's agent.
    ///
    /// The agent advertises its own slash commands over ACP, and the in-app
    /// completion menu already offers them from this same list — so a phone
    /// answering "unknown command" for one the menu would happily complete is
    /// the surface disagreeing with itself.
    ///
    /// A pane that has not advertised yet forwards too — [`SlashClaim::Unsaid`]
    /// is not "it does not have that command", and a name daruda does not own
    /// is far likelier the agent's than a typo of one of ours. So does a pane
    /// that is not there, so the answer comes from the delivery attempt, which
    /// can say `target_gone`, rather than from a guess here.
    ///
    /// Asked about a *named* pane, so it answers about that pane whatever it
    /// is — including the orchestrator's, which can be `last_pinged` and so
    /// can be the target of a reply. [`Self::rules_out_slash_command`] asks
    /// about the population instead and scopes itself accordingly.
    pub(crate) fn agent_takes_slash_command(&self, pane: PaneId, name: &str, cx: &App) -> bool {
        match self.agent_chat_view(pane) {
            Some(view) => pane_slash_claim(view.read(cx), name) != SlashClaim::Disclaims,
            None => true,
        }
    }

    /// What this window's agents say about `/name` — asked when nothing names
    /// a target, so there is no one pane to ask.
    ///
    /// Scoped to [`Self::lane_agent_chats`]: every lane's chats, minus the
    /// orchestrator. The orchestrator exclusion is the part
    /// [`Self::fallback_agent_chat`] shares — an unowned slash can only ever
    /// be delivered to a pane `/list` offers, so a chat the phone is never
    /// shown must not decide the answer, and being usually cold and hidden it
    /// would decide every one.
    ///
    /// The lane scope deliberately does *not* match: a fallback target must
    /// come from the lane the user is in, but a vocabulary answer is about
    /// what the agents know, and `/list` offers every lane's chat. A
    /// background lane that claims the name suppresses the typo answer, which
    /// is the conservative direction.
    ///
    /// Folded rather than reduced to a bool because silence has to stay
    /// distinguishable: see [`SlashClaim`].
    pub(crate) fn slash_claim(&self, name: &str, cx: &App) -> SlashClaim {
        self.lane_agent_chats()
            .map(|(_, view)| pane_slash_claim(view.read(cx), name))
            .fold(SlashClaim::Unsaid, SlashClaim::merge)
    }

    /// Test hook: put `names` on `pane`'s advertised command list. Slash
    /// ownership turns on that one field, and a test outside
    /// `crate::workspace` cannot reach the view that holds it.
    #[cfg(test)]
    pub(crate) fn advertise_slash_commands_for_test(
        &self,
        pane: PaneId,
        names: &[&str],
        cx: &mut App,
    ) {
        let view = self.agent_chat_view(pane).expect("pane").clone();
        let commands = names
            .iter()
            .map(|n| daruda_acp::SlashCommand {
                name: (*n).to_string(),
                description: String::new(),
                input: daruda_acp::SlashCommandInput::NoInput,
            })
            .collect();
        view.update(cx, |v, _| v.advertise_commands_for_test(commands));
    }
}

/// What one pane's agent says about `/name`: an empty advertised list is
/// [`SlashClaim::Unsaid`] (the session has not spoken, which is neither a
/// claim nor a denial), a list naming it is `Claims`, and any other list is
/// `Disclaims`. The one predicate both slash-ownership questions aggregate —
/// one pane's answer, and every pane's.
fn pane_slash_claim(view: &AgentChatView, name: &str) -> SlashClaim {
    let advertised = view.session_config().available_commands();
    if advertised.is_empty() {
        SlashClaim::Unsaid
    } else if advertised.iter().any(|c| c.name == name) {
        SlashClaim::Claims
    } else {
        SlashClaim::Disclaims
    }
}

#[cfg(test)]
mod tests {
    use super::SlashClaim::{self, Claims, Disclaims, Unsaid};

    /// `merge` is folded over panes and again over windows, so the answer must
    /// not depend on how the panes were grouped or ordered, and an empty fold
    /// must come out `Unsaid`. Associative, commutative, idempotent, with
    /// `Unsaid` as the identity — checked exhaustively, since there are nine
    /// pairs.
    #[test]
    fn merge_is_a_semilattice_with_unsaid_as_identity() {
        let all = [Claims, Disclaims, Unsaid];
        for a in all {
            assert_eq!(a.merge(Unsaid), a, "identity");
            assert_eq!(a.merge(a), a, "idempotent");
            for b in all {
                assert_eq!(a.merge(b), b.merge(a), "commutative");
                for c in all {
                    assert_eq!(
                        a.merge(b).merge(c),
                        a.merge(b.merge(c)),
                        "associative: grouping must not change the answer"
                    );
                }
            }
        }
        // Precedence, stated once rather than inferred from the fold.
        assert_eq!(Claims.merge(Disclaims), Claims, "a claim outranks a denial");
        assert_eq!(
            Disclaims.merge(Unsaid),
            Disclaims,
            "an answer outranks silence"
        );
    }

    /// Only a positive denial earns the suggestion.
    #[test]
    fn only_disclaims_rules_out() {
        assert!(SlashClaim::Disclaims.rules_out());
        assert!(!SlashClaim::Claims.rules_out());
        assert!(!SlashClaim::Unsaid.rules_out());
    }
}
