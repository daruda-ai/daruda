//! A claimed tap on a page's button: select or stop the pane it names, as
//! `/use` and `/stop` would, and show what that came to on the page itself.

use super::super::board::Outcome;
use super::super::token::{Act, Tap};
use crate::remote_channel::dispatch::{self, Effect};
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::threads::Scope;
use crate::surface::strings as s;
use gpui::{App, AsyncApp};

/// The paired person tapped the button carrying `data` on connection `id`'s
/// page, and the tap is claimed. Select or stop the pane it names, as `/use`
/// and `/stop` would, and show what that came to on the page itself: the
/// conversation is not where the person is looking.
pub(crate) async fn tapped(id: &str, user_id: &str, team_id: &str, data: &str, cx: &mut AsyncApp) {
    let Some((scope, acted)) = cx.update(|cx| act(id, user_id, team_id, data, cx)) else {
        return;
    };
    let text = match acted {
        Acted::Said(text) => text,
        Acted::Stopping(effect) => said(effect.settled().await),
    };
    cx.update(|cx| {
        // The connection may have gone, or changed identity, while the stop
        // settled; a page made under the old identity is not shown again.
        let Some((bot, home, now)) = super::serving_person(id, user_id, team_id, cx) else {
            return;
        };
        if now != scope {
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
        super::links::publish_latest(scope, home, Some(bot), cx);
    });
}

/// What a tap did at once, or the stop it started.
enum Acted {
    Said(Option<String>),
    Stopping(Effect),
}

fn act(id: &str, user_id: &str, team_id: &str, data: &str, cx: &mut App) -> Option<(Scope, Acted)> {
    if !cx.has_global::<RemoteChannels>() {
        return None;
    }
    let (_, _, scope) = super::serving_person(id, user_id, team_id, cx)?;
    let tapped = Tap::parse(data).and_then(|tap| {
        let queue = &cx.global::<RemoteChannels>().home.queue;
        Some((queue.resolve(&scope, tap)?, tap.act))
    });
    let target = dispatch::Target::Remote(id.into());
    let acted = match tapped {
        // The page is rebuilt below, so the person sees what is there now.
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
