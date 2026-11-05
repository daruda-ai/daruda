//! The app's one quit hook: what must end, and in what order, within a
//! single deadline before the process exits.

use gpui::App;

use crate::runs::{QUIT_FLUSH_DEADLINE, RunsHost};

/// Signal flows first and start the phone card edits so both proceed while
/// journals flush; then wait on the cards and the flows. GPUI gives returned
/// quit futures little time, so every phase blocks here instead.
pub(crate) fn install(cx: &mut App) {
    cx.on_app_quit(|cx| {
        run(std::time::Instant::now() + QUIT_FLUSH_DEADLINE, cx);
        async {}
    })
    .detach();
}

fn run(until: std::time::Instant, cx: &mut App) {
    RunsHost::quit_begin(cx);
    let cards = crate::remote_channel::cards::quit::begin(cx);
    RunsHost::quit_flush(until, cx);
    cards.wait(until);
    RunsHost::quit_wait(until, cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_channel::cards::tests::EditCapture;
    use crate::remote_channel::global::Delivery;

    /// The installed hook is what settles phone cards on quit.
    #[gpui::test]
    fn quitting_the_app_cancels_live_phone_cards(cx: &mut gpui::TestAppContext) {
        let tasks = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            RunsHost::install(
                RunsHost::open(daruda_store::tasks::TaskStore::in_directory(
                    tasks.path(),
                    None,
                    "test",
                    crate::runs::no_hook(),
                )),
                cx,
            );
            let mut telegram = crate::telegram::global::install_for_test(true, Some(42), cx);
            cx.set_global(EditCapture::default());
            let (_, _answer) =
                crate::control::approval::request("Approve this".into(), Delivery::Explicit, cx);
            telegram.begin_send(10, cx).unwrap()(cx);
            install(cx);
        });
        cx.quit();
        cx.read(|cx| {
            let edits = &cx.global::<EditCapture>().edits;
            assert_eq!(edits.len(), 1);
            assert_eq!(
                edits[0].label,
                crate::surface::strings::notification::card_cancelled_at_quit()
            );
        });
    }
}
