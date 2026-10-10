//! Pure, bounded attachment normalization. No filesystem, HTTP or database access.
//!
//! Images are decoded to pixels, oriented using EXIF, and encoded without uploaded metadata.
//! JPEG uses quality 90; PNG is lossless; GIF retains only its first frame (animation
//! is dropped). Animated PNG/WebP likewise become a single still image. Thumbnails describe
//! the stored pixels, not the pre-normalization upload. PDF metadata is **not stripped**;
//! PDFs and UTF-8 text are stored unchanged. Serving code must use these content types and
//! appropriate download headers, never the uploaded filename or MIME as a security decision.
//!
//! JPEG is ALWAYS decoded and re-encoded; identical uploads are deterministic, but a
//! stored JPEG's hash can change on re-ingest (lossy generation). PNG/GIF/lossless WebP,
//! PDF and text are hash-idempotent. Lossy VP8 WebP without alpha is stored as JPEG q90;
//! alpha-bearing and VP8L inputs are stored as lossless WebP. Animated inputs become stills.
//! ICC profiles and PNG gAMA/cHRM/sRGB are dropped WITHOUT colour conversion: wide-gamut
//! uploads can look different. Serving code must use the returned type/extension.
//!
//! WebP admits VP8 (with raw or compressed alpha) and VP8L, including predictor,
//! colour-transform and colour-indexing auxiliary images and colour caches, ONLY when
//! the decoded first image/animation frame (including its compressed ALPH) has its
//! meta-Huffman bit clear. Unused animation frames get structural/dimension checks only;
//! duplicate/extra image chunks are Invalid. Auxiliary images have one group by
//! specification and are bounded/skipped. Descending two-symbol simple codes are
//! `UnsupportedType`; remaining trees mirror image-webp 0.2.4's bit consumption.
//! Main meta-Huffman is `UnsupportedType`, not a size error: the pinned decoder can
//! allocate 65,536 Huffman groups independently of pixel count. Malformed data is Invalid.
//! Compatibility is size-dependent: libwebp commonly enables main meta-Huffman for
//! realistic lossless photos, smooth-alpha images >=256x192, 512-square stickers,
//! lossless screenshots and lossless animation. These are `attachment-type-unsupported`;
//! small lossless/alpha fixtures and ordinary alpha-free lossy WebP work. This is not
//! a promise to support arbitrary lossless/alpha WebP below the byte/pixel limits.

use std::io::{self, Cursor, Write};
use std::path::PathBuf;

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageFormat, ImageReader};
use sha2::{Digest, Sha256};

/// Upload and image-resource policy. Limits are inclusive.
///
/// Configuration invariants: all fields must be nonzero. Struct-literal overrides are
/// permitted; callers must validate their configuration, not untrusted user input.
/// `ingest` debug-asserts these invariants (misconfiguration is a programmer error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_pixels: u64,
    pub max_dimension: u32,
    pub max_alloc_bytes: u64,
    pub thumb_max_edge: u32,
}

impl Limits {
    /// 25 MiB uploads, 25 Mpx, 16K edges, 512 MiB budget, 320px thumbnails.
    ///
    /// Admission uses decoder.total_bytes() times a measured factor: 2 for PNG,
    /// 4 for JPEG/WebP/GIF, plus twice the input, twice the output cap, and 1 MiB scratch.
    /// JPEG needs the larger factor for progressive coefficient buffers and grayscale→RGB.
    /// The 25 Mpx ceiling fits even 8-byte RGBA16 PNG and 4-byte RGBA WebP with maximum
    /// input/output caps: 16*25M + 4*25MiB + 1MiB < 512MiB. Tests measure peak heap for
    /// rotated RGBA16, RGBA WebP, and ordinary 12–24.5 Mpx images. This is conservative
    /// admission, not an OS allocator cap; S7 must still bound concurrency/process memory.
    pub fn production() -> Self {
        Self {
            max_bytes: 25 * 1024 * 1024,
            max_pixels: 25_000_000,
            max_dimension: 16_384,
            max_alloc_bytes: 512 * 1024 * 1024,
            thumb_max_edge: 320,
        }
    }
}

/// Typed failures mapped by the HTTP layer to stable problem slugs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IngestError {
    #[error("attachment exceeds a byte, dimension, pixel or allocation limit")]
    TooLarge,
    #[error("attachment type is not allowed")]
    UnsupportedType,
    #[error("declared type or extension contradicts the bytes")]
    TypeMismatch,
    #[error("attachment is invalid or truncated")]
    Invalid,
}

impl IngestError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::TooLarge => "attachment-too-large",
            Self::UnsupportedType => "attachment-type-unsupported",
            Self::TypeMismatch => "attachment-type-mismatch",
            Self::Invalid => "attachment-invalid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
}

/// Only normalized bytes and authoritative (sniffed) metadata leave this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingested {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub sha256: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub thumbnail: Option<Thumbnail>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Jpeg,
    Png,
    Gif,
    Webp,
    Pdf,
    Text,
}

impl Kind {
    fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
            Self::Pdf => "application/pdf",
            Self::Text => "text/plain",
        }
    }

    fn format(self) -> Option<ImageFormat> {
        match self {
            Self::Jpeg => Some(ImageFormat::Jpeg),
            Self::Png => Some(ImageFormat::Png),
            Self::Gif => Some(ImageFormat::Gif),
            Self::Webp => Some(ImageFormat::WebP),
            Self::Pdf | Self::Text => None,
        }
    }
}

/// Sniff, validate, normalize, thumbnail, then hash **stored** bytes.
///
/// Hints never choose a decoder. A filename with no extension and an absent/empty/generic
/// `application/octet-stream` MIME and unrecognized hints are neutral. Only recognized
/// family contradictions reject (case-insensitive; MIME parameters ignored). PDFs require a versioned
/// header and EOF in the final 1024 bytes; this is a structural check, not a PDF sandbox.
/// Text must be UTF-8 without NUL. PDF active content is not sanitized.
pub fn ingest(
    bytes: &[u8],
    declared_name: Option<&str>,
    declared_mime: Option<&str>,
    limits: &Limits,
) -> Result<Ingested, IngestError> {
    debug_assert!(
        limits.max_bytes > 0
            && limits.max_pixels > 0
            && limits.max_dimension > 0
            && limits.max_alloc_bytes > 0
            && limits.thumb_max_edge > 0,
        "invalid ingest Limits"
    );
    if bytes.len() > limits.max_bytes {
        return Err(IngestError::TooLarge);
    }
    let kind = sniff(bytes)?;
    check_hints(kind, declared_name, declared_mime)?;
    if let Some(format) = kind.format() {
        return ingest_image(bytes, kind, format, limits);
    }
    match kind {
        Kind::Pdf => {
            let valid_version = bytes.starts_with(b"%PDF-1.") || bytes.starts_with(b"%PDF-2.");
            let version_digit = bytes.get(7).is_some_and(u8::is_ascii_digit);
            let tail = &bytes[bytes.len().saturating_sub(1024)..];
            if !valid_version || !version_digit || !tail.windows(5).any(|w| w == b"%%EOF") {
                return Err(IngestError::Invalid);
            }
        }
        Kind::Text => {
            if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
                return Err(IngestError::Invalid);
            }
        }
        _ => unreachable!("image formats handled above"),
    }
    Ok(Ingested {
        bytes: bytes.to_vec(),
        content_type: kind.mime(),
        sha256: hash(bytes),
        width: None,
        height: None,
        thumbnail: None,
    })
}

/// `aa/bb/<sha256>` for exactly 64 lowercase hexadecimal characters; never accepts paths.
pub fn blob_relative_path(sha256_hex: &str) -> Result<PathBuf, IngestError> {
    if sha256_hex.len() != 64
        || !sha256_hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(IngestError::Invalid);
    }
    Ok(PathBuf::from(&sha256_hex[..2])
        .join(&sha256_hex[2..4])
        .join(sha256_hex))
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn sniff(bytes: &[u8]) -> Result<Kind, IngestError> {
    // Explicit magic catches invalid/truncated headers too (infer requires longer prefixes).
    if bytes.starts_with(b"%PDF-") {
        return Ok(Kind::Pdf);
    }
    if bytes.starts_with(b"\xff\xd8") {
        return Ok(Kind::Jpeg);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(Kind::Png);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Ok(Kind::Gif);
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Ok(Kind::Webp);
    }
    // Signature sniffing gets a bounded prefix, never a whole container/compound file.
    if let Some(ty) = infer::get(&bytes[..bytes.len().min(4096)]) {
        return match ty.mime_type() {
            "image/jpeg" => Ok(Kind::Jpeg),
            "image/png" => Ok(Kind::Png),
            "image/gif" => Ok(Kind::Gif),
            "image/webp" => Ok(Kind::Webp),
            "application/pdf" => Ok(Kind::Pdf),
            "text/plain" => Ok(Kind::Text),
            _ => Err(IngestError::UnsupportedType),
        };
    }
    // This is a deliberately conservative signature heuristic, NOT a markup sanitizer:
    // reject common markup/executable/container prefixes. Other UTF-8 (including unknown
    // tags) stays text/plain; S7 must serve nosniff + safe Content-Disposition. "MZ" text
    // is a deliberate false positive, not evidence that the file is executable.
    let text = std::str::from_utf8(bytes).map_err(|_| IngestError::UnsupportedType)?;
    let start = text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_lowercase();
    if bytes.starts_with(b"MZ")
        || bytes.starts_with(b"PK\x03\x04")
        || start.starts_with("<svg")
        || start.starts_with("<?xml")
        || start.starts_with("<!doctype")
        || start.starts_with("<html")
    {
        return Err(IngestError::UnsupportedType);
    }
    Ok(Kind::Text)
}

fn mime_family(mime: &str) -> Option<Kind> {
    match mime {
        "image/jpeg" | "image/jpg" | "image/pjpeg" => Some(Kind::Jpeg),
        "image/png" | "image/x-png" => Some(Kind::Png),
        "image/gif" => Some(Kind::Gif),
        "image/webp" => Some(Kind::Webp),
        "application/pdf" => Some(Kind::Pdf),
        m if m.starts_with("text/") && !markup_mime(m) => Some(Kind::Text),
        _ => None,
    }
}
fn markup_mime(mime: &str) -> bool {
    matches!(
        mime,
        "text/html" | "text/xml" | "image/svg+xml" | "application/xml" | "application/xhtml+xml"
    ) || mime.ends_with("+xml")
}
fn check_hints(kind: Kind, name: Option<&str>, mime: Option<&str>) -> Result<(), IngestError> {
    if let Some(mime) = mime {
        let mime = mime
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if markup_mime(&mime) || mime_family(&mime).is_some_and(|hint| hint != kind) {
            return Err(IngestError::TypeMismatch);
        }
    }
    if let Some(name) = name {
        let basename = name.rsplit(['/', '\\']).next().unwrap_or(name);
        if let Some((_, ext)) = basename.rsplit_once('.') {
            let ext = ext.to_ascii_lowercase();
            let hint = match ext.as_str() {
                "jpg" | "jpeg" | "jpe" | "jfif" | "jif" => Some(Kind::Jpeg),
                "png" => Some(Kind::Png),
                "gif" => Some(Kind::Gif),
                "webp" => Some(Kind::Webp),
                "pdf" => Some(Kind::Pdf),
                "txt" | "text" => Some(Kind::Text),
                _ => None,
            };
            if matches!(ext.as_str(), "svg" | "html" | "htm" | "xml")
                || hint.is_some_and(|h| h != kind)
            {
                return Err(IngestError::TypeMismatch);
            }
        }
    }
    Ok(())
}

fn check_dimensions(w: u32, h: u32, limits: &Limits) -> Result<(), IngestError> {
    if w == 0 || h == 0 {
        return Err(IngestError::Invalid);
    }
    let pixels = u64::from(w) * u64::from(h);
    if w > limits.max_dimension || h > limits.max_dimension || pixels > limits.max_pixels {
        return Err(IngestError::TooLarge);
    }
    Ok(())
}

fn image_error(error: image::ImageError) -> IngestError {
    match error {
        image::ImageError::Limits(_) => IngestError::TooLarge,
        image::ImageError::IoError(e) if e.kind() == io::ErrorKind::OutOfMemory => {
            IngestError::TooLarge
        }
        _ => IngestError::Invalid,
    }
}

/// Shared decode admission estimate, also used by the full-only allocation probe.
/// Requested live heap policy, not RSS or an enforceable allocator cap.
#[doc(hidden)]
pub fn image_admission_bytes(
    format: ImageFormat,
    decoded_bytes: u64,
    input_len: usize,
    max_bytes: usize,
) -> Result<u64, IngestError> {
    let factor = if format == ImageFormat::Png { 2 } else { 4 };
    decoded_bytes
        .checked_mul(factor)
        .and_then(|v| v.checked_add((input_len as u64).checked_mul(2)?))
        .and_then(|v| v.checked_add((max_bytes as u64).checked_mul(2)?))
        .and_then(|v| v.checked_add(1024 * 1024))
        .ok_or(IngestError::TooLarge)
}

fn decode(bytes: &[u8], format: ImageFormat, limits: &Limits) -> Result<DynamicImage, IngestError> {
    let mut il = image::Limits::default();
    il.max_image_width = Some(limits.max_dimension);
    il.max_image_height = Some(limits.max_dimension);
    il.max_alloc = Some(limits.max_alloc_bytes);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(il.clone());
    let mut decoder = reader.into_decoder().map_err(image_error)?;
    let (w, h) = decoder.dimensions();
    check_dimensions(w, h, limits)?;
    let budget =
        image_admission_bytes(format, decoder.total_bytes(), bytes.len(), limits.max_bytes)?;
    if budget > limits.max_alloc_bytes {
        return Err(IngestError::TooLarge);
    }
    // into_decoder does not reserve the destination pixel buffer; do so explicitly.
    il.reserve(decoder.total_bytes()).map_err(image_error)?;
    decoder.set_limits(il).map_err(image_error)?;
    DynamicImage::from_decoder(decoder).map_err(image_error)
}

fn ingest_image(
    bytes: &[u8],
    kind: Kind,
    format: ImageFormat,
    limits: &Limits,
) -> Result<Ingested, IngestError> {
    // Preflight before constructing any decoder. Strip compressed PNG metadata before the
    // library sees it (iCCP/zTXt/iTXt could themselves be decompression bombs).
    let (prepared, orientation) = prepare(bytes, kind, limits)?;
    let mut pixels = decode(&prepared, format, limits)?;
    pixels.apply_orientation(orientation);
    let (w, h) = (pixels.width(), pixels.height());
    let stored_kind =
        if kind == Kind::Webp && !pixels.color().has_alpha() && webp_has_vp8(&prepared)? {
            Kind::Jpeg
        } else {
            kind
        };
    let stored = encode(&pixels, stored_kind, limits.max_bytes)?;
    // Release the source and stripped input before decoding the stored generation.
    drop(pixels);
    drop(prepared);
    let stored_pixels = decode(
        &stored,
        stored_kind.format().ok_or(IngestError::Invalid)?,
        limits,
    )?;
    let edge = limits.thumb_max_edge.min(w.max(h));
    let thumb = stored_pixels.thumbnail(edge, edge);
    let thumb_kind = if stored_pixels.color().has_alpha() {
        Kind::Png
    } else {
        Kind::Jpeg
    };
    let thumb_bytes = encode(&thumb, thumb_kind, limits.max_bytes)?;
    let thumbnail = Thumbnail {
        sha256: hash(&thumb_bytes),
        bytes: thumb_bytes,
        content_type: thumb_kind.mime(),
        width: thumb.width(),
        height: thumb.height(),
    };
    Ok(Ingested {
        sha256: hash(&stored),
        bytes: stored,
        content_type: stored_kind.mime(),
        width: Some(w),
        height: Some(h),
        thumbnail: Some(thumbnail),
    })
}

// A bounded writer also covers incompressible normalized images larger than the upload.
struct BoundedOutput {
    bytes: Vec<u8>,
    cap: usize,
}

impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.cap.saturating_sub(self.bytes.len()) {
            return Err(io::Error::from(io::ErrorKind::OutOfMemory));
        }
        let required = self.bytes.len() + bytes.len();
        if required > self.bytes.capacity() {
            // Geometric growth avoids quadratic reallocations for small JPEG writes,
            // but never reserves capacity beyond the explicit output cap.
            let target = required
                .max(self.bytes.capacity().saturating_mul(2))
                .max(4096)
                .min(self.cap);
            self.bytes
                .try_reserve_exact(target - self.bytes.len())
                .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode(pixels: &DynamicImage, kind: Kind, cap: usize) -> Result<Vec<u8>, IngestError> {
    let mut out = BoundedOutput {
        bytes: Vec::new(),
        cap,
    };
    let (w, h) = (pixels.width(), pixels.height());
    match kind {
        Kind::Jpeg => {
            let rgb = pixels.to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
                .encode(rgb.as_raw(), w, h, image::ExtendedColorType::Rgb8)
                .map_err(image_error)?;
        }
        Kind::Png => {
            // Keep grayscale, alpha and 16-bit samples without copying uploaded metadata.
            image::codecs::png::PngEncoder::new(&mut out)
                .write_image(pixels.as_bytes(), w, h, pixels.color().into())
                .map_err(image_error)?;
        }
        Kind::Gif => {
            let rgba = pixels.to_rgba8();
            image::codecs::gif::GifEncoder::new(&mut out)
                .encode(rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8)
                .map_err(image_error)?;
        }
        Kind::Webp => {
            if pixels.color().has_alpha() {
                let rgba = pixels.to_rgba8();
                image::codecs::webp::WebPEncoder::new_lossless(&mut out)
                    .write_image(rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8)
                    .map_err(image_error)?;
            } else {
                let rgb = pixels.to_rgb8();
                image::codecs::webp::WebPEncoder::new_lossless(&mut out)
                    .write_image(rgb.as_raw(), w, h, image::ExtendedColorType::Rgb8)
                    .map_err(image_error)?;
            }
        }
        _ => return Err(IngestError::Invalid),
    }
    Ok(out.bytes)
}

fn be32(bytes: &[u8]) -> Result<u32, IngestError> {
    Ok(u32::from_be_bytes(
        bytes
            .get(..4)
            .ok_or(IngestError::Invalid)?
            .try_into()
            .map_err(|_| IngestError::Invalid)?,
    ))
}
fn le32(bytes: &[u8]) -> Result<u32, IngestError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(..4)
            .ok_or(IngestError::Invalid)?
            .try_into()
            .map_err(|_| IngestError::Invalid)?,
    ))
}
fn le24(bytes: &[u8]) -> Result<u32, IngestError> {
    let b = bytes.get(..3).ok_or(IngestError::Invalid)?;
    Ok(u32::from(b[0]) | (u32::from(b[1]) << 8) | (u32::from(b[2]) << 16))
}
fn le16(bytes: &[u8]) -> Result<u32, IngestError> {
    let b = bytes.get(..2).ok_or(IngestError::Invalid)?;
    Ok(u32::from(u16::from_le_bytes([b[0], b[1]])))
}
fn orientation(exif: &[u8]) -> Orientation {
    Orientation::from_exif_chunk(exif).unwrap_or(Orientation::NoTransforms)
}

fn prepare(
    bytes: &[u8],
    kind: Kind,
    limits: &Limits,
) -> Result<(Vec<u8>, Orientation), IngestError> {
    match kind {
        Kind::Png => prepare_png(bytes, limits),
        Kind::Jpeg => prepare_jpeg(bytes, limits),
        Kind::Webp => prepare_webp(bytes, limits),
        Kind::Gif => {
            check_dimensions(
                le16(bytes.get(6..).ok_or(IngestError::Invalid)?)?,
                le16(bytes.get(8..).ok_or(IngestError::Invalid)?)?,
                limits,
            )?;
            preflight_gif(bytes, limits)?;
            Ok((bytes.to_vec(), Orientation::NoTransforms))
        }
        _ => Err(IngestError::Invalid),
    }
}

fn prepare_png(bytes: &[u8], limits: &Limits) -> Result<(Vec<u8>, Orientation), IngestError> {
    if bytes.get(8..16) != Some(b"\0\0\0\rIHDR") {
        return Err(IngestError::Invalid);
    }
    check_dimensions(
        be32(bytes.get(16..).ok_or(IngestError::Invalid)?)?,
        be32(bytes.get(20..).ok_or(IngestError::Invalid)?)?,
        limits,
    )?;
    let mut out = bytes[..8].to_vec();
    let mut pos = 8usize;
    let mut orient = Orientation::NoTransforms;
    while pos < bytes.len() {
        let length = be32(&bytes[pos..])? as usize;
        let end = pos
            .checked_add(length)
            .and_then(|v| v.checked_add(12))
            .ok_or(IngestError::Invalid)?;
        let chunk = bytes.get(pos..end).ok_or(IngestError::Invalid)?;
        let tag = &chunk[4..8];
        if tag == b"eXIf" {
            orient = orientation(&chunk[8..8 + length]);
        }
        if matches!(tag, b"IHDR" | b"PLTE" | b"tRNS" | b"IDAT" | b"IEND") {
            if tag == b"IHDR" && pos != 8 {
                return Err(IngestError::Invalid);
            }
            out.extend_from_slice(chunk);
        } else if tag[0] & 32 == 0 {
            return Err(IngestError::Invalid); // Unknown critical chunk.
        }
        pos = end;
        if tag == b"IEND" {
            return Ok((out, orient)); // Trailing data is never stored.
        }
    }
    Err(IngestError::Invalid)
}

// Return marker, segment start and next byte; only called before the first SOS.
fn jpeg_segment(bytes: &[u8], pos: usize) -> Result<(u8, usize, usize), IngestError> {
    if bytes.get(pos) != Some(&0xff) {
        return Err(IngestError::Invalid);
    }
    let mut marker_pos = pos + 1;
    while bytes.get(marker_pos) == Some(&0xff) {
        marker_pos += 1;
    }
    let marker = *bytes.get(marker_pos).ok_or(IngestError::Invalid)?;
    if matches!(marker, 0 | 0xd8 | 0xd9 | 0xd0..=0xd7 | 0x01) {
        return Err(IngestError::Invalid);
    }
    let p = marker_pos + 1;
    let b = bytes.get(p..p + 2).ok_or(IngestError::Invalid)?;
    let length = usize::from(u16::from_be_bytes([b[0], b[1]]));
    if length < 2 {
        return Err(IngestError::Invalid);
    }
    let end = p.checked_add(length).ok_or(IngestError::Invalid)?;
    if end > bytes.len() {
        return Err(IngestError::Invalid);
    }
    Ok((marker, p + 2, end))
}

fn prepare_jpeg(bytes: &[u8], limits: &Limits) -> Result<(Vec<u8>, Orientation), IngestError> {
    let mut pos = 2usize;
    let mut dimensions_seen = false;
    let mut orient = Orientation::NoTransforms;
    // Header-only pass precedes any buffer allocation.
    while pos < bytes.len() {
        let (marker, start, end) = jpeg_segment(bytes, pos)?;
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            let d = bytes.get(start..start + 5).ok_or(IngestError::Invalid)?;
            if start + 5 > end {
                return Err(IngestError::Invalid);
            }
            check_dimensions(
                u32::from(u16::from_be_bytes([d[3], d[4]])),
                u32::from(u16::from_be_bytes([d[1], d[2]])),
                limits,
            )?;
            dimensions_seen = true;
        }
        if marker == 0xe1 && start + 6 <= end && bytes.get(start..start + 6) == Some(b"Exif\0\0") {
            orient = orientation(&bytes[start + 6..end]);
        }
        if marker == 0xda {
            break;
        }
        pos = end;
    }
    if !dimensions_seen {
        return Err(IngestError::Invalid);
    }
    // Metadata does not enter the decoder. Keep encoder's fixed JFIF APP0 (only if exact).
    let mut out = bytes[..2].to_vec();
    pos = 2;
    while pos < bytes.len() {
        let (marker, start, end) = jpeg_segment(bytes, pos)?;
        let fixed_jfif =
            marker == 0xe0 && &bytes[start..end] == b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0";
        if !matches!(marker, 0xe0..=0xef | 0xfe) || fixed_jfif {
            out.extend_from_slice(&bytes[pos..end]);
        }
        if marker == 0xda {
            out.extend_from_slice(&bytes[end..]);
            return Ok((out, orient));
        }
        pos = end;
    }
    Err(IngestError::Invalid)
}

fn preflight_gif(bytes: &[u8], limits: &Limits) -> Result<(), IngestError> {
    let flags = *bytes.get(10).ok_or(IngestError::Invalid)?;
    let mut pos = 13
        + if flags & 0x80 != 0 {
            3usize << ((flags & 7) + 1)
        } else {
            0
        };
    loop {
        match bytes.get(pos) {
            Some(0x3b) => return Ok(()),
            Some(0x21) => {
                pos += 2; // Extension label, then sub-blocks.
                skip_gif_blocks(bytes, &mut pos)?;
            }
            Some(0x2c) => {
                let d = bytes.get(pos + 1..pos + 10).ok_or(IngestError::Invalid)?;
                check_dimensions(le16(&d[4..])?, le16(&d[6..])?, limits)?;
                pos += 10
                    + if d[8] & 0x80 != 0 {
                        3usize << ((d[8] & 7) + 1)
                    } else {
                        0
                    };
                if bytes.get(pos).is_none() {
                    return Err(IngestError::Invalid);
                }
                pos += 1; // LZW minimum code size.
                skip_gif_blocks(bytes, &mut pos)?;
            }
            _ => return Err(IngestError::Invalid),
        }
    }
}

fn skip_gif_blocks(bytes: &[u8], pos: &mut usize) -> Result<(), IngestError> {
    loop {
        let n = usize::from(*bytes.get(*pos).ok_or(IngestError::Invalid)?);
        *pos += 1;
        if n == 0 {
            return Ok(());
        }
        *pos = pos.checked_add(n).ok_or(IngestError::Invalid)?;
        if *pos > bytes.len() {
            return Err(IngestError::Invalid);
        }
    }
}

fn webp_dimensions(tag: &[u8], data: &[u8]) -> Result<Option<(u32, u32)>, IngestError> {
    Ok(match tag {
        b"VP8X" => Some((
            le24(data.get(4..).ok_or(IngestError::Invalid)?)? + 1,
            le24(data.get(7..).ok_or(IngestError::Invalid)?)? + 1,
        )),
        b"VP8L" => {
            if data.first() != Some(&0x2f) {
                return Err(IngestError::Invalid);
            }
            let bits = le32(data.get(1..).ok_or(IngestError::Invalid)?)?;
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8 " => {
            if data.get(3..6) != Some(b"\x9d\x01\x2a") {
                return Err(IngestError::Invalid);
            }
            Some((
                le16(data.get(6..).ok_or(IngestError::Invalid)?)? & 0x3fff,
                le16(data.get(8..).ok_or(IngestError::Invalid)?)? & 0x3fff,
            ))
        }
        _ => None,
    })
}

fn webp_chunks<'a>(
    bytes: &'a [u8],
    mut visit: impl FnMut(&'a [u8], &'a [u8], &'a [u8]) -> Result<(), IngestError>,
) -> Result<(), IngestError> {
    let mut pos = 0usize;
    while pos < bytes.len() {
        #[cfg(test)]
        WEBP_CHUNK_STEPS.with(|steps| steps.set(steps.get() + 1));
        let header = bytes.get(pos..pos + 8).ok_or(IngestError::Invalid)?;
        let len = le32(&header[4..])? as usize;
        let end = pos
            .checked_add(8)
            .and_then(|v| v.checked_add(len))
            .ok_or(IngestError::Invalid)?;
        let padded_end = end.checked_add(len & 1).ok_or(IngestError::Invalid)?;
        let data = bytes.get(pos + 8..end).ok_or(IngestError::Invalid)?;
        let whole = bytes.get(pos..padded_end).ok_or(IngestError::Invalid)?;
        visit(&header[..4], data, whole)?;
        pos = padded_end;
    }
    Ok(())
}

#[derive(Default)]
struct WebpImage<'a> {
    image: Option<(&'a [u8], &'a [u8])>,
    alpha: Option<&'a [u8]>,
}
impl<'a> WebpImage<'a> {
    fn add(&mut self, tag: &'a [u8], data: &'a [u8]) -> Result<(), IngestError> {
        if tag == b"ALPH" {
            if self.alpha.is_some() || self.image.is_some() {
                return Err(IngestError::Invalid);
            }
            self.alpha = Some(data);
        } else {
            if self.image.is_some() || (self.alpha.is_some() && tag != b"VP8 ") {
                return Err(IngestError::Invalid);
            }
            self.image = Some((tag, data));
        }
        Ok(())
    }
    fn validate(&self) -> Result<(), IngestError> {
        self.image.map(|_| ()).ok_or(IngestError::Invalid)
    }
}

fn prepare_webp(bytes: &[u8], limits: &Limits) -> Result<(Vec<u8>, Orientation), IngestError> {
    if bytes.get(..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WEBP") {
        return Err(IngestError::Invalid);
    }
    let size = le32(bytes.get(4..).ok_or(IngestError::Invalid)?)? as usize;
    let end = size.checked_add(8).ok_or(IngestError::Invalid)?;
    let chunks = bytes.get(12..end).ok_or(IngestError::Invalid)?;
    // Validate the complete container BEFORE any auxiliary-pixel walk. The decoder
    // retains the first chunk of each kind and decodes only the first ANMF frame.
    // Reject ambiguous image/alpha duplicates, including in unused frames, rather
    // than letting the guard and decoder select different streams.
    let mut canvas = None;
    let mut animated = false;
    let mut top = WebpImage::default();
    let mut first_frame = None;
    let mut orient = Orientation::NoTransforms;
    let mut chunk_index = 0;
    webp_chunks(chunks, |tag, data, _| {
        if let Some((w, h)) = webp_dimensions(tag, data)? {
            check_dimensions(w, h, limits)?;
        }
        match tag {
            b"VP8X" => {
                if chunk_index != 0 || canvas.is_some() {
                    return Err(IngestError::Invalid);
                }
                canvas = webp_dimensions(tag, data)?;
                animated = data[0] & 2 != 0;
            }
            b"VP8 " | b"VP8L" | b"ALPH" => top.add(tag, data)?,
            b"ANMF" => {
                if !animated {
                    return Err(IngestError::Invalid);
                }
                let head = data.get(..16).ok_or(IngestError::Invalid)?;
                let dims = (le24(&head[6..])? + 1, le24(&head[9..])? + 1);
                check_dimensions(dims.0, dims.1, limits)?;
                let (cw, ch) = canvas.ok_or(IngestError::Invalid)?;
                if u64::from(le24(head)?) * 2 + u64::from(dims.0) > u64::from(cw)
                    || u64::from(le24(&head[3..])?) * 2 + u64::from(dims.1) > u64::from(ch)
                {
                    return Err(IngestError::Invalid);
                }
                let mut frame = WebpImage::default();
                webp_chunks(&data[16..], |t, d, _| {
                    if !matches!(t, b"ALPH" | b"VP8 " | b"VP8L") {
                        return Err(IngestError::Invalid);
                    }
                    if let Some((w, h)) = webp_dimensions(t, d)? {
                        check_dimensions(w, h, limits)?;
                        if (w, h) != dims {
                            return Err(IngestError::Invalid);
                        }
                    }
                    frame.add(t, d)
                })?;
                frame.validate()?;
                if first_frame.is_none() {
                    first_frame = Some((frame, dims));
                }
            }
            b"EXIF" => orient = orientation(data.strip_prefix(b"Exif\0\0").unwrap_or(data)),
            _ => {}
        }
        chunk_index += 1;
        Ok(())
    })?;
    let (selected, dims) = if animated {
        if top.image.is_some() || top.alpha.is_some() {
            return Err(IngestError::Invalid);
        }
        first_frame.ok_or(IngestError::Invalid)?
    } else {
        let dims = canvas
            .or_else(|| {
                top.image
                    .and_then(|(t, d)| webp_dimensions(t, d).ok().flatten())
            })
            .ok_or(IngestError::Invalid)?;
        if let Some((t, d)) = top.image
            && webp_dimensions(t, d)? != Some(dims)
        {
            return Err(IngestError::Invalid);
        }
        (top, dims)
    };
    selected.validate()?;
    if let Some(alpha) = selected.alpha {
        preflight_webp_entropy(b"ALPH", alpha, dims)?;
    }
    if let Some((tag, data)) = selected.image {
        preflight_webp_entropy(tag, data, dims)?;
    }
    let mut out = bytes[..12].to_vec();
    webp_chunks(chunks, |tag, _, whole| {
        if matches!(tag, b"ICCP" | b"EXIF" | b"XMP ") {
            return Ok(());
        }
        let start = out.len();
        out.extend_from_slice(whole);
        if tag == b"VP8X" {
            out[start + 8] &= !(0x20 | 0x08 | 0x04); // Drop ICC/EXIF/XMP flags.
        }
        Ok(())
    })?;
    let size = u32::try_from(out.len() - 8).map_err(|_| IngestError::TooLarge)?;
    out[4..8].copy_from_slice(&size.to_le_bytes());
    Ok((out, orient))
}

fn webp_has_vp8(bytes: &[u8]) -> Result<bool, IngestError> {
    // Normalization retains only the first image: later animation codecs do not
    // turn a lossless first frame into a lossy JPEG.
    let mut first_lossy = None;
    webp_chunks(
        bytes.get(12..).ok_or(IngestError::Invalid)?,
        |tag, data, _| {
            if first_lossy.is_none() && matches!(tag, b"VP8 " | b"VP8L") {
                first_lossy = Some(tag == b"VP8 ");
            }
            if tag == b"ANMF" && first_lossy.is_none() {
                webp_chunks(data.get(16..).ok_or(IngestError::Invalid)?, |t, _, _| {
                    if first_lossy.is_none() && matches!(t, b"VP8 " | b"VP8L") {
                        first_lossy = Some(t == b"VP8 ");
                    }
                    Ok(())
                })?;
            }
            Ok(())
        },
    )?;
    first_lossy.ok_or(IngestError::Invalid)
}

#[cfg(test)]
#[path = "ingest_webp_tests.rs"]
mod webp_tests;

// The main meta-Huffman bit alone is dangerous. Auxiliary streams cannot contain meta
// groups: consume their bounded Huffman/LZ77 syntax without allocating pixel images.
fn preflight_webp_entropy(tag: &[u8], data: &[u8], dims: (u32, u32)) -> Result<(), IngestError> {
    let (stream, mut w, h) = match tag {
        b"VP8L" => {
            let (w, h) = webp_dimensions(tag, data)?.ok_or(IngestError::Invalid)?;
            (data.get(5..).ok_or(IngestError::Invalid)?, w, h)
        }
        b"ALPH" => {
            let flags = *data.first().ok_or(IngestError::Invalid)?;
            match flags & 3 {
                0 => return Ok(()),
                1 => (&data[1..], dims.0, dims.1),
                _ => return Err(IngestError::Invalid),
            }
        }
        _ => return Ok(()),
    };
    let mut bits = LsbBits {
        bytes: stream,
        pos: 0,
    };
    // At most two subsampled transform images, plus a <=256-entry palette.
    // Charge their entire declared size, even on zero-bit fast paths. One selected
    // VP8L/ALPH stream is walked per ingest, so this is also the total ingest cap.
    let mut auxiliary_pixels = 2 * u64::from(w.div_ceil(4)) * u64::from(h.div_ceil(4)) + 256;
    let mut seen = 0u8;
    while bits.read(1)? != 0 {
        let transform = bits.read(2)? as u8;
        if seen & (1 << transform) != 0 {
            return Err(IngestError::Invalid);
        }
        seen |= 1 << transform;
        match transform {
            0 | 1 => {
                let shift = bits.read(3)? + 2;
                skip_webp_auxiliary(
                    &mut bits,
                    w.div_ceil(1 << shift),
                    h.div_ceil(1 << shift),
                    &mut auxiliary_pixels,
                )?;
            }
            2 => {}
            3 => {
                let colors = bits.read(8)? + 1;
                skip_webp_auxiliary(&mut bits, colors, 1, &mut auxiliary_pixels)?;
                let shift = if colors <= 2 {
                    3
                } else if colors <= 4 {
                    2
                } else if colors <= 16 {
                    1
                } else {
                    0
                };
                w = w.div_ceil(1 << shift);
            }
            _ => return Err(IngestError::Invalid),
        }
    }
    webp_color_cache(&mut bits)?;
    if bits.read(1)? != 0 {
        return Err(IngestError::UnsupportedType);
    }
    Ok(())
}

struct LsbBits<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl LsbBits<'_> {
    fn read(&mut self, count: usize) -> Result<u32, IngestError> {
        let mut value = 0;
        for i in 0..count {
            let byte = *self.bytes.get(self.pos / 8).ok_or(IngestError::Invalid)?;
            value |= u32::from((byte >> (self.pos % 8)) & 1) << i;
            self.pos += 1;
        }
        Ok(value)
    }
}

fn webp_color_cache(bits: &mut LsbBits<'_>) -> Result<u32, IngestError> {
    if bits.read(1)? == 0 {
        return Ok(0);
    }
    let n = bits.read(4)?;
    if !(1..=11).contains(&n) {
        return Err(IngestError::Invalid);
    }
    Ok(1 << n)
}

// A tiny canonical-code walker. At most 2328 symbols / 4655 nodes per tree;
// code lengths are <=15. No image-sized tables or buffers are built by the guard.
struct WebpHuffman {
    nodes: Vec<[i32; 2]>,
    single: Option<u32>,
}
impl WebpHuffman {
    fn build(lengths: &[u8]) -> Result<Self, IngestError> {
        let mut counts = [0u32; 16];
        for &len in lengths {
            counts[usize::from(len)] += 1;
        }
        let symbols = lengths.len() as u32 - counts[0];
        if symbols == 0 {
            return Err(IngestError::Invalid);
        }
        if symbols == 1 {
            return Ok(Self {
                nodes: Vec::new(),
                single: Some(
                    lengths
                        .iter()
                        .position(|&v| v != 0)
                        .ok_or(IngestError::Invalid)? as u32,
                ),
            });
        }
        let mut next = [0u32; 16];
        let mut code = 0;
        for len in 1..16 {
            next[len] = code;
            code = (code + counts[len]) << 1;
            if code > 1 << (len + 1) {
                return Err(IngestError::Invalid);
            }
        }
        if code != 1 << 16 {
            return Err(IngestError::Invalid);
        }
        let mut nodes = Vec::with_capacity(2 * symbols as usize);
        nodes.push([0, 0]);
        for (symbol, &len) in lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let code = next[len as usize];
            next[len as usize] += 1;
            let mut node = 0;
            for depth in (0..len).rev() {
                let branch = ((code >> depth) & 1) as usize;
                if depth == 0 {
                    if nodes[node][branch] != 0 {
                        return Err(IngestError::Invalid);
                    }
                    nodes[node][branch] = -(symbol as i32) - 1;
                } else {
                    if nodes[node][branch] == 0 {
                        nodes[node][branch] = nodes.len() as i32;
                        nodes.push([0, 0]);
                    }
                    if nodes[node][branch] < 0 {
                        return Err(IngestError::Invalid);
                    }
                    node = nodes[node][branch] as usize;
                }
            }
        }
        Ok(Self {
            nodes,
            single: None,
        })
    }
    fn read(&self, bits: &mut LsbBits<'_>) -> Result<u32, IngestError> {
        if let Some(symbol) = self.single {
            return Ok(symbol);
        }
        let mut node = 0;
        for _ in 0..15 {
            let child = self.nodes[node][bits.read(1)? as usize];
            if child < 0 {
                return Ok((-child - 1) as u32);
            }
            if child == 0 {
                return Err(IngestError::Invalid);
            }
            node = child as usize;
        }
        Err(IngestError::Invalid)
    }
    fn parse(bits: &mut LsbBits<'_>, alphabet: usize) -> Result<Self, IngestError> {
        if bits.read(1)? != 0 {
            let two = bits.read(1)? != 0;
            let wide = bits.read(1)? != 0;
            let first = bits.read(if wide { 8 } else { 1 })?;
            if first as usize >= alphabet {
                return Err(IngestError::Invalid);
            }
            if !two {
                return Ok(Self {
                    nodes: Vec::new(),
                    single: Some(first),
                });
            }
            let second = bits.read(8)?;
            if second as usize >= alphabet {
                return Err(IngestError::Invalid);
            }
            if second < first {
                // libwebp emits ascending pairs. Fail closed on descending codes
                // rather than canonicalizing them and moving the main meta bit.
                return Err(IngestError::UnsupportedType);
            }
            // Mirror image-webp's build_two_node, including first == second:
            // bit zero is FIRST, bit one SECOND, and both consume exactly one bit.
            return Ok(Self {
                nodes: vec![[-(first as i32) - 1, -(second as i32) - 1]],
                single: None,
            });
        }
        let mut lengths = vec![0u8; alphabet];
        let order = [
            17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
        ];
        let count = bits.read(4)? as usize + 4;
        let mut code_lengths = [0u8; 19];
        for &slot in &order[..count] {
            code_lengths[slot] = bits.read(3)? as u8;
        }
        let tree = Self::build(&code_lengths)?;
        let max = if bits.read(1)? != 0 {
            let nbits = 2 + 2 * bits.read(3)? as usize;
            let max = bits.read(nbits)? as usize + 2;
            if max > alphabet {
                return Err(IngestError::Invalid);
            }
            max
        } else {
            alphabet
        };
        let mut pos = 0;
        let mut previous = 8;
        for _ in 0..max {
            if pos == alphabet {
                break;
            }
            let code = tree.read(bits)?;
            if code < 16 {
                lengths[pos] = code as u8;
                pos += 1;
                if code != 0 {
                    previous = code as u8;
                }
            } else {
                let (extra, base) = match code {
                    16 => (2, 3),
                    17 => (3, 3),
                    18 => (7, 11),
                    _ => return Err(IngestError::Invalid),
                };
                let repeat = bits.read(extra)? as usize + base;
                let end = pos.checked_add(repeat).ok_or(IngestError::Invalid)?;
                lengths
                    .get_mut(pos..end)
                    .ok_or(IngestError::Invalid)?
                    .fill(if code == 16 { previous } else { 0 });
                pos = end;
            }
        }
        Self::build(&lengths)
    }
}
fn webp_prefix_value(bits: &mut LsbBits<'_>, code: u32) -> Result<u32, IngestError> {
    if code < 4 {
        return Ok(code + 1);
    }
    let extra = ((code - 2) >> 1) as usize;
    Ok(((2 + (code & 1)) << extra) + bits.read(extra)? + 1)
}
// Tests count entropy-loop work rather than benchmarking debug builds under load.
#[cfg(test)]
thread_local! {
    static WEBP_AUXILIARY_STEPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static WEBP_CHUNK_STEPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
fn skip_webp_auxiliary(
    bits: &mut LsbBits<'_>,
    w: u32,
    h: u32,
    remaining: &mut u64,
) -> Result<(), IngestError> {
    let count = u64::from(w) * u64::from(h);
    *remaining = remaining.checked_sub(count).ok_or(IngestError::Invalid)?;
    let cache = webp_color_cache(bits)? as usize;
    let trees = [
        WebpHuffman::parse(bits, 280 + cache)?,
        WebpHuffman::parse(bits, 256)?,
        WebpHuffman::parse(bits, 256)?,
        WebpHuffman::parse(bits, 256)?,
        WebpHuffman::parse(bits, 40)?,
    ];
    // Exhaust all zero-bit paths, not just single literal trees. Single cache
    // references consume no bits; an all-reference stream has no initial pixel
    // to copy and is invalid. Otherwise each iteration must consume >=1 bit,
    // bounding work by input bits as well as the declared auxiliary-pixel cap.
    if let Some(green) = trees[0].single {
        if (green < 256 && trees[1..4].iter().all(|t| t.single.is_some())) || green >= 280 {
            return Ok(());
        }
        if (256..260).contains(&green) && trees[4].single.is_some_and(|s| s < 4) {
            return Err(IngestError::Invalid);
        }
    }
    let mut pos = 0;
    while pos < count {
        #[cfg(test)]
        WEBP_AUXILIARY_STEPS.with(|steps| steps.set(steps.get() + 1));
        let code = trees[0].read(bits)?;
        let run = if code < 256 {
            for tree in &trees[1..4] {
                tree.read(bits)?;
            }
            1
        } else if code < 280 {
            let length = webp_prefix_value(bits, code - 256)?;
            let distance = trees[4].read(bits)?;
            webp_prefix_value(bits, distance)?;
            // Distances affect pixel values, not bit consumption. The actual decoder
            // validates references later; no pixel values or distance-sized memory here.
            u64::from(length)
        } else {
            1
        }; // Colour cache references contain no more bits.
        pos = pos.checked_add(run).ok_or(IngestError::Invalid)?;
        if pos > count {
            return Err(IngestError::Invalid);
        }
    }
    Ok(())
}
