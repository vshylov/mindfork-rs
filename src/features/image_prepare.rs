//! Decoding, downscaling and re-encoding an image for attachment (spec §9.10).
//!
//! The counterpart of [`super::doc_extract`] for the `/image` command: it turns whatever
//! the user points at into something every provider accepts and that is cheap enough to
//! replay on every turn. Three jobs, in order of how much they matter:
//!
//! 1. **Normalize the format.** xAI accepts `jpg`/`png` only, so a webp attached without
//!    re-encoding would work on three providers out of four — a support matrix the user
//!    discovers at send time. Everything becomes png or jpeg here instead.
//! 2. **Downscale.** A phone photo is 3–12 MB and rides *every* turn of the conversation.
//!    The clouds resize server-side but bill and receive the original each time, and the
//!    payload also lands in the chat file (fork F2). 1568 px on the long edge is
//!    Anthropic's standard-resolution ceiling and sits comfortably above the ~256-token
//!    encoder budgets measured everywhere else (research §2).
//! 3. **Measure.** Width and height drive the displayed token estimate and the chip.
//!
//! An image that is *already* png/jpeg and already within the ceiling is passed through
//! **byte for byte**: re-encoding it would only add generation loss (lessons: heavy
//! recompression is what makes text in a screenshot unreadable) and cost time.

use std::io::Cursor;

use image::{ImageFormat, ImageReader};

/// The jpeg quality used when re-encoding. 85 is the usual "no visible artifacts on
/// photographic content" setting; going lower starts to matter for text in screenshots,
/// which is exactly the content this project's users attach.
const JPEG_QUALITY: u8 = 85;

/// An image ready to be attached: a normalized payload plus what the UI needs to describe
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedImage {
    /// `image/png` or `image/jpeg` — never anything else.
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
    /// The encoded payload (not base64 — the caller encodes once, on its own thread).
    pub bytes: Vec<u8>,
}

/// Why an image could not be prepared. Localized by the caller (axis B), so this layer
/// stays locale-free.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrepareError {
    /// The bytes are not an image in any format we decode (HEIC included — its decoder is
    /// patent-encumbered and deliberately absent).
    #[error("unsupported or corrupt image format")]
    Undecodable,
    /// Decoding started but failed, or the re-encode did.
    #[error("image could not be processed: {0}")]
    Failed(String),
}

/// Prepares `bytes` for attachment. `downscale_px` is the long-edge ceiling; `0` disables
/// downscaling (but not format normalization, which is what the provider matrix needs).
pub fn prepare(bytes: &[u8], downscale_px: u32) -> Result<PreparedImage, PrepareError> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| PrepareError::Failed(e.to_string()))?;
    let format = reader.format().ok_or(PrepareError::Undecodable)?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| PrepareError::Undecodable)?;

    let long_edge = width.max(height);
    let needs_downscale = downscale_px > 0 && long_edge > downscale_px;
    let already_normal = matches!(format, ImageFormat::Png | ImageFormat::Jpeg);

    // Nothing to do: hand back the original bytes rather than re-encode them. A png that
    // survives untouched keeps the pixel-exact text a screenshot was attached for.
    if already_normal && !needs_downscale {
        return Ok(PreparedImage {
            mime: mime_of(format),
            width,
            height,
            bytes: bytes.to_vec(),
        });
    }

    let decoded = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| PrepareError::Failed(e.to_string()))?
        .decode()
        .map_err(|e| PrepareError::Failed(e.to_string()))?;
    let decoded = downscale(decoded, needs_downscale.then_some(downscale_px));
    // Alpha cannot survive jpeg, and a png source is kept as png regardless: both are the
    // "graphics, not photograph" case, where lossy artifacts are the expensive kind.
    let keep_png = format == ImageFormat::Png || decoded.color().has_alpha();
    encode(&decoded, keep_png)
}

/// Prepares raw RGBA pixels for attachment — what the clipboard hands over
/// (`arboard::ImageData`: row-major RGBA8, no container format at all). Same downscale
/// ceiling as [`prepare`].
///
/// **Always encoded as png**, unlike [`prepare`], which picks jpeg for photographic
/// sources. The dominant clipboard image in a developer's terminal is a screenshot, and
/// jpeg artifacts on small text are exactly the expensive failure — while the size that
/// would justify jpeg is already bounded by the downscale.
pub fn prepare_rgba(
    width: u32,
    height: u32,
    rgba: &[u8],
    downscale_px: u32,
) -> Result<PreparedImage, PrepareError> {
    // A clipboard that reports a size its buffer cannot back is not something to guess
    // about: a wrong stride would render as diagonal garbage rather than fail.
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4));
    if width == 0 || height == 0 || expected != Some(rgba.len()) {
        return Err(PrepareError::Undecodable);
    }
    let buf = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or(PrepareError::Undecodable)?;
    let decoded = image::DynamicImage::ImageRgba8(buf);
    let needs = downscale_px > 0 && width.max(height) > downscale_px;
    let decoded = downscale(decoded, needs.then_some(downscale_px));
    encode(&decoded, true)
}

/// Resizes to a long-edge ceiling, or returns the image untouched when `to` is `None`.
fn downscale(decoded: image::DynamicImage, to: Option<u32>) -> image::DynamicImage {
    match to {
        // Lanczos3 over the cheaper filters: downscaling by 2–8x is where a nearest or
        // triangle filter visibly destroys small text.
        Some(px) => decoded.resize(px, px, image::imageops::FilterType::Lanczos3),
        None => decoded,
    }
}

/// Encodes to png or jpeg and reports the matching MIME type.
fn encode(decoded: &image::DynamicImage, keep_png: bool) -> Result<PreparedImage, PrepareError> {
    let mut out = Vec::new();
    if keep_png {
        decoded
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .map_err(|e| PrepareError::Failed(e.to_string()))?;
    } else {
        // The jpeg encoder needs an alpha-free buffer, which `keep_png` already
        // guarantees; `to_rgb8` also normalizes a 16-bit or grayscale source.
        let rgb = decoded.to_rgb8();
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
        encoder
            .encode_image(&rgb)
            .map_err(|e| PrepareError::Failed(e.to_string()))?;
    }
    Ok(PreparedImage {
        mime: if keep_png { "image/png" } else { "image/jpeg" },
        width: decoded.width(),
        height: decoded.height(),
        bytes: out,
    })
}

fn mime_of(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "image/jpeg",
        _ => "image/png",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, Rgb, Rgba};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let buf = ImageBuffer::from_fn(width, height, |x, _| Rgb([(x % 256) as u8, 10, 20]));
        let mut out = Vec::new();
        DynamicImage::ImageRgb8(buf)
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    fn png_rgba(width: u32, height: u32) -> Vec<u8> {
        let buf = ImageBuffer::from_fn(width, height, |_, _| Rgba([1, 2, 3, 128]));
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(buf)
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    fn bmp_rgb(width: u32, height: u32) -> Vec<u8> {
        let buf = ImageBuffer::from_fn(width, height, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, 7])
        });
        let mut out = Vec::new();
        DynamicImage::ImageRgb8(buf)
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Bmp)
            .unwrap();
        out
    }

    #[test]
    fn a_small_png_is_passed_through_byte_for_byte() {
        let source = png(40, 30);
        let prepared = prepare(&source, 1568).unwrap();
        assert_eq!(prepared.mime, "image/png");
        assert_eq!((prepared.width, prepared.height), (40, 30));
        // The point of the fast path: not merely "still a png", but the same bytes.
        assert_eq!(prepared.bytes, source);
    }

    #[test]
    fn an_oversized_image_is_downscaled_to_the_long_edge() {
        let prepared = prepare(&png(3000, 1500), 1568).unwrap();
        assert_eq!(prepared.width, 1568);
        assert_eq!(prepared.height, 784, "aspect ratio must be preserved");
        assert_ne!(prepared.bytes, png(3000, 1500));
    }

    #[test]
    fn a_tall_image_is_measured_by_its_long_edge_too() {
        let prepared = prepare(&png(400, 3200), 1568).unwrap();
        assert_eq!((prepared.width, prepared.height), (196, 1568));
    }

    #[test]
    fn downscaling_can_be_switched_off_but_normalization_cannot() {
        // 0 = keep the original size...
        let prepared = prepare(&png(2000, 1000), 0).unwrap();
        assert_eq!((prepared.width, prepared.height), (2000, 1000));
        // ...while a foreign format is still re-encoded, because xAI takes png/jpeg only.
        let prepared = prepare(&bmp_rgb(20, 10), 0).unwrap();
        assert_eq!(prepared.mime, "image/jpeg");
        assert_eq!((prepared.width, prepared.height), (20, 10));
    }

    #[test]
    fn transparency_forces_png_even_for_a_foreign_format() {
        // An RGBA png stays png (both rules point the same way)...
        let prepared = prepare(&png_rgba(2000, 100), 1568).unwrap();
        assert_eq!(prepared.mime, "image/png");
        assert_eq!(prepared.width, 1568);
        // ...and the produced bytes really are a png, not a mislabelled jpeg.
        assert_eq!(
            ImageReader::new(Cursor::new(&prepared.bytes))
                .with_guessed_format()
                .unwrap()
                .format(),
            Some(ImageFormat::Png)
        );
    }

    #[test]
    fn a_photographic_source_becomes_jpeg_and_gets_smaller() {
        let source = bmp_rgb(1200, 800);
        let prepared = prepare(&source, 1568).unwrap();
        assert_eq!(prepared.mime, "image/jpeg");
        assert!(
            prepared.bytes.len() < source.len(),
            "jpeg of a photographic bitmap should be smaller than the raw bmp: {} vs {}",
            prepared.bytes.len(),
            source.len()
        );
        assert_eq!(
            ImageReader::new(Cursor::new(&prepared.bytes))
                .with_guessed_format()
                .unwrap()
                .format(),
            Some(ImageFormat::Jpeg)
        );
    }

    /// Raw RGBA is what the clipboard hands over — no container, so the only thing
    /// standing between us and diagonal garbage is the size check.
    #[test]
    fn raw_rgba_from_the_clipboard_becomes_a_png() {
        let rgba: Vec<u8> = (0..40 * 30).flat_map(|i| [i as u8, 20, 30, 255]).collect();
        let prepared = prepare_rgba(40, 30, &rgba, 1568).unwrap();
        assert_eq!(prepared.mime, "image/png");
        assert_eq!((prepared.width, prepared.height), (40, 30));
        assert_eq!(
            ImageReader::new(Cursor::new(&prepared.bytes))
                .with_guessed_format()
                .unwrap()
                .format(),
            Some(ImageFormat::Png)
        );
    }

    /// A screenshot stays png even when it is fully opaque, where [`prepare`] would pick
    /// jpeg for the same pixels: small text is the content people paste, and jpeg
    /// artifacts on it are the expensive failure.
    #[test]
    fn an_opaque_clipboard_image_is_still_png_not_jpeg() {
        let rgba: Vec<u8> = (0..64 * 64)
            .flat_map(|i| [(i % 256) as u8, ((i / 3) % 256) as u8, 90, 255])
            .collect();
        assert_eq!(prepare_rgba(64, 64, &rgba, 1568).unwrap().mime, "image/png");
    }

    #[test]
    fn a_large_clipboard_image_is_downscaled_like_any_other() {
        let rgba = vec![128u8; 2400 * 1200 * 4];
        let prepared = prepare_rgba(2400, 1200, &rgba, 1568).unwrap();
        assert_eq!((prepared.width, prepared.height), (1568, 784));
    }

    #[test]
    fn rgba_with_a_buffer_that_does_not_match_its_size_is_refused() {
        // One pixel short, one pixel long, and the degenerate sizes: all refused rather
        // than reinterpreted at a guessed stride.
        assert_eq!(
            prepare_rgba(4, 4, &[0u8; 4 * 4 * 4 - 4], 1568),
            Err(PrepareError::Undecodable)
        );
        assert_eq!(
            prepare_rgba(4, 4, &[0u8; 4 * 4 * 4 + 4], 1568),
            Err(PrepareError::Undecodable)
        );
        assert_eq!(
            prepare_rgba(0, 4, &[], 1568),
            Err(PrepareError::Undecodable)
        );
        assert_eq!(
            prepare_rgba(4, 0, &[], 1568),
            Err(PrepareError::Undecodable)
        );
        // And a size whose byte count would overflow is a refusal, not a panic.
        assert_eq!(
            prepare_rgba(u32::MAX, u32::MAX, &[0u8; 4], 1568),
            Err(PrepareError::Undecodable)
        );
    }

    #[test]
    fn non_image_bytes_are_refused_as_undecodable() {
        assert_eq!(
            prepare(b"not an image at all", 1568),
            Err(PrepareError::Undecodable)
        );
        // A truncated header is the likelier real-world shape, and must not panic.
        assert!(prepare(&png(40, 30)[..8], 1568).is_err());
    }

    #[test]
    fn an_empty_input_is_refused_rather_than_producing_a_zero_sized_image() {
        assert!(prepare(&[], 1568).is_err());
    }
}
