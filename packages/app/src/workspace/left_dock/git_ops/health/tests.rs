use super::*;

#[test]
fn git_read_health_cooldown_reports_once_and_recovers() {
    let now = Instant::now();
    let mut health = GitReadHealth::default();
    let first = health.admit(ReadOrigin::Automatic, now).unwrap();
    let stale = health.admit(ReadOrigin::Automatic, now).unwrap();
    let failure = ReadOutcome::Environment(GitEnvironmentFailure::ExecutableMissing);
    assert_eq!(health.finish(first, failure, now), HealthEffect::Report);
    assert_eq!(health.finish(stale, failure, now), HealthEffect::Stale);
    assert!(
        health
            .admit(ReadOrigin::Automatic, now + Duration::from_secs(29))
            .is_none()
    );
    assert_eq!(health.retry_at(), Some(now + COOLDOWN));
    let probe = health.admit(ReadOrigin::Automatic, now + COOLDOWN).unwrap();
    assert!(health.admit(ReadOrigin::Manual, now + COOLDOWN).is_none());
    assert_eq!(
        health.finish(probe, failure, now + COOLDOWN),
        HealthEffect::Quiet
    );
    let manual = health.admit(ReadOrigin::Manual, now + COOLDOWN).unwrap();
    assert_eq!(
        health.finish(manual, ReadOutcome::Success, now + COOLDOWN),
        HealthEffect::Recovered
    );
    assert_eq!(health.retry_at(), None);
    assert_eq!(
        health.finish(probe, failure, now + COOLDOWN),
        HealthEffect::Stale
    );
    let next = health.admit(ReadOrigin::Automatic, now + COOLDOWN).unwrap();
    assert_eq!(
        health.finish(next, failure, now + COOLDOWN),
        HealthEffect::Report
    );
}

#[test]
fn git_read_health_changed_cause_and_abandonment() {
    let now = Instant::now();
    let mut health = GitReadHealth::default();
    let permit = health.admit(ReadOrigin::Automatic, now).unwrap();
    health.finish(
        permit,
        ReadOutcome::Environment(GitEnvironmentFailure::ExecutableMissing),
        now,
    );
    let probe = health.admit(ReadOrigin::Manual, now).unwrap();
    health.abandon(probe, now);
    assert_eq!(health.retry_at(), Some(now + COOLDOWN));
    let probe = health.admit(ReadOrigin::Manual, now).unwrap();
    assert_eq!(
        health.finish(
            probe,
            ReadOutcome::Environment(GitEnvironmentFailure::XcodeLicenseNotAccepted),
            now
        ),
        HealthEffect::Report
    );
    let probe = health.admit(ReadOrigin::Manual, now).unwrap();
    assert_eq!(
        health.finish(probe, ReadOutcome::OtherFailure, now),
        HealthEffect::Cleared
    );
    assert!(!health.is_blocked());
}
