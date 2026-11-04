//! Turn composer attachment sources into prompt content, GPUI-free so the
//! load runs on the background executor. Paths become resource links;
//! clipboard images are decoded, fitted to the send edge, and re-encoded.

use crate::surface::strings as s;
use base64::Engine as _;
use daruda_acp::{AttachmentContent, PromptAttachment};
use std::path::PathBuf;

/// Clipboard image bytes accepted before decoding; a full Retina capture fits.
pub(super) const IMAGE_SOURCE_LIMIT: usize = 32 * 1024 * 1024;
/// Decoder bounds: no side past 16384 px, about 32 megapixels of RGBA.
/// RGBA conversion and scaling allocate their own buffers beyond this.
const IMAGE_DECODE_MAX_SIDE: u32 = 16384;
const IMAGE_DECODE_MAX_ALLOC: u64 = 128 * 1024 * 1024;
/// Longer edge sent to the agent. Larger captures are scaled down rather
/// than refused; providers downscale past this anyway.
const IMAGE_SEND_MAX_EDGE: u32 = 2048;
/// Encoded image cap, within the chat's 12 MiB per-prompt base64 budget.
const IMAGE_ENCODED_LIMIT: usize = 8 * 1024 * 1024;
/// JPEG quality for an image whose lossless PNG is still over the cap.
const IMAGE_JPEG_QUALITY: u8 = 85;

/// Where a composer attachment comes from, before it is loaded.
pub(super) enum Source {
    Path(PathBuf),
    Image(Vec<u8>),
}

/// Load every source off the GPUI thread; fails whole on any refusal.
pub(super) fn load_sources(sources: Vec<Source>) -> anyhow::Result<Vec<PromptAttachment>> {
    anyhow::ensure!(
        sources.len() <= 8,
        "{}",
        daruda_chat::labels::attachment_limit()
    );
    sources.into_iter().map(load).collect()
}

fn load(source: Source) -> anyhow::Result<PromptAttachment> {
    match source {
        Source::Image(bytes) => image_attachment(
            s::agent_chat::attachment_clipboard_name().to_string(),
            &bytes,
        ),
        Source::Path(path) => {
            let uri = daruda_core::file_url::from_local_path(&path)
                .ok_or_else(|| anyhow::anyhow!("Attachment path must be absolute"))?;
            let name = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned();
            Ok(PromptAttachment {
                name,
                content: AttachmentContent::ResourceLink { uri },
            })
        }
    }
}

fn image_attachment(name: String, bytes: &[u8]) -> anyhow::Result<PromptAttachment> {
    let too_large = || anyhow::anyhow!("{}", s::agent_chat::attachment_image_too_large());
    if bytes.len() > IMAGE_SOURCE_LIMIT {
        return Err(too_large());
    }
    let image = decode_rgba(bytes).map_err(|error| match error {
        image::ImageError::Limits(_) => too_large(),
        error => error.into(),
    })?;
    let image = fit_send_edge(image);
    let (data, mime_type) = match encode_png(&image)? {
        png if png.len() <= IMAGE_ENCODED_LIMIT => (png, "image/png"),
        // Photos can stay over the cap as lossless PNG even at the send edge.
        _ => (encode_jpeg(&image)?, "image/jpeg"),
    };
    if data.len() > IMAGE_ENCODED_LIMIT {
        return Err(too_large());
    }
    Ok(PromptAttachment {
        name,
        content: AttachmentContent::Image {
            data: base64::engine::general_purpose::STANDARD.encode(data),
            mime_type: mime_type.into(),
        },
    })
}

fn decode_rgba(bytes: &[u8]) -> image::ImageResult<image::RgbaImage> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(IMAGE_DECODE_MAX_SIDE);
    limits.max_image_height = Some(IMAGE_DECODE_MAX_SIDE);
    limits.max_alloc = Some(IMAGE_DECODE_MAX_ALLOC);
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits);
    Ok(reader.decode()?.into_rgba8())
}

/// Scale so the longer edge is at most [`IMAGE_SEND_MAX_EDGE`], keeping the
/// aspect ratio; smaller images pass through untouched.
fn fit_send_edge(image: image::RgbaImage) -> image::RgbaImage {
    let (width, height) = image.dimensions();
    let long = width.max(height);
    if long <= IMAGE_SEND_MAX_EDGE {
        return image;
    }
    let scale = |side: u32| {
        let scaled = u64::from(side) * u64::from(IMAGE_SEND_MAX_EDGE) / u64::from(long);
        u32::try_from(scaled).unwrap_or(IMAGE_SEND_MAX_EDGE).max(1)
    };
    image::imageops::resize(
        &image,
        scale(width),
        scale(height),
        image::imageops::FilterType::Triangle,
    )
}

fn encode_png(image: &image::RgbaImage) -> image::ImageResult<Vec<u8>> {
    use image::ImageEncoder as _;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(png)
}

/// JPEG has no alpha; transparent areas flatten onto white, not black.
fn encode_jpeg(image: &image::RgbaImage) -> image::ImageResult<Vec<u8>> {
    use image::ImageEncoder as _;
    let rgb: Vec<u8> = image
        .pixels()
        .flat_map(|pixel| {
            let [r, g, b, a] = pixel.0;
            let a = u16::from(a);
            let over_white = |c: u8| ((u16::from(c) * a + 255 * (255 - a)) / 255) as u8;
            [over_white(r), over_white(g), over_white(b)]
        })
        .collect();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, IMAGE_JPEG_QUALITY).write_image(
        &rgb,
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_paths_are_references_without_reading_file_contents() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("folder with spaces");
        std::fs::create_dir(&folder).unwrap();
        let file = dir.path().join("binary.dat");
        std::fs::File::create(&file)
            .unwrap()
            .set_len((IMAGE_SOURCE_LIMIT + 1) as u64)
            .unwrap();
        let missing = dir.path().join("missing.txt");
        for path in [folder, file, missing, dir.path().to_path_buf()] {
            let attachment = load(Source::Path(path.clone())).unwrap();
            assert_eq!(
                attachment.content,
                AttachmentContent::ResourceLink {
                    uri: daruda_core::file_url::from_local_path(&path).unwrap(),
                }
            );
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        use image::ImageEncoder as _;
        let pixels = vec![0x80; (width * height * 4) as usize];
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
            .unwrap();
        png
    }

    fn sent_dimensions(attachment: &PromptAttachment) -> (u32, u32) {
        let AttachmentContent::Image { data, .. } = &attachment.content else {
            panic!("expected an image attachment");
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap();
        (image.width(), image.height())
    }

    #[test]
    fn a_capture_wider_than_the_send_edge_is_scaled_down_not_refused() {
        let attachment = image_attachment("shot".into(), &png(5000, 100)).unwrap();
        assert_eq!(sent_dimensions(&attachment), (2048, 40));
        let attachment = image_attachment("tall".into(), &png(10, 6000)).unwrap();
        assert_eq!(sent_dimensions(&attachment), (3, 2048));
    }

    #[test]
    fn a_noisy_image_over_the_png_cap_is_sent_as_jpeg() {
        let image = image::RgbaImage::from_fn(2048, 2048, |x, y| {
            let n = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)).to_le_bytes();
            image::Rgba([n[0], n[1], n[2], 255])
        });
        assert!(encode_png(&image).unwrap().len() > IMAGE_ENCODED_LIMIT);
        let attachment = image_attachment("photo".into(), &encode_png(&image).unwrap()).unwrap();
        let AttachmentContent::Image { mime_type, .. } = &attachment.content else {
            panic!("expected an image attachment");
        };
        assert_eq!(mime_type, "image/jpeg");
        assert_eq!(sent_dimensions(&attachment), (2048, 2048));
    }

    #[test]
    fn an_image_within_the_send_edge_keeps_its_size() {
        let attachment = image_attachment("small".into(), &png(640, 480)).unwrap();
        assert_eq!(sent_dimensions(&attachment), (640, 480));
    }

    #[test]
    fn an_image_past_the_decode_bounds_reports_it_is_too_large() {
        let error =
            image_attachment("huge".into(), &png(IMAGE_DECODE_MAX_SIDE + 1, 1)).unwrap_err();
        assert_eq!(
            error.to_string(),
            s::agent_chat::attachment_image_too_large()
        );
    }
}
