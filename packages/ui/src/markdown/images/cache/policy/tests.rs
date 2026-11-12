use gpui::{AppContext as _, TestAppContext};

use super::super::tests::{Rig, write_image};
use super::super::*;

#[gpui::test]
async fn retirement_is_oldest_first_and_same_lease_stays_deferred(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(cx);
    let paths = [dir.path().join("a.png"), dir.path().join("b.png")];
    for path in &paths {
        write_image(path, 1);
        rig.request(path, cx);
        cx.run_until_parked();
    }
    rig.cache.update(cx, |cache, _| {
        assert_eq!(cache.loaded.front(), Some(&paths[0]));
        cache.retire(&paths[0]);
        assert_eq!(cache.loaded.front(), Some(&paths[1]));
        assert_eq!(cache.bytes, 4);
        assert_eq!(cache.discarded_bytes, 4);
        assert_eq!(cache.reserved_bytes(), 8);
    });
    assert!(rig.request(&paths[0], cx).is_none());
    cx.run_until_parked();
    assert!(rig.request(&paths[0], cx).is_none());
    let new_lease = cx.new(|_| ());
    rig.window
        .update(cx, |_, window, cx| {
            rig.cache.update(cx, |cache, cx| {
                cache.request(
                    paths[0].clone(),
                    new_lease.entity_id(),
                    new_lease.entity_id(),
                    window,
                    cx,
                );
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert!(rig.request(&paths[0], cx).unwrap().is_ok());
}

#[gpui::test]
async fn queued_images_reserve_at_most_two_decoder_budgets(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(cx);
    for index in 0..10 {
        rig.request(&dir.path().join(format!("missing-{index}.png")), cx);
    }
    rig.cache.read_with(cx, |cache, _| {
        assert_eq!(cache.running, MAX_JOBS);
        assert_eq!(cache.reserved_bytes(), MAX_JOBS * JOB_BYTES);
        assert!(cache.reserved_bytes() <= CACHE_BYTES);
        assert_eq!(
            cache
                .entries
                .values()
                .filter(|entry| matches!(entry.state, State::Queued))
                .count(),
            8
        );
    });
    cx.run_until_parked();
    rig.cache.read_with(cx, |cache, _| {
        assert_eq!(cache.running, 0);
        assert_eq!(cache.reserved_bytes(), 0);
        assert!(
            cache
                .entries
                .values()
                .all(|entry| matches!(entry.state, State::Ready(Err(_))))
        );
    });
}

#[gpui::test]
async fn budget_pressure_automatically_retires_oldest_residents(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(cx);
    // Distinct cache residents share an unpainted fixture, keeping this policy
    // test's physical allocation small while exercising full byte accounting.
    let fixture = Arc::new(RenderImage::new(vec![image::Frame::new(
        image::RgbaImage::new(1024, 1024),
    )]));
    let resident_bytes = fixture.as_bytes(0).unwrap().len();
    let resident_count = CACHE_BYTES / resident_bytes;
    let paths = (0..resident_count)
        .map(|index| dir.path().join(format!("resident-{index}.png")))
        .collect::<Vec<_>>();
    rig.cache.update(cx, |cache, _| {
        for path in &paths {
            cache.entries.insert(
                path.clone(),
                Entry {
                    generation: 0,
                    version: None,
                    state: State::Ready(Ok(fixture.clone())),
                    leases: HashMap::from([(
                        rig.lease.entity_id(),
                        HashSet::from([rig.lease.entity_id()]),
                    )]),
                    workers: 0,
                    blocked: HashSet::new(),
                },
            );
            cache.loaded.push_back(path.clone());
            cache.bytes += resident_bytes;
        }
        assert_eq!(cache.reserved_bytes(), CACHE_BYTES);
    });
    let fresh = dir.path().join("fresh.png");
    write_image(&fresh, 1);
    let evicted_count = JOB_BYTES / resident_bytes;
    rig.window
        .update(cx, |_, window, cx| {
            rig.cache.update(cx, |cache, cx| {
                assert!(
                    cache
                        .request(
                            fresh.clone(),
                            rig.lease.entity_id(),
                            rig.lease.entity_id(),
                            window,
                            cx,
                        )
                        .is_none()
                );
                // Inspect the reservation before the enclosing window update drains
                // deferred atlas retirement and admits the queued decoder.
                assert_eq!(cache.loaded.front(), Some(&paths[evicted_count]));
                assert_eq!(cache.discarded_bytes, JOB_BYTES);
                assert!(cache.reserved_bytes() <= CACHE_BYTES);
                for (index, path) in paths.iter().enumerate() {
                    assert_eq!(
                        matches!(cache.entries[path].state, State::Deferred),
                        index < evicted_count,
                    );
                }
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert!(rig.request(&fresh, cx).unwrap().is_ok());
    assert!(rig.request(&paths[0], cx).is_none());
    cx.run_until_parked();
    rig.cache.read_with(cx, |cache, _| {
        assert!(matches!(cache.entries[&paths[0]].state, State::Deferred));
        assert_eq!(cache.running, 0);
        assert_eq!(cache.discarded_bytes, 0);
        assert!(cache.reserved_bytes() <= CACHE_BYTES);
    });
}
