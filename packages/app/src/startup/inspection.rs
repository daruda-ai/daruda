//! Optional inspection of the real desktop startup: replay, capture, and smoke.

#[cfg(feature = "replay")]
pub(super) fn load_replay() -> Option<crate::replay::Loaded> {
    // Load before a restored pane connects: its wire tap could truncate the
    // captured conversation before replay has read it.
    crate::replay::parse_replay_arg()
        .and_then(|path| crate::replay::load(&path, crate::replay::parse_replay_agent_arg()))
}

/// Run before `globals::init_all`, which starts the PR poller: a capture
/// answers for GitHub from a scripted `gh` (PR scenarios script their own),
/// so it never reaches GitHub with the developer's login.
#[cfg_attr(not(feature = "screenshot"), allow(unused_variables))]
pub(super) fn before_globals(cx: &mut gpui::App) {
    #[cfg(feature = "screenshot")]
    if crate::screenshot::parse_screenshot_arg().is_some() {
        crate::gh_host::GhHost::set_global(
            std::sync::Arc::new(daruda_github::fake::FakeGh::new()),
            cx,
        );
    }
}

pub(super) fn after_startup(cx: &mut gpui::App) {
    #[cfg(feature = "screenshot")]
    if let Some(path) = crate::screenshot::parse_screenshot_arg() {
        if crate::screenshot::parse_terminal_widen_flag() {
            crate::screenshot::schedule_terminal_widen_capture(path, cx);
        } else {
            crate::screenshot::schedule_capture(
                path,
                crate::screenshot::parse_scenario_arg(),
                crate::screenshot::parse_themes_arg(),
                crate::screenshot::parse_size_arg(),
                cx,
            );
        }
    }
    // Smoke observes the same initialized desktop as an interactive launch.
    if crate::smoke::requested() {
        crate::smoke::schedule(cx);
    }
}
