//! The app-wide [`Git`] backend. This global is the only way app code reaches
//! git; tests replace it with a fake so they spawn no `git` processes.

use std::sync::Arc;

use gpui::{App, Global};

use crate::lane::git::{Git, SystemGit};

pub(crate) struct GitHost(Arc<dyn Git>);

impl Global for GitHost {}

impl GitHost {
    /// The installed backend. Clone it (or a handle it opens) before moving
    /// work off the foreground thread.
    pub(crate) fn global(cx: &App) -> Arc<dyn Git> {
        cx.global::<GitHost>().0.clone()
    }

    pub(crate) fn set_global(git: Arc<dyn Git>, cx: &mut App) {
        cx.set_global(GitHost(git));
    }

    /// Install the system `git` unless a backend is already present.
    pub(crate) fn init_if_missing(cx: &mut App) {
        if !cx.has_global::<GitHost>() {
            Self::set_global(Arc::new(SystemGit), cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lane::git::FakeGit;

    #[gpui::test]
    fn init_if_missing_keeps_an_installed_backend(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let fake = FakeGit::new();
            fake.set_available(false);
            GitHost::set_global(Arc::new(fake), cx);
            GitHost::init_if_missing(cx);
            assert!(!GitHost::global(cx).available(), "the fake stays installed");
        });
    }

    #[gpui::test]
    fn init_if_missing_installs_a_backend_when_absent(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert!(!cx.has_global::<GitHost>());
            GitHost::init_if_missing(cx);
            assert!(cx.has_global::<GitHost>());
        });
    }
}
