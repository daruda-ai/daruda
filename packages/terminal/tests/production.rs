//! The PTY spawn of a shipping binary, in a test binary of its own: ending
//! isolation is process-global.

#[cfg(unix)]
use daruda_terminal::pty::{PtyConfig, spawn_pty};

/// `cargo test` links the test-support stub into the app binary too, so a
/// process that requires production must still get a real shell.
#[cfg(unix)]
#[test]
fn a_production_process_spawns_a_real_shell() {
    daruda_core::isolation::require_production();
    let config = PtyConfig {
        shell: "/bin/sh".into(),
        ..PtyConfig::default()
    };

    let handle = spawn_pty(&config).unwrap();
    assert!(handle.child_pid.is_some(), "a real shell has a pid");
    handle.write(b"exit\n").unwrap();
    let _ = handle
        .exit_rx
        .recv_timeout(std::time::Duration::from_secs(5));
}
