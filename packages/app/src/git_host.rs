//! The app-wide [`Git`] backend. This global is the only way app code reaches
//! git; test builds default to a fake so they spawn no `git` processes.

use std::sync::Arc;

use gpui::{App, Global};

use crate::lane::git::Git;

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

    /// Install the default backend unless one is already present: the
    /// system `git`, or in test builds an empty [`FakeGit`] so a test that
    /// never asked for git cannot reach the real CLI.
    ///
    /// [`FakeGit`]: crate::lane::git::FakeGit
    pub(crate) fn init_if_missing(cx: &mut App) {
        if !cx.has_global::<GitHost>() {
            Self::set_global(default_backend(), cx);
        }
    }
}

#[cfg(not(test))]
fn default_backend() -> Arc<dyn Git> {
    Arc::new(crate::lane::git::SystemGit)
}

#[cfg(test)]
fn default_backend() -> Arc<dyn Git> {
    Arc::new(crate::lane::git::FakeGit::new())
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

    /// The default reaches no real repository: a fake opened on this very
    /// checkout reports no commits, where the system `git` would find one.
    #[gpui::test]
    fn init_if_missing_installs_the_fake_in_test_builds(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert!(!cx.has_global::<GitHost>());
            GitHost::init_if_missing(cx);
            let git = GitHost::global(cx);
            let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
            assert!(git.probe(here).is_none(), "no repository is scripted");
            assert!(git.open(here).head_message().is_err());
        });
    }
}
