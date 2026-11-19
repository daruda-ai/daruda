//! The app-wide [`TextRunner`]. This global is the only way app code runs
//! an agent's CLI for text; test builds default to a fake that answers
//! nothing, so a test that never scripted an answer cannot start an agent.

use std::sync::Arc;

use gpui::{App, Global};

use crate::agent::text_run::TextRunner;

pub(crate) struct TextRunHost(Arc<dyn TextRunner>);

impl Global for TextRunHost {}

impl TextRunHost {
    /// The installed runner, or the default one when none was. Clone it
    /// before moving work off the foreground thread.
    pub(crate) fn global(cx: &App) -> Arc<dyn TextRunner> {
        cx.try_global::<TextRunHost>()
            .map_or_else(default_runner, |host| host.0.clone())
    }

    #[cfg(test)]
    pub(crate) fn set_global(runner: Arc<dyn TextRunner>, cx: &mut App) {
        cx.set_global(TextRunHost(runner));
    }
}

#[cfg(not(test))]
fn default_runner() -> Arc<dyn TextRunner> {
    Arc::new(crate::agent::text_run::SystemTextRunner)
}

#[cfg(test)]
fn default_runner() -> Arc<dyn TextRunner> {
    Arc::new(crate::agent::text_run::FakeTextRunner::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::text_run::{FakeTextRunner, TextRun, TextRunFailure};
    use std::sync::atomic::AtomicBool;

    fn run() -> TextRun {
        TextRun {
            cli: crate::agent::adapter_cli::AdapterCliRequest {
                adapter: "adapter".into(),
                agent_env: Vec::new(),
                words: Vec::new(),
            },
            account: daruda_config::AccountEnv::ambient(),
        }
    }

    /// With nothing installed a test gets a runner that starts no agent.
    #[gpui::test]
    fn a_test_without_a_runner_starts_no_agent(cx: &mut gpui::TestAppContext) {
        let answer = cx
            .update(|cx| TextRunHost::global(cx).run(&run(), "p".into(), &AtomicBool::new(false)));
        assert!(
            matches!(answer, Err(TextRunFailure::Failed(_))),
            "{answer:?}"
        );
    }

    #[gpui::test]
    fn an_installed_runner_is_the_one_asked(cx: &mut gpui::TestAppContext) {
        let fake = FakeTextRunner::default();
        fake.answer(Ok("answer"));
        cx.update(|cx| TextRunHost::set_global(Arc::new(fake.clone()), cx));
        let answer = cx
            .update(|cx| TextRunHost::global(cx).run(&run(), "p".into(), &AtomicBool::new(false)));
        assert_eq!(answer.as_deref(), Ok("answer"));
        assert_eq!(fake.asked()[0].1, "p");
    }
}
