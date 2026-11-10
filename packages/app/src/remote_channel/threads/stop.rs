//! The stop button on a pane thread's root: the value it carries, and what a
//! tap on it does. The value names the thread, not the pane, so it still
//! finds the pane after a restart renumbers panes.

use super::route;
use crate::remote_channel::dispatch::{self, Effect};
use crate::remote_channel::runtime::BotSession;
use crate::surface::strings as s;
use gpui::App;

/// Distinct from every other button value: a card's is a bare
/// `Uuid::simple` or `apv:` / `qst:` / `faq:` plus one, a listing row's `lst:`.
const PREFIX: &str = "thr:stop:";

pub(crate) fn token(thread: &str) -> String {
    format!("{PREFIX}{thread}")
}

/// The thread a stop button's value names.
pub(crate) fn thread_of(data: &str) -> Option<&str> {
    data.strip_prefix(PREFIX)
        .filter(|thread| !thread.is_empty())
}

/// Answer a tap on connection `id`: stop the pane the thread speaks for, or
/// say its conversation is not open. `None` when `data` is no stop button's.
pub(crate) fn tap(id: &str, bot: &BotSession, data: &str, cx: &mut App) -> Option<Effect> {
    let thread = thread_of(data)?;
    let effect = match route::pane_of_thread(id, bot, thread, cx) {
        Some(pane) => dispatch::stop_pane(pane, &dispatch::Target::Remote(id.into()), cx),
        None => Effect::Reply(crate::remote_channel::command::RenderedReply {
            text: s::remote_channel::thread_closed(),
            keyboard: None,
        }),
    };
    Some(never_strip_root(effect))
}

/// A tap that comes to nothing must still not be answered by stripping the
/// tapped message, as a stale card's is: this message is the thread's
/// status root, which status updates own.
fn never_strip_root(effect: Effect) -> Effect {
    match effect {
        Effect::None => Effect::Feedback {
            label: s::remote_channel::stale_callback(),
            edit: dispatch::Edit::KeepButtons,
        },
        effect => effect,
    }
}

#[cfg(test)]
pub(crate) fn tap_effect_for_test(effect: Effect) -> Effect {
    never_strip_root(effect)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No other button value can be read as a stop, nor a stop as another.
    #[test]
    fn a_stop_value_names_only_its_thread() {
        let thread = uuid::Uuid::new_v4().simple().to_string();
        assert_eq!(thread_of(&token(&thread)), Some(thread.as_str()));
        let others = [
            uuid::Uuid::new_v4().simple().to_string(),
            format!("apv:{thread}"),
            format!("qst:{thread}"),
            format!("faq:{thread}"),
            "lst:7:0".to_owned(),
            "thr:stop:".to_owned(),
        ];
        for other in others {
            assert_eq!(thread_of(&other), None, "{other}");
        }
    }
}
