//! Flush tests that must reach the disk.
//!
//! A binary of its own: `require_production` is process-global, and inside
//! `tests/it` it would end isolation under whichever tests run after it.
//! Every test here requires production first, so their order is irrelevant.

use daruda_core::isolation::require_production;
use daruda_core::path::sync_file_entry;

#[test]
fn an_empty_file_in_new_nested_directories_can_be_published() {
    require_production();
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("new").join("nested");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("log");
    let file = std::fs::File::create(&path).unwrap();

    sync_file_entry(&file, &path).unwrap();
    assert!(std::fs::read(&path).unwrap().is_empty());
}

#[test]
fn a_renamed_file_is_published_using_its_final_writable_handle() {
    require_production();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("temporary");
    let target = temp.path().join("snapshot");
    std::fs::write(&source, b"state").unwrap();
    std::fs::rename(&source, &target).unwrap();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&target)
        .unwrap();

    sync_file_entry(&file, &target).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"state");
}

#[cfg(windows)]
#[test]
fn publication_refuses_a_handle_without_write_access() {
    require_production();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("file");
    std::fs::write(&path, b"state").unwrap();
    let file = std::fs::File::open(&path).unwrap();

    assert!(sync_file_entry(&file, &path).is_err());
}

/// Only Unix resolves the final path; Windows flushes the handle alone.
#[cfg(unix)]
#[test]
fn a_missing_final_path_is_not_published() {
    require_production();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("entry");
    let file = std::fs::File::create(&path).unwrap();

    assert!(sync_file_entry(&file, &temp.path().join("missing")).is_err());
}
