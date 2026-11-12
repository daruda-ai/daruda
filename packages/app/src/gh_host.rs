//! The app-wide [`Gh`] backend. This global is the only way app code reaches
//! the `gh` CLI; test builds default to a fake so they never reach GitHub
//! with the developer's credentials.

use std::sync::Arc;

use daruda_github::Gh;
use gpui::{App, Global};

pub(crate) struct GhHost(Arc<dyn Gh>);

impl Global for GhHost {}

impl GhHost {
    /// The installed backend. Clone it before moving work off the
    /// foreground thread.
    pub(crate) fn global(cx: &App) -> Arc<dyn Gh> {
        cx.global::<GhHost>().0.clone()
    }

    pub(crate) fn set_global(gh: Arc<dyn Gh>, cx: &mut App) {
        cx.set_global(GhHost(gh));
    }

    /// Install the default backend unless one is already present: the
    /// system `gh`, or in test builds an empty [`FakeGh`] so a test that
    /// never asked for gh cannot reach the real CLI.
    ///
    /// [`FakeGh`]: daruda_github::fake::FakeGh
    pub(crate) fn init_if_missing(cx: &mut App) {
        if !cx.has_global::<GhHost>() {
            Self::set_global(default_backend(), cx);
        }
    }
}

#[cfg(not(test))]
fn default_backend() -> Arc<dyn Gh> {
    Arc::new(daruda_github::SystemGh)
}

#[cfg(test)]
fn default_backend() -> Arc<dyn Gh> {
    Arc::new(daruda_github::fake::FakeGh::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use daruda_github::availability::GhAuth;
    use daruda_github::fake::{FakeGh, GhOp};

    #[gpui::test]
    fn init_if_missing_keeps_an_installed_backend(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let fake = FakeGh::new();
            fake.set_auth("github.com", GhAuth::LoggedOut);
            GhHost::set_global(Arc::new(fake.clone()), cx);
            GhHost::init_if_missing(cx);
            let auth = GhHost::global(cx).auth("github.com").unwrap();
            assert_eq!(auth, GhAuth::LoggedOut, "the installed fake answers");
            assert_eq!(fake.count(GhOp::Auth), 1);
        });
    }

    #[gpui::test]
    fn init_if_missing_installs_the_fake_in_test_builds(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert!(!cx.has_global::<GhHost>());
            GhHost::init_if_missing(cx);
            // The real CLI would answer `LoggedOut` for a host nobody logged
            // in to; the empty fake answers `Ready` for any host.
            let auth = GhHost::global(cx).auth("ghe.example.invalid").unwrap();
            assert_eq!(auth, GhAuth::Ready);
        });
    }
}
