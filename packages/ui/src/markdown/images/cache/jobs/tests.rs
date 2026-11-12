use gpui::TestAppContext;

use super::super::tests::{Rig, write_image};
use super::super::*;
use std::time::Duration;

#[gpui::test]
async fn overwrite_and_missing_file_creation_reload_cached_results(cx: &mut TestAppContext) {
    for initially_exists in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview.png");
        if initially_exists {
            write_image(&path, 1);
        }
        let rig = Rig::new(cx);
        assert!(rig.request(&path, cx).is_none());
        cx.run_until_parked();
        let initial = rig.request(&path, cx).unwrap();
        assert_eq!(initial.is_ok(), initially_exists);
        write_image(&path, 7);
        rig.poll(cx);
        let current = rig.request(&path, cx).unwrap().unwrap();
        assert_eq!(current.as_bytes(0).unwrap().len(), 7 * 4);
        assert!(
            rig.cache
                .read_with(cx, |cache, _| cache.reserved_bytes() <= CACHE_BYTES)
        );
    }
}

#[gpui::test]
async fn owner_drop_is_not_prevented_by_workers_or_poller(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preview.png");
    write_image(&path, 1);
    let rig = Rig::new(cx);
    let weak = rig.cache.downgrade();
    rig.request(&path, cx);
    drop(rig);
    cx.run_until_parked();
    assert!(weak.upgrade().is_none());
}

#[gpui::test]
async fn invalidated_inflight_generation_cannot_publish_old_result(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preview.png");
    write_image(&path, 1);
    let rig = Rig::new(cx);
    rig.request(&path, cx);
    rig.cache.update(cx, |cache, cx| {
        assert_eq!(cache.running, 1);
        cache.invalidate(&path, Version::Read, cx);
        // An invalidated worker still owns its decoder reservation until completion.
        assert_eq!(cache.running, 1);
        assert_eq!(cache.entries[&path].generation, 1);
    });
    write_image(&path, 7);
    cx.run_until_parked();
    let result = rig.request(&path, cx).unwrap().unwrap();
    assert_eq!(result.as_bytes(0).unwrap().len(), 7 * 4);
    assert_eq!(rig.cache.read_with(cx, |cache, _| cache.running), 0);
}

#[gpui::test]
async fn same_length_overwrite_reloads_when_modification_time_changes(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preview.bmp");
    let write = |color, seconds| {
        image::RgbaImage::from_pixel(1, 1, image::Rgba(color))
            .save(&path)
            .unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
            )
            .unwrap();
    };
    write([255, 0, 0, 255], 1_000);
    let initial_version = Version::read(&path);
    let original_length = std::fs::metadata(&path).unwrap().len();
    let rig = Rig::new(cx);
    rig.request(&path, cx);
    cx.run_until_parked();
    let first = rig.request(&path, cx).unwrap().unwrap();
    assert_eq!(first.as_bytes(0).unwrap(), &[0, 0, 255, 255]);
    write([0, 0, 255, 255], 2_000);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), original_length);
    assert_ne!(Version::read(&path), initial_version);
    rig.poll(cx);
    let updated = rig.request(&path, cx).unwrap().unwrap();
    assert_eq!(updated.size(0), first.size(0));
    assert_eq!(updated.as_bytes(0).unwrap(), &[255, 0, 0, 255]);
}

#[gpui::test]
async fn deferred_images_are_not_redecoded_when_their_file_changes(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preview.png");
    write_image(&path, 1);
    let rig = Rig::new(cx);
    rig.request(&path, cx);
    cx.run_until_parked();
    rig.cache.update(cx, |cache, _| cache.retire(&path));
    write_image(&path, 7);
    rig.poll(cx);
    rig.cache.read_with(cx, |cache, _| {
        assert!(matches!(cache.entries[&path].state, State::Deferred));
        assert_eq!(cache.running, 0);
        assert_eq!(cache.bytes, 0);
    });
}
