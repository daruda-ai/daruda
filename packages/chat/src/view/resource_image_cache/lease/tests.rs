use super::super::ResourceImageCache;
use super::*;

#[test]
fn last_consumer_releases_texture_after_cache_owner_is_gone() {
    let released = Rc::new(Cell::new(0));
    let observed = released.clone();
    let mut cache = ResourceImageCache::default();
    cache.install_maintenance(move |image| {
        if image.is_some() {
            observed.set(observed.get() + 1);
        }
    });
    cache.cache(
        "orphan".into(),
        Ok(
            CachedImage::from_raster(daruda_content::visual::RasterImage {
                width: 2,
                height: 1,
                bgra: vec![0; 8],
                scale: 1.,
            })
            .unwrap(),
        ),
    );
    let lease = cache.lookup("orphan").unwrap().unwrap();
    drop(cache);
    assert_eq!(released.get(), 0);
    drop(lease);
    assert_eq!(released.get(), 1);
}
