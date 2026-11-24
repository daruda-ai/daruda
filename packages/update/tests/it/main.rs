//! Public update installation and crash recovery contracts.

mod swap;
mod swap_recovery;

/// How long a released flock may still read as held. See [`once_released`].
#[cfg(unix)]
const FLOCK_RELEASE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);
#[cfg(unix)]
const FLOCK_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// Run `step`, again while the install's lock still reads as held.
///
/// A flock lives on the open file description, and a test beside this one
/// that forks a child (the archive test runs `tar`) copies every descriptor
/// into it until the child execs. So a lock the previous step of the same
/// test just dropped can read as held for that moment. A lock never
/// released still reads as held once the grace runs out, and fails as before.
/// Gated with its one caller, a test of a Unix symlink.
#[cfg(unix)]
fn once_released<T>(
    mut step: impl FnMut() -> Result<T, daruda_update::UpdateError>,
) -> Result<T, daruda_update::UpdateError> {
    let held = std::io::Error::from(std::io::ErrorKind::WouldBlock).to_string();
    let deadline = std::time::Instant::now() + FLOCK_RELEASE_GRACE;
    loop {
        match step() {
            Err(daruda_update::UpdateError::Io(reason))
                if reason == held && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(FLOCK_RETRY_INTERVAL);
            }
            done => return done,
        }
    }
}
