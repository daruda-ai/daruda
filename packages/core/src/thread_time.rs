//! CPU time the calling thread has spent running. Unlike wall-clock time it
//! does not count the time the thread waits while the system runs other
//! work, so a budget measured in it gives the same answer under load.

use std::time::Duration;

/// Measures the CPU time the thread that started it spends from then on.
/// Read it on the same thread: another thread's reading measures that thread.
#[derive(Clone, Copy, Debug)]
pub struct CpuStopwatch {
    start: Duration,
}

impl CpuStopwatch {
    /// `None` where the OS does not report a thread's CPU time.
    pub fn start() -> Option<Self> {
        imp::now().map(|start| Self { start })
    }

    /// CPU time this thread has run since `start`. Windows counts it in
    /// scheduler ticks (about 15.6 ms), so short spans there read as zero.
    pub fn elapsed(&self) -> Duration {
        imp::now().map_or(Duration::ZERO, |now| now.saturating_sub(self.start))
    }
}

#[cfg(unix)]
mod imp {
    use std::time::Duration;

    pub(super) fn now() -> Option<Duration> {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `ts` is a valid, writable timespec for the call's duration.
        let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
        if rc != 0 {
            return None;
        }
        Some(Duration::new(
            u64::try_from(ts.tv_sec).ok()?,
            u32::try_from(ts.tv_nsec).ok()?,
        ))
    }
}

#[cfg(windows)]
mod imp {
    use std::time::Duration;

    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};

    pub(super) fn now() -> Option<Duration> {
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: the current-thread pseudo handle is always valid, and the
        // four FILETIMEs are writable for the call's duration.
        let ok = unsafe {
            GetThreadTimes(
                GetCurrentThread(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        };
        if ok == 0 {
            return None;
        }
        // FILETIME counts 100 ns intervals.
        let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
        Some(Duration::from_nanos(
            ticks(kernel)
                .saturating_add(ticks(user))
                .saturating_mul(100),
        ))
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    pub(super) fn now() -> Option<std::time::Duration> {
        None
    }
}
