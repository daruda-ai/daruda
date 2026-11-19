//! One text-only answer from an agent's CLI, on the adapter and account a
//! new session of that agent would get.
//!
//! GPUI-free. [`resolve`] names the run; a [`TextRunner`] performs it. The
//! app reaches the runner only through `crate::text_run_host`, so a test
//! never starts a real agent.

use std::sync::atomic::AtomicBool;

use daruda_agent::accounts::text_run::text_only_args;
use daruda_config::{AccountEnv, AgentDefinition};
use daruda_store::accounts::AccountRecipeId;

use super::adapter_cli::AdapterCliRequest;
use super::launch_resolve::AgentLaunchSpec;

/// A text-only run ready to perform: the agent's CLI words on its adapter,
/// under one account's environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextRun {
    pub(crate) cli: AdapterCliRequest,
    pub(crate) account: AccountEnv,
}

/// Why a text run gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TextRunFailure {
    /// Whoever asked stopped waiting; nothing to report.
    Cancelled,
    Failed(String),
}

pub(crate) trait TextRunner: Send + Sync {
    /// Answer `prompt`. **Blocking**; raising `cancelled` ends the run.
    fn run(
        &self,
        run: &TextRun,
        prompt: String,
        cancelled: &AtomicBool,
    ) -> Result<String, TextRunFailure>;
}

/// The auth domain and CLI request for `agent`'s text-only run, or `None`
/// when it has none: a remote launch, an adapter of no known domain, or a
/// domain whose text-only switches are not confirmed.
pub(crate) fn resolve(agent: &AgentDefinition) -> Option<(AccountRecipeId, AdapterCliRequest)> {
    let recipe = agent.launch.account_recipe(false)?;
    let words = text_only_args(recipe)?;
    let spec = AgentLaunchSpec::of(agent);
    let adapter = spec.launch.local_command()?.to_owned();
    Some((
        recipe,
        AdapterCliRequest {
            adapter,
            agent_env: spec.env,
            words: words.iter().map(|word| (*word).to_owned()).collect(),
        },
    ))
}

/// The real runner, absent from test builds: nothing there may start an
/// agent.
#[cfg(not(test))]
mod system {
    use std::sync::atomic::AtomicBool;

    use daruda_acp::preparation::PreparationCancellation;
    use daruda_agent::accounts::text_run::{TextRunError, run_text};

    use super::super::adapter_cli;
    use super::{TextRun, TextRunFailure, TextRunner};

    /// Runs the agent's real CLI. The adapter is prepared first (an install,
    /// at worst), outside the run's own deadline.
    pub(crate) struct SystemTextRunner;

    impl TextRunner for SystemTextRunner {
        fn run(
            &self,
            run: &TextRun,
            prompt: String,
            cancelled: &AtomicBool,
        ) -> Result<String, TextRunFailure> {
            let prepared = adapter_cli::prepare(&run.cli, &PreparationCancellation::default())
                .map_err(TextRunFailure::Failed)?;
            let cli = prepared
                .for_account(&run.account)
                .map_err(TextRunFailure::Failed)?;
            run_text(&cli.command, prompt, cancelled).map_err(|error| match error {
                TextRunError::Cancelled => TextRunFailure::Cancelled,
                other => TextRunFailure::Failed(other.to_string()),
            })
        }
    }
}

#[cfg(not(test))]
pub(crate) use system::SystemTextRunner;

/// Answers from a script, and records what it was asked.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct FakeTextRunner {
    state: std::sync::Arc<std::sync::Mutex<FakeState>>,
}

#[cfg(test)]
#[derive(Default)]
struct FakeState {
    answers: std::collections::VecDeque<Result<String, TextRunFailure>>,
    asked: Vec<(TextRun, String)>,
}

#[cfg(test)]
impl FakeTextRunner {
    /// Queue what the next run answers; a run with nothing queued fails.
    pub(crate) fn answer(&self, answer: Result<&str, &str>) {
        self.state.lock().unwrap().answers.push_back(
            answer
                .map(str::to_owned)
                .map_err(|reason| TextRunFailure::Failed(reason.to_owned())),
        );
    }

    /// Every run performed, with its prompt.
    pub(crate) fn asked(&self) -> Vec<(TextRun, String)> {
        self.state.lock().unwrap().asked.clone()
    }
}

#[cfg(test)]
impl TextRunner for FakeTextRunner {
    fn run(
        &self,
        run: &TextRun,
        prompt: String,
        cancelled: &AtomicBool,
    ) -> Result<String, TextRunFailure> {
        let mut state = self.state.lock().unwrap();
        state.asked.push((run.clone(), prompt));
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(TextRunFailure::Cancelled);
        }
        state
            .answers
            .pop_front()
            .unwrap_or_else(|| Err(TextRunFailure::Failed("no answer scripted".into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daruda_config::AgentLaunch;

    /// Only an agent whose CLI is known to answer without tools is run:
    /// the built-in Claude is, Codex (switches not measured) and a remote
    /// launch (its CLI is not on this machine) are not.
    #[test]
    fn only_a_local_agent_of_a_measured_domain_resolves() {
        let (recipe, request) = resolve(&AgentDefinition::claude_default()).expect("claude");
        assert_eq!(recipe, AccountRecipeId::Claude);
        assert_eq!(&request.words[..2], ["--cli", "-p"]);
        assert_eq!(resolve(&AgentDefinition::codex_default()), None);

        let mut remote = AgentDefinition::claude_default();
        let adapter_command = remote.launch.adapter_command().to_owned();
        remote.launch = AgentLaunch::Ssh {
            host: "box".into(),
            adapter_command,
        };
        assert_eq!(resolve(&remote), None);
    }
}
