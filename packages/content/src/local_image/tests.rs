use super::*;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};

#[test]
fn loads_supported_formats_outside_the_working_directory() {
    let directory = tempfile::tempdir().unwrap();
    for format in [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Gif,
        ImageFormat::WebP,
        ImageFormat::Bmp,
    ] {
        let path = directory.path().join(format!("image.{format:?}"));
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(2, 3, Rgb([255, 0, 0])));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image.write_to(&mut encoded, format).unwrap();
        std::fs::write(&path, encoded.into_inner()).unwrap();
        let raster = load(&path).unwrap_or_else(|error| panic!("{format:?}: {error}"));
        assert_eq!((raster.width, raster.height), (2, 3), "{format:?}");
        assert_eq!(raster.bgra.len(), 24, "{format:?}");
        assert_eq!(raster.scale, 1.0, "{format:?}");
    }
}

#[test]
fn rejects_encoded_size_and_dimensions_before_rendering() {
    let directory = tempfile::tempdir().unwrap();
    let oversized = directory.path().join("oversized.png");
    std::fs::File::create(&oversized)
        .unwrap()
        .set_len(MAX_BYTES + 1)
        .unwrap();
    assert_eq!(load(&oversized).err(), Some(LocalImageError::TooLarge));
    for (width, height) in [(MAX_DIMENSION + 1, 1), (1, MAX_DIMENSION + 1)] {
        let path = directory.path().join(format!("{width}x{height}.png"));
        RgbImage::new(width, height).save(&path).unwrap();
        assert_eq!(load(&path).err(), Some(LocalImageError::DecodeLimit));
    }
}

#[test]
fn reports_directory_missing_and_corrupt_files() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(load(directory.path()).err(), Some(LocalImageError::NotFile));
    assert_eq!(
        load(&directory.path().join("missing.png")).err(),
        Some(LocalImageError::Read)
    );
    let corrupt = directory.path().join("corrupt.png");
    std::fs::write(&corrupt, b"not an image").unwrap();
    assert_eq!(load(&corrupt).err(), Some(LocalImageError::Decode));
}

#[test]
fn animated_images_decode_only_the_first_raster_frame() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("animated.gif");
    let mut encoder = image::codecs::gif::GifEncoder::new(std::fs::File::create(&path).unwrap());
    for pixel in [[255, 0, 0, 255], [0, 0, 255, 255]] {
        encoder
            .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                1,
                1,
                image::Rgba(pixel),
            )))
            .unwrap();
    }
    drop(encoder);
    let raster = load(&path).unwrap();
    assert_eq!((raster.width, raster.height), (1, 1));
    assert_eq!(raster.bgra, [0, 0, 255, 255]);
}
