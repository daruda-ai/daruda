use super::*;

#[test]
fn reset_reserves_workers_and_rejects_same_source_from_old_generation() {
    let mut cache = ResourceImageCache::default();
    cache.set_visible(HashSet::from(["a".into(), "b".into()]));
    let old_a = cache.reserve("a".into(), 1).unwrap();
    let old_b = cache.reserve("b".into(), 2).unwrap();
    cache.clear();
    cache.set_visible(HashSet::from(["a".into(), "b".into()]));
    assert!(cache.reserve("a".into(), 1).is_none());
    assert_eq!(cache.inflight_count(), 2);
    assert!(!cache.complete(old_a, Err(ResourceImageError::Read)));
    let replacement = cache.reserve("a".into(), 1).unwrap();
    assert!(!cache.complete(old_b, Err(ResourceImageError::Read)));
    assert_eq!(cache.inflight_count(), 1);
    assert!(cache.lookup("a").is_none());
    assert!(cache.complete(replacement, Err(ResourceImageError::Decode)));
    assert!(matches!(
        cache.lookup("a"),
        Some(Err(ResourceImageError::Decode))
    ));
}

#[test]
fn consumer_release_retries_maintenance_without_counting_bitmap_arc_clones() {
    let mut cache = ResourceImageCache::default();
    let wakes = Rc::new(Cell::new(0));
    let observed = wakes.clone();
    cache.install_maintenance(move |_| observed.set(observed.get() + 1));
    let image = CachedImage::from_raster(daruda_content::visual::RasterImage {
        width: 2,
        height: 1,
        bgra: vec![0; 8],
        scale: 1.,
    })
    .unwrap();
    let _unrelated_bitmap_clone = image.clone();
    cache.cache("image".into(), Ok(image));
    let lease = cache.lookup("image").unwrap().unwrap();
    cache.clear();
    assert!(cache.take_releasable().is_empty());
    assert_eq!(cache.resident_bytes(), 8);
    drop(lease);
    assert_eq!(wakes.get(), 1);
    assert_eq!(cache.take_releasable().len(), 1);
    assert_eq!(cache.resident_bytes(), 0);
}

#[test]
fn frame_pins_retire_only_the_fifo_bytes_needed_for_next_admission() {
    let mut cache = ResourceImageCache::default();
    let image = CachedImage::from_raster(daruda_content::visual::RasterImage {
        width: 1024,
        height: 1024,
        bgra: vec![0; 4 * 1024 * 1024],
        scale: 1.,
    })
    .unwrap();
    for ix in 0..64 {
        cache.cache(format!("image-{ix}"), Ok(image.clone()));
    }
    let frame: Vec<_> = (0..64)
        .map(|ix| cache.lookup(&format!("image-{ix}")).unwrap().unwrap())
        .collect();
    cache.prepare_admission();
    assert_eq!(
        cache.entries().len(),
        48,
        "retire one 64 MiB decoder reservation, retaining newer previews"
    );
    assert_eq!(
        cache.resident_bytes(),
        256 * 1024 * 1024,
        "frame pins remain charged until released"
    );
    drop(frame);
    assert_eq!(cache.take_releasable().len(), 16);
    assert_eq!(cache.slots(), 1);
}

#[test]
fn modal_pin_on_oldest_preview_does_not_stall_reclamation_of_newer_previews() {
    let mut cache = ResourceImageCache::default();
    let image = CachedImage::from_raster(daruda_content::visual::RasterImage {
        width: 1024,
        height: 1024,
        bgra: vec![0; 4 * 1024 * 1024],
        scale: 1.,
    })
    .unwrap();
    for ix in 0..64 {
        cache.cache(format!("image-{ix}"), Ok(image.clone()));
    }
    let modal = cache.lookup("image-0").unwrap().unwrap().into_modal();
    let frame: Vec<_> = (0..64)
        .map(|ix| cache.lookup(&format!("image-{ix}")).unwrap().unwrap())
        .collect();
    cache.prepare_admission();
    assert_eq!(cache.entries().len(), 47);
    drop(frame);
    assert_eq!(cache.take_releasable().len(), 16);
    assert_eq!(
        cache.slots(),
        1,
        "oldest modal remains charged while newer FIFO candidates free a decoder slot"
    );
    drop(modal);
    assert_eq!(cache.take_releasable().len(), 1);
}
