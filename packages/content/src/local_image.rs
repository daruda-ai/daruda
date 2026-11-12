//! Bounded local raster decoding shared by Markdown and tool previews.

use std::io::Read as _;
use std::path::Path;

use crate::visual::{self, RasterImage};

/// Maximum encoded file size, enforced again while reading a changing file.
pub const MAX_BYTES: u64 = 8 * 1024 * 1024;
/// Maximum width or height of a decoded raster.
pub const MAX_DIMENSION: u32 = 4096;
/// Decoder allocation budget, independent of the encoded size limit.
pub const MAX_ALLOC_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalImageError {
    Read,
    NotFile,
    TooLarge,
    DecodeLimit,
    Decode,
}

impl std::fmt::Display for LocalImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Read => "unable to read image file",
            Self::NotFile => "image path is not a regular file",
            Self::TooLarge => "image exceeds the encoded size limit",
            Self::DecodeLimit => "image exceeds the dimension or memory limit",
            Self::Decode => "unsupported or invalid image data",
        })
    }
}

impl std::error::Error for LocalImageError {}

/// Read and decode one raster frame. Call only from a background executor:
/// this function performs blocking file I/O and CPU-bound decoding.
pub fn load(path: &Path) -> Result<RasterImage, LocalImageError> {
    // Check before opening too: Windows refuses directory handles, and special
    // files can block on open even though they are never eligible images.
    if !std::fs::metadata(path)
        .map_err(|_| LocalImageError::Read)?
        .is_file()
    {
        return Err(LocalImageError::NotFile);
    }
    let file = std::fs::File::open(path).map_err(|_| LocalImageError::Read)?;
    let metadata = file.metadata().map_err(|_| LocalImageError::Read)?;
    if !metadata.is_file() {
        return Err(LocalImageError::NotFile);
    }
    if metadata.len() > MAX_BYTES {
        return Err(LocalImageError::TooLarge);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| LocalImageError::Read)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(LocalImageError::TooLarge);
    }

    // Third-party decoders process untrusted bytes; a malformed image must
    // become a cached failure rather than panic its host's executor.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        visual::decode_image_bounded(&bytes, MAX_DIMENSION, MAX_ALLOC_BYTES).map_err(|error| {
            match error.downcast_ref::<image::ImageError>() {
                Some(image::ImageError::Limits(_)) => LocalImageError::DecodeLimit,
                _ => LocalImageError::Decode,
            }
        })
    }))
    .unwrap_or(Err(LocalImageError::Decode))
}

#[cfg(test)]
mod tests;
