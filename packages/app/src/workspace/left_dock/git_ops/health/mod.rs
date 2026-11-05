//! Admission and notification episodes for automatic Git reads.
use crate::lane::git::{GitEnvironmentFailure, GitError};
use std::time::{Duration, Instant};

pub(super) const COOLDOWN: Duration = Duration::from_secs(30);
#[derive(Clone, Copy)]
pub(super) enum ReadOrigin {
    Automatic,
    Manual,
}
#[derive(Clone, Copy)]
pub(super) struct ReadPermit {
    epoch: u64,
}
#[derive(Clone, Copy)]
pub(super) enum ReadOutcome {
    Success,
    Environment(GitEnvironmentFailure),
    OtherFailure,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum HealthEffect {
    Quiet,
    Stale,
    Report,
    Recovered,
    Cleared,
}
#[derive(Default)]
enum State {
    #[default]
    Ready,
    Cooling {
        cause: GitEnvironmentFailure,
        until: Instant,
    },
    Probing {
        cause: GitEnvironmentFailure,
    },
}
#[derive(Default)]
pub(super) struct GitReadHealth {
    epoch: u64,
    state: State,
}
impl GitReadHealth {
    pub fn is_blocked(&self) -> bool {
        !matches!(self.state, State::Ready)
    }
    pub fn retry_at(&self) -> Option<Instant> {
        match self.state {
            State::Cooling { until, .. } => Some(until),
            _ => None,
        }
    }
    pub fn admit(&mut self, origin: ReadOrigin, now: Instant) -> Option<ReadPermit> {
        match self.state {
            State::Ready => {}
            State::Cooling { cause, until }
                if now >= until || matches!(origin, ReadOrigin::Manual) =>
            {
                self.epoch += 1;
                self.state = State::Probing { cause };
            }
            _ => return None,
        }
        Some(ReadPermit { epoch: self.epoch })
    }
    pub fn abandon(&mut self, permit: ReadPermit, now: Instant) {
        if permit.epoch == self.epoch
            && let State::Probing { cause } = self.state
        {
            self.epoch += 1;
            self.state = State::Cooling {
                cause,
                until: now + COOLDOWN,
            };
        }
    }
    pub fn finish(
        &mut self,
        permit: ReadPermit,
        outcome: ReadOutcome,
        now: Instant,
    ) -> HealthEffect {
        if permit.epoch != self.epoch {
            return HealthEffect::Stale;
        }
        match outcome {
            ReadOutcome::Environment(cause) => {
                let repeated = matches!(self.state, State::Probing { cause: old } | State::Cooling { cause: old, .. } if old == cause);
                self.epoch += 1;
                self.state = State::Cooling {
                    cause,
                    until: now + COOLDOWN,
                };
                if repeated {
                    HealthEffect::Quiet
                } else {
                    HealthEffect::Report
                }
            }
            _ if self.is_blocked() => {
                self.epoch += 1;
                self.state = State::Ready;
                if matches!(outcome, ReadOutcome::Success) {
                    HealthEffect::Recovered
                } else {
                    HealthEffect::Cleared
                }
            }
            _ => HealthEffect::Quiet,
        }
    }
}

pub(super) trait GitReadResult {
    fn outcome(&self) -> ReadOutcome;
    fn git_error_message(&self) -> Option<String>;
}
impl<T> GitReadResult for Result<T, GitError> {
    fn outcome(&self) -> ReadOutcome {
        match self {
            Ok(_) => ReadOutcome::Success,
            Err(e) => e
                .environment_failure()
                .map(ReadOutcome::Environment)
                .unwrap_or(ReadOutcome::OtherFailure),
        }
    }

    fn git_error_message(&self) -> Option<String> {
        self.as_ref().err().map(ToString::to_string)
    }
}
#[cfg(test)]
mod tests;
