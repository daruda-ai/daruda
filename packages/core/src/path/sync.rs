//! Durable publication of a file under its final name.

use std::fs::File;
use std::io;
use std::path::Path;

/// Persist `file` and its final `path` after a create or rename. The caller
/// holds a writable handle to that file and prevents concurrent path changes.
/// A failure does not establish publication, even if the path already exists.
///
/// Unix flushes the file, its containing directory, and ancestors so newly
/// created directory entries are durable too. Windows flushes the writable
/// file handle after the name change: `FlushFileBuffers` persists file-system
/// metadata, including an empty file's creation; no directory handle is used.
/// See [Microsoft's file caching contract][metadata] and [CreateFile][creation].
///
/// [metadata]: https://learn.microsoft.com/en-us/windows/win32/fileio/file-caching
/// [creation]: https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew#caching-behavior
#[cfg(unix)]
pub fn sync_file_entry(file: &File, path: &Path) -> io::Result<()> {
    let path = super::canonicalize(path)?;
    let mut flushed = Flushed::default();
    flushed.push(file.try_clone()?)?;
    for directory in path.parent().into_iter().flat_map(Path::ancestors) {
        flushed.push(File::open(directory)?)?;
    }
    flushed.drain()
}

#[cfg(windows)]
pub fn sync_file_entry(file: &File, _: &Path) -> io::Result<()> {
    file.sync_all()
}

#[cfg(not(any(unix, windows)))]
pub fn sync_file_entry(_: &File, _: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "durable file publication is unsupported on this platform",
    ))
}

/// Handles already flushed to their device, in flush order, awaiting the
/// barrier that makes them durable.
#[cfg(unix)]
#[derive(Default)]
struct Flushed {
    handles: Vec<File>,
    devices: Vec<u64>,
}

#[cfg(unix)]
impl Flushed {
    fn push(&mut self, handle: File) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt as _;
        let device = handle.metadata()?.dev();
        flush_to_device(&handle)?;
        self.devices.push(device);
        self.handles.push(handle);
        Ok(())
    }

    fn drain(self) -> io::Result<()> {
        for position in barrier_positions(&self.devices) {
            drain_device(&self.handles[position])?;
        }
        Ok(())
    }
}

/// Positions, in flush order, of the last flush on each device. One device
/// barrier there persists every earlier flush on that device.
#[cfg(any(unix, test))]
fn barrier_positions(devices: &[u64]) -> Vec<usize> {
    devices
        .iter()
        .enumerate()
        .filter(|&(position, device)| !devices[position + 1..].contains(device))
        .map(|(position, _)| position)
        .collect()
}

/// macOS `fsync` only reaches the drive's volatile cache; `F_FULLFSYNC`
/// drains that cache and, per fcntl(2), persists everything fsync'd on the
/// device before it. So each handle gets the cheap flush and each device one
/// drain, instead of a drain per handle (~3 ms each).
#[cfg(target_os = "macos")]
fn flush_to_device(handle: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd as _;
    // SAFETY: the descriptor is owned by `handle`, which outlives the call.
    if unsafe { libc::fsync(handle.as_raw_fd()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn drain_device(handle: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd as _;
    // SAFETY: the descriptor is owned by `handle`, which outlives the call;
    // `F_FULLFSYNC` ignores its argument.
    if unsafe { libc::fcntl(handle.as_raw_fd(), libc::F_FULLFSYNC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Elsewhere `fsync` already reaches stable storage, so it needs no drain.
#[cfg(all(unix, not(target_os = "macos")))]
fn flush_to_device(handle: &File) -> io::Result<()> {
    handle.sync_all()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn drain_device(_: &File) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_device_is_drained_once_after_its_last_flush() {
        let cases: [(&[u64], &[usize], &str); 4] = [
            (&[], &[], "nothing flushed needs no barrier"),
            (&[7], &[0], "a lone file is drained itself"),
            (
                &[7, 7, 7],
                &[2],
                "one device: only its last flush drains it",
            ),
            (
                &[7, 7, 3, 7, 3],
                &[3, 4],
                "two devices: each drained after its own last flush",
            ),
        ];
        for (devices, expected, label) in cases {
            assert_eq!(barrier_positions(devices), expected, "{label}");
        }
    }

    #[test]
    fn a_file_in_freshly_created_directories_is_published() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("a").join("b");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("entry");
        let file = File::create(&path).unwrap();

        sync_file_entry(&file, &path).unwrap();
        assert!(sync_file_entry(&file, &directory.join("missing")).is_err());
    }
}
