//! A claimed tap on a page's button: select or stop the pane it names, as
//! `/use` and `/stop` would, and show what that came to on the page itself.

use super::super::board::Outcome;
use super::super::token::{Act, Tap};
use crate::remote_channel::dispatch::{self, Effect};
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::threads::Scope;
use crate::surface::strings as s;
use gpui::{App, AsyncApp};
use std::sync::Arc;

/// The paired person tapped the button carrying `data` on connection `id`'s
/// page, and `bot` of the worker running `generation` claimed the tap. The
/// result is shown on the page: the conversation is not where the person is
/// looking.
///
/// Acted on whether or not the connection can send right now, as a claimed
/// message is: a reconnect does not lose the tap, and the page showing its
/// result goes out once the connection sends again.
pub(crate) async fn tapped(
    id: &str,
    generation: uuid::Uuid,
    bot: &Arc<BotSession>,
    user_id: &str,
    team_id: &str,
    data: &str,
    cx: &mut AsyncApp,
) {
    let Some((scope, acted)) = cx.update(|cx| act(id, generation, bot, user_id, team_id, data, cx))
    else {
        return;
    };
    let text = match acted {
        Acted::Said(text) => text,
        Acted::Stopping(effect) => said(effect.settled().await),
    };
    cx.update(|cx| {
        // The connection may have gone, or changed identity, while the stop
        // settled; a page made under the old identity is not shown again.
        if !super::identity_holds(&scope, cx) {
            return;
        }
        let outcome = text.map(|text| Outcome {
            at: chrono::Utc::now().timestamp(),
            text,
        });
        cx.global_mut::<RemoteChannels>()
            .home
            .queue
            .tapped(&scope, outcome);
        // Built anew, so the person also sees the panes as they are now.
        super::changed(cx);
    });
}

/// What a tap did at once, or the stop it started.
enum Acted {
    Said(Option<String>),
    Stopping(Effect),
}

fn act(
    id: &str,
    generation: uuid::Uuid,
    bot: &Arc<BotSession>,
    user_id: &str,
    team_id: &str,
    data: &str,
    cx: &mut App,
) -> Option<(Scope, Acted)> {
    if !cx.has_global::<RemoteChannels>() {
        return None;
    }
    let scope = {
        let channels = cx.global::<RemoteChannels>();
        let connection = channels.live(id, cx)?;
        if !connection.runs(generation, bot) {
            return None;
        }
        super::person_scope(connection, bot, user_id, team_id)?
    };
    let tapped = Tap::parse(data).and_then(|tap| {
        let queue = &cx.global::<RemoteChannels>().home.queue;
        Some((queue.resolve(&scope, tap)?, tap.act))
    });
    let target = dispatch::Target::Remote(id.into());
    let acted = match tapped {
        // The page is built anew, so the person sees what is there now.
        None => Acted::Said(Some(s::remote_channel::home_stale())),
        Some((pane, Act::Select)) => Acted::Said(dispatch::select_live(pane, &target, cx)),
        Some((pane, Act::Stop)) => Acted::Stopping(dispatch::stop_pane(pane, &target, cx)),
    };
    Some((scope, acted))
}

/// The words a settled effect has for the person, if any.
fn said(effect: Effect) -> Option<String> {
    match effect {
        Effect::Reply(reply) => Some(reply.text),
        Effect::Feedback { label, .. } => Some(label),
        Effect::None | Effect::Later(_) => None,
    }
}
