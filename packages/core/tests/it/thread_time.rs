//! Public API tests for thread_time.

use std::time::{Duration, Instant};

use daruda_core::thread_time::CpuStopwatch;

#[test]
fn a_thread_that_runs_accumulates_cpu_time() {
    let watch = CpuStopwatch::start().expect("this OS reports thread CPU time");
    // Bounded by wall clock only so a broken clock fails instead of hanging.
    let give_up = Instant::now() + Duration::from_secs(30);
    let mut spins = 0u64;
    while watch.elapsed() < Duration::from_millis(40) {
        spins = std::hint::black_box(spins.wrapping_add(1));
        assert!(Instant::now() < give_up, "40 ms of CPU time never passed");
    }
}

#[test]
fn a_thread_that_sleeps_spends_no_cpu_time() {
    let watch = CpuStopwatch::start().expect("this OS reports thread CPU time");
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        watch.elapsed() < Duration::from_millis(50),
        "{:?}",
        watch.elapsed()
    );
}
