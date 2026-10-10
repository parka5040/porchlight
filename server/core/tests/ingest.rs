use std::io::Cursor;

use image::{DynamicImage, ImageFormat, RgbImage, RgbaImage};
use porchlight_core::ingest::{IngestError, Limits, blob_relative_path, ingest};
use proptest::prelude::*;
use sha2::{Digest, Sha256};

fn fixture(format: ImageFormat, alpha: bool) -> Vec<u8> {
    let image = if alpha {
        DynamicImage::ImageRgba8(RgbaImage::from_fn(24, 12, |x, y| {
            image::Rgba([
                x as u8 * 10,
                y as u8 * 20,
                150,
                if x % 2 == 0 { 0 } else { 255 },
            ])
        }))
    } else {
        DynamicImage::ImageRgb8(RgbImage::from_fn(24, 12, |x, y| {
            image::Rgb([x as u8 * 10, y as u8 * 20, 150])
        }))
    };
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, format).unwrap();
    out.into_inner()
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn production_limits_and_stable_error_codes() {
    let l = Limits::production();
    assert_eq!(l.max_bytes, 25 * 1024 * 1024);
    assert_eq!(l.thumb_max_edge, 320);
    assert_eq!(l.max_pixels, 25_000_000);
    assert_eq!(l.max_dimension, 16_384);
    assert_eq!(l.max_alloc_bytes, 512 * 1024 * 1024);
    assert_eq!(IngestError::TooLarge.code(), "attachment-too-large");
    assert_eq!(
        IngestError::UnsupportedType.code(),
        "attachment-type-unsupported"
    );
    assert_eq!(IngestError::TypeMismatch.code(), "attachment-type-mismatch");
    assert_eq!(IngestError::Invalid.code(), "attachment-invalid");
}

#[test]
fn bytes_limit_precedes_parsing_and_hints() {
    let l = Limits {
        max_bytes: 3,
        ..Limits::production()
    };
    assert_eq!(
        ingest(b"garbage", Some("a.exe"), Some("bad"), &l),
        Err(IngestError::TooLarge)
    );
    assert!(ingest(b"abc", None, None, &l).is_ok());
}

#[test]
fn text_pdf_validation_hashes_and_hints() {
    for bytes in [b"hello \xe2\x98\x83\n".as_slice(), b"".as_slice()] {
        let out = ingest(
            bytes,
            Some("note.TXT"),
            Some("text/plain; charset=utf-8"),
            &Limits::production(),
        )
        .unwrap();
        assert_eq!(out.bytes, bytes);
        assert_eq!(out.content_type, "text/plain");
        assert_eq!(out.sha256, hash(bytes));
        assert!(out.thumbnail.is_none());
        assert_eq!(out.width, None);
    }
    for bytes in [b"a\0b".as_slice(), b"\xff\xfe\x00".as_slice()] {
        assert!(ingest(bytes, None, None, &Limits::production()).is_err());
    }
    for version in ["1.7", "2.0"] {
        let pdf = format!("%PDF-{version}\n/Author (preserved)\n%%EOF\n");
        let out = ingest(
            pdf.as_bytes(),
            Some("doc.pdf"),
            Some("application/pdf"),
            &Limits::production(),
        )
        .unwrap();
        assert_eq!(out.bytes, pdf.as_bytes());
        assert_eq!(out.sha256, hash(pdf.as_bytes()));
        assert!(out.thumbnail.is_none());
    }
    for bytes in [
        b"%PDF-1.7\nmissing eof".as_slice(),
        b"%PDF-3.0\n%%EOF".as_slice(),
    ] {
        assert_eq!(
            ingest(bytes, None, None, &Limits::production()),
            Err(IngestError::Invalid)
        );
    }
    let mut pdf = b"%PDF-1.7\n%%EOF".to_vec();
    pdf.extend(vec![b' '; 1100]);
    assert_eq!(
        ingest(&pdf, None, None, &Limits::production()),
        Err(IngestError::Invalid)
    );
    let png = fixture(ImageFormat::Png, false);
    for (name, mime) in [(Some("lie.jpg"), None), (None, Some("text/plain"))] {
        assert_eq!(
            ingest(&png, name, mime, &Limits::production()),
            Err(IngestError::TypeMismatch)
        );
    }
    assert!(
        ingest(
            &png,
            Some("image.PNG"),
            Some("IMAGE/PNG"),
            &Limits::production()
        )
        .is_ok()
    );
    assert!(
        ingest(
            &png,
            Some("file"),
            Some("application/octet-stream"),
            &Limits::production()
        )
        .is_ok()
    );
    for bytes in [
        b"PK\x03\x04\x00\x00\x00\x00".as_slice(),
        b"MZ\x00\x00\x00\x00".as_slice(),
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".as_slice(),
    ] {
        assert_eq!(
            ingest(bytes, None, None, &Limits::production()),
            Err(IngestError::UnsupportedType)
        );
    }
}

#[test]
fn strict_relative_blob_paths() {
    let sha = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    assert_eq!(
        blob_relative_path(sha).unwrap().to_str().unwrap(),
        format!("ab/cd/{sha}")
    );
    for bad in [
        "",
        "../abc",
        &"a".repeat(63),
        &"a".repeat(65),
        &"A".repeat(64),
        &"g".repeat(64),
        &format!("{} /", "a".repeat(62)),
    ] {
        assert_eq!(blob_relative_path(bad), Err(IngestError::Invalid));
    }
}

#[test]
fn every_image_format_is_bounded_and_hash_idempotent() {
    for format in [
        ImageFormat::Jpeg,
        ImageFormat::Png,
        ImageFormat::Gif,
        ImageFormat::WebP,
    ] {
        for alpha in [false, true] {
            if format == ImageFormat::Jpeg && alpha {
                continue;
            }
            let bytes = fixture(format, alpha);
            let l = Limits {
                thumb_max_edge: 8,
                ..Limits::production()
            };
            let out = ingest(&bytes, None, None, &l).unwrap();
            assert_eq!((out.width, out.height), (Some(24), Some(12)));
            assert_eq!(out.sha256, hash(&out.bytes));
            let thumb = out.thumbnail.as_ref().unwrap();
            assert_eq!((thumb.width, thumb.height), (8, 4));
            assert_eq!(thumb.sha256, hash(&thumb.bytes));
            assert_eq!(
                thumb.content_type,
                if alpha || format == ImageFormat::Gif {
                    "image/png"
                } else {
                    "image/jpeg"
                }
            );
            assert_eq!(image::load_from_memory(&thumb.bytes).unwrap().width(), 8);
            let again = ingest(&out.bytes, None, Some(out.content_type), &l).unwrap();
            if format != ImageFormat::Jpeg {
                assert_eq!(again.sha256, out.sha256, "{format:?}, alpha={alpha}");
                assert_eq!(again.thumbnail.unwrap().sha256, thumb.sha256);
            }
            assert_eq!(ingest(&bytes, None, None, &l).unwrap(), out);
        }
    }
}

// Little-endian TIFF: orientation entry + pointer to a GPS IFD carrying a GPS text marker.
fn exif() -> Vec<u8> {
    let mut out = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
    out.extend_from_slice(&[0x25, 0x88, 4, 0, 1, 0, 0, 0, 38, 0, 0, 0]);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&[0x1d, 0, 2, 0, 13, 0, 0, 0, 56, 0, 0, 0]);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(b"GPS-SECRET-42\0");
    out
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&out[4..]).to_be_bytes());
    out
}

fn contains(bytes: &[u8], marker: &[u8]) -> bool {
    bytes.windows(marker.len()).any(|w| w == marker)
}

#[test]
fn jpeg_gps_orientation_and_comments_are_removed_before_hashing() {
    let jpeg = fixture(ImageFormat::Jpeg, false);
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend(exif());
    let mut input = jpeg[..2].to_vec();
    input.extend_from_slice(&[0xff, 0xe1]);
    input.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    input.extend(app1);
    input.extend_from_slice(&[0xff, 0xfe, 0, 17]);
    input.extend_from_slice(b"PRIVATE-COMMENT");
    input.extend_from_slice(&jpeg[2..]);
    let out = ingest(&input, None, None, &Limits::production()).unwrap();
    assert_eq!((out.width, out.height), (Some(12), Some(24)));
    for marker in [
        b"Exif".as_slice(),
        b"GPS-SECRET-42".as_slice(),
        b"PRIVATE-COMMENT".as_slice(),
    ] {
        assert!(!contains(&out.bytes, marker));
        assert!(!contains(&out.thumbnail.as_ref().unwrap().bytes, marker));
    }
    assert_eq!(out.sha256, hash(&out.bytes));
    assert_ne!(out.sha256, hash(&input));
    assert_eq!(
        ingest(&input, None, None, &Limits::production()).unwrap(),
        out
    );
}

#[test]
fn png_text_exif_and_other_ancillary_chunks_are_removed() {
    let png = fixture(ImageFormat::Png, true);
    let mut input = png[..33].to_vec();
    input.extend(png_chunk(b"tEXt", b"Comment\0PRIVATE-TEXT"));
    input.extend(png_chunk(b"eXIf", &exif()));
    input.extend(png_chunk(b"iTXt", b"Note\0\0\0\0\0PRIVATE-ITXT"));
    input.extend_from_slice(&png[33..]);
    let out = ingest(&input, None, None, &Limits::production()).unwrap();
    assert_eq!((out.width, out.height), (Some(12), Some(24)));
    for marker in [
        b"tEXt".as_slice(),
        b"eXIf".as_slice(),
        b"iTXt".as_slice(),
        b"PRIVATE-TEXT".as_slice(),
        b"GPS-SECRET-42".as_slice(),
    ] {
        assert!(!contains(&out.bytes, marker));
    }
    assert_eq!(out.thumbnail.unwrap().content_type, "image/png");
    assert_eq!(
        ingest(&out.bytes, None, None, &Limits::production())
            .unwrap()
            .sha256,
        out.sha256
    );
}

#[test]
fn gif_drops_animation_and_comments() {
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        for c in [10, 200] {
            encoder
                .encode_frame(image::Frame::new(RgbaImage::from_pixel(
                    8,
                    4,
                    image::Rgba([c, 0, 0, 255]),
                )))
                .unwrap();
        }
    }
    let trailer = bytes.len() - 1;
    bytes.splice(
        trailer..trailer,
        b"\x21\xfe\x0dGIF-SECRET-42\x00".iter().copied(),
    );
    let out = ingest(&bytes, None, None, &Limits::production()).unwrap();
    assert!(!contains(&out.bytes, b"GIF-SECRET-42"));
    use image::AnimationDecoder;
    let frames = image::codecs::gif::GifDecoder::new(Cursor::new(&out.bytes))
        .unwrap()
        .into_frames()
        .collect_frames()
        .unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].buffer().get_pixel(0, 0).0, [10, 0, 0, 255]);
}

#[test]
fn header_bombs_and_real_compressed_images_hit_limits() {
    let png = fixture(ImageFormat::Png, false);
    let mut data = png[16..29].to_vec();
    data[..4].copy_from_slice(&100_000u32.to_be_bytes());
    data[4..8].copy_from_slice(&100_000u32.to_be_bytes());
    let mut bomb = png[..8].to_vec();
    bomb.extend(png_chunk(b"IHDR", &data));
    // No image data: the header must be rejected before attempting to decode.
    assert_eq!(
        ingest(&bomb, None, None, &Limits::production()),
        Err(IngestError::TooLarge)
    );
    let big = DynamicImage::ImageRgba8(RgbaImage::new(2048, 2048));
    let mut bytes = Cursor::new(Vec::new());
    big.write_to(&mut bytes, ImageFormat::Png).unwrap();
    assert!(bytes.get_ref().len() < 100_000);
    let l = Limits {
        max_pixels: 100_000,
        ..Limits::production()
    };
    assert_eq!(
        ingest(bytes.get_ref(), None, None, &l),
        Err(IngestError::TooLarge)
    );
    for l in [
        Limits {
            max_dimension: 23,
            ..Limits::production()
        },
        Limits {
            max_pixels: 287,
            ..Limits::production()
        },
        Limits {
            max_alloc_bytes: 1024,
            ..Limits::production()
        },
    ] {
        assert_eq!(ingest(&png, None, None, &l), Err(IngestError::TooLarge));
    }
}

#[test]
fn png_preserves_sixteen_bit_pixels() {
    let image = DynamicImage::ImageRgba16(image::ImageBuffer::from_pixel(
        3,
        2,
        image::Rgba([12345u16, 23456, 34567, 45678]),
    ));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Png).unwrap();
    let out = ingest(bytes.get_ref(), None, None, &Limits::production()).unwrap();
    assert_eq!(
        image::load_from_memory(&out.bytes).unwrap().to_rgba16(),
        image.to_rgba16()
    );
    assert_eq!(
        ingest(&out.bytes, None, None, &Limits::production())
            .unwrap()
            .sha256,
        out.sha256
    );
}

#[test]
fn png_does_not_parse_compressed_metadata_and_drops_trailing_bytes() {
    let png = fixture(ImageFormat::Png, false);
    let mut input = png[..33].to_vec();
    // Deliberately invalid compressed data: pixels remain valid and these chunks must not
    // be handed to a metadata decompressor. Neither privacy nor admission trusts them.
    input.extend(png_chunk(b"iCCP", b"PRIVATE-ICC\0\0bad-zlib"));
    input.extend(png_chunk(b"zTXt", b"PRIVATE-ZTXT\0\0bad-zlib"));
    input.extend_from_slice(&png[33..]);
    input.extend_from_slice(b"TRAILING-PRIVATE");
    let out = ingest(&input, None, None, &Limits::production()).unwrap();
    for marker in [
        b"iCCP".as_slice(),
        b"zTXt".as_slice(),
        b"PRIVATE".as_slice(),
    ] {
        assert!(!contains(&out.bytes, marker));
    }
}

fn webp_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = kind.to_vec();
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
    out
}

#[test]
fn webp_exif_orientation_icc_and_xmp_are_removed() {
    let webp = fixture(ImageFormat::WebP, true);
    let mut input = webp[..12].to_vec();
    // Extended container: alpha + ICC + EXIF + XMP; 24 by 12 canvas.
    input.extend(webp_chunk(b"VP8X", &[0x3c, 0, 0, 0, 23, 0, 0, 11, 0, 0]));
    input.extend(webp_chunk(b"ICCP", b"PRIVATE-ICC"));
    input.extend_from_slice(&webp[12..]);
    input.extend(webp_chunk(b"EXIF", &exif()));
    input.extend(webp_chunk(b"XMP ", b"PRIVATE-XMP"));
    let size = (input.len() - 8) as u32;
    input[4..8].copy_from_slice(&size.to_le_bytes());
    let out = ingest(&input, None, None, &Limits::production()).unwrap();
    assert_eq!((out.width, out.height), (Some(12), Some(24)));
    for marker in [
        b"EXIF".as_slice(),
        b"ICCP".as_slice(),
        b"XMP ".as_slice(),
        b"GPS-SECRET-42".as_slice(),
        b"PRIVATE".as_slice(),
    ] {
        assert!(!contains(&out.bytes, marker));
    }
    assert_eq!(
        ingest(&out.bytes, None, None, &Limits::production())
            .unwrap()
            .sha256,
        out.sha256
    );
}

#[test]
fn all_exif_orientations_transform_pixels_before_thumbnailing() {
    let png = fixture(ImageFormat::Png, true);
    let source = image::load_from_memory(&png).unwrap();
    for code in 1..=8u8 {
        let mut exif = exif();
        exif[18] = code;
        let mut input = png[..33].to_vec();
        input.extend(png_chunk(b"eXIf", &exif));
        input.extend_from_slice(&png[33..]);
        let out = ingest(&input, None, None, &Limits::production()).unwrap();
        let mut expected = source.clone();
        expected.apply_orientation(image::metadata::Orientation::from_exif(code).unwrap());
        assert_eq!(
            image::load_from_memory(&out.bytes).unwrap().to_rgba8(),
            expected.to_rgba8()
        );
        assert_eq!(out.thumbnail.as_ref().unwrap().width, expected.width());
        assert_eq!(out.thumbnail.as_ref().unwrap().height, expected.height());
    }
}

#[test]
fn jpeg_does_not_trust_metadata_markers_or_trailing_data() {
    let l = Limits::production();
    let out = ingest(&fixture(ImageFormat::Jpeg, false), None, None, &l).unwrap();
    for segment in [
        b"\xff\xe1\x00\x10PRIVATE-APP1!!".as_slice(),
        b"\xff\xfe\x00\x10PRIVATE-COM!!!".as_slice(),
    ] {
        let mut modified = out.bytes[..2].to_vec();
        // Length includes the two length bytes; build a valid segment explicitly.
        modified.extend_from_slice(&segment[..2]);
        modified.extend_from_slice(&((segment.len() - 2) as u16).to_be_bytes());
        modified.extend_from_slice(&segment[4..]);
        modified.extend_from_slice(&out.bytes[2..]);
        let clean = ingest(&modified, None, None, &l).unwrap();
        assert!(!contains(&clean.bytes, b"PRIVATE"));
    }
    let mut modified = out.bytes.clone();
    modified.extend_from_slice(b"PRIVATE-TRAILER");
    let clean = ingest(&modified, None, None, &l).unwrap();
    assert!(!contains(&clean.bytes, b"PRIVATE-TRAILER"));
}

#[test]
fn bombs_in_gif_and_webp_headers_are_rejected_before_decoding() {
    let mut gif = b"GIF89a".to_vec();
    gif.extend_from_slice(&65535u16.to_le_bytes());
    gif.extend_from_slice(&65535u16.to_le_bytes());
    assert_eq!(
        ingest(&gif, None, None, &Limits::production()),
        Err(IngestError::TooLarge)
    );
    let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
    webp.extend(webp_chunk(
        b"VP8X",
        &[0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
    ));
    let size = (webp.len() - 8) as u32;
    webp[4..8].copy_from_slice(&size.to_le_bytes());
    assert_eq!(
        ingest(&webp, None, None, &Limits::production()),
        Err(IngestError::TooLarge)
    );
    // Small GIF canvas but a huge internal frame: canvas-only checks are insufficient.
    let mut gif = b"GIF89a\x01\x00\x01\x00\0\0\0\x2c\0\0\0\0".to_vec();
    gif.extend_from_slice(&65535u16.to_le_bytes());
    gif.extend_from_slice(&65535u16.to_le_bytes());
    gif.push(0);
    assert_eq!(
        ingest(&gif, None, None, &Limits::production()),
        Err(IngestError::TooLarge)
    );
}

#[test]
fn webp_meta_huffman_expansion_is_rejected_before_decoder_allocation() {
    // 1x1 VP8L, no transforms, no color cache, meta-Huffman present. A tiny entropy
    // image can name group 65535 and cause gigabytes of Huffman table allocation,
    // independently of the declared pixel count. Reject the feature before reading it.
    let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
    webp.extend(webp_chunk(b"VP8L", &[0x2f, 0, 0, 0, 0, 0b100]));
    let size = (webp.len() - 8) as u32;
    webp[4..8].copy_from_slice(&size.to_le_bytes());
    assert_eq!(
        ingest(&webp, None, None, &Limits::production()),
        Err(IngestError::UnsupportedType)
    );
}

#[test]
fn webp_entropy_guard_covers_animation_alpha_and_auxiliary_images() {
    let vp8l = webp_chunk(b"VP8L", &[0x2f, 0, 0, 0, 0, 0b100]);
    let mut frame = vec![0; 16];
    frame.extend(vp8l);
    let mut alpha_chunks = webp_chunk(b"ALPH", &[1, 0b100]);
    alpha_chunks.extend(webp_chunk(b"VP8 ", b"\0\0\0\x9d\x01\x2a\x01\0\x01\0"));
    for (flags, chunk) in [(2, webp_chunk(b"ANMF", &frame)), (16, alpha_chunks)] {
        let mut input = b"RIFF\0\0\0\0WEBP".to_vec();
        input.extend(webp_chunk(b"VP8X", &[flags, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        input.extend(chunk);
        let size = (input.len() - 8) as u32;
        input[4..8].copy_from_slice(&size.to_le_bytes());
        assert_eq!(
            ingest(&input, None, None, &Limits::production()),
            Err(IngestError::UnsupportedType)
        );
    }
}

#[test]
fn normalized_output_is_also_byte_bounded() {
    let input = fixture(ImageFormat::Jpeg, false);
    let normal = ingest(&input, None, None, &Limits::production()).unwrap();
    assert!(normal.bytes.len() > input.len());
    let limits = Limits {
        max_bytes: input.len(),
        ..Limits::production()
    };
    assert_eq!(
        ingest(&input, None, None, &limits),
        Err(IngestError::TooLarge)
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, failure_persistence: None, rng_seed: proptest::test_runner::RngSeed::Fixed(0x706f7263686b36), ..ProptestConfig::default() })]
    #[test]
    fn generated_pixel_images_are_hash_idempotent(
        which in 0usize..3,
        w in 1u32..24,
        h in 1u32..24,
        seed in any::<u32>(),
    ) {
        let format = [ImageFormat::Png, ImageFormat::Gif, ImageFormat::WebP][which];
        let image = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let value = seed.wrapping_add(x.wrapping_mul(0x1234567)).wrapping_add(y.wrapping_mul(0xabcdef));
            image::Rgb([(value >> 16) as u8, (value >> 8) as u8, value as u8])
        }));
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        let out = ingest(bytes.get_ref(), None, None, &Limits::production()).unwrap();
        let again = ingest(&out.bytes, None, None, &Limits::production()).unwrap();
        prop_assert_eq!(out.sha256, again.sha256);
        prop_assert_eq!(out.thumbnail.unwrap().sha256, again.thumbnail.unwrap().sha256);
    }
    #[test]
    fn mutated_images_never_panic(which in 0usize..4, position in any::<usize>(), replacement in any::<u8>()) {
        let format = [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Gif, ImageFormat::WebP][which];
        let mut bytes = fixture(format, false);
        let position = position % bytes.len();
        bytes[position] = replacement;
        if let Ok(out) = ingest(&bytes, None, None, &Limits::production()) {
            let again = ingest(&out.bytes, None, None, &Limits::production()).unwrap();
            if out.content_type != "image/jpeg" { prop_assert_eq!(again.sha256, out.sha256); }
        }
    }
    #[test]
    fn malformed_exif_never_panics(exif in prop::collection::vec(any::<u8>(), 0..512)) {
        let png = fixture(ImageFormat::Png, true);
        let mut bytes = png[..33].to_vec();
        bytes.extend(png_chunk(b"eXIf", &exif));
        bytes.extend_from_slice(&png[33..]);
        let out = ingest(&bytes, None, None, &Limits::production()).unwrap();
        prop_assert_eq!(ingest(&out.bytes, None, None, &Limits::production()).unwrap().sha256, out.sha256);
    }
    #[test]
    fn random_inputs_never_panic_and_success_is_idempotent(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        if let Ok(out) = ingest(&bytes, None, None, &Limits::production()) {
            let again = ingest(&out.bytes, None, None, &Limits::production()).unwrap();
            if out.content_type != "image/jpeg" { prop_assert_eq!(out.sha256, again.sha256); }
        }
    }
    #[test]
    fn truncated_images_never_panic(which in 0usize..4, cut in any::<usize>()) {
        let format = [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Gif, ImageFormat::WebP][which];
        let bytes = fixture(format, false);
        let cut = cut % bytes.len();
        if let Ok(out) = ingest(&bytes[..cut], None, None, &Limits::production()) {
            let again = ingest(&out.bytes, None, None, &Limits::production()).unwrap();
            if out.content_type != "image/jpeg" { prop_assert_eq!(again.sha256, out.sha256); }
        }
    }
}

#[test]
fn jpeg_entropy_is_always_transcoded() {
    let l = Limits::production();
    let out = ingest(&fixture(ImageFormat::Jpeg, false), None, None, &l).unwrap();
    let payload = b"<html><script>alert('ATTACKER-PAYLOAD')</script></html>";
    let mut html = out.bytes[..out.bytes.len() - 2].to_vec();
    html.extend_from_slice(payload);
    html.extend_from_slice(b"\xff\xd9");
    let clean = ingest(&html, None, None, &l).unwrap();
    assert!(!contains(&clean.bytes, payload));
    assert_ne!(clean.bytes, html);
    let sos = out.bytes.windows(2).position(|w| w == b"\xff\xda").unwrap();
    let end = sos + 2 + u16::from_be_bytes([out.bytes[sos + 2], out.bytes[sos + 3]]) as usize;
    let mut arbitrary = out.bytes[..end].to_vec();
    arbitrary.extend_from_slice(&[b'A'; 4000]);
    arbitrary.extend_from_slice(b"\xff\xd9");
    let clean = ingest(&arbitrary, None, None, &l).unwrap();
    assert!(!contains(&clean.bytes, &[b'A'; 4000]));
    assert_ne!(clean.bytes, arbitrary);
    assert_eq!(ingest(&arbitrary, None, None, &l).unwrap(), clean);
}

#[test]
fn hints_compare_recognized_families_only() {
    let l = Limits::production();
    for (name, mime) in [
        ("notes.md", "text/plain"),
        ("list.csv", "text/csv"),
        ("notes.txt", "text/markdown"),
        (".env", "application/octet-stream"),
    ] {
        assert_eq!(
            ingest(b"hello", Some(name), Some(mime), &l)
                .unwrap()
                .content_type,
            "text/plain"
        );
    }
    let jpeg = fixture(ImageFormat::Jpeg, false);
    for name in ["photo.jfif", "photo.jif", "photo.unknown"] {
        for mime in ["image/jpg", "image/pjpeg", "x-unknown/custom"] {
            assert!(ingest(&jpeg, Some(name), Some(mime), &l).is_ok());
        }
    }
    assert!(
        ingest(
            &fixture(ImageFormat::Png, false),
            None,
            Some("image/x-png"),
            &l
        )
        .is_ok()
    );
    for mime in ["text/html", "text/xml", "image/svg+xml", "application/pdf"] {
        assert_eq!(
            ingest(b"hello", None, Some(mime), &l),
            Err(IngestError::TypeMismatch)
        );
    }
    assert_eq!(
        ingest(&jpeg, Some("lie.png"), None, &l),
        Err(IngestError::TypeMismatch)
    );
}

#[test]
fn real_webp_profiles_and_animation() {
    let l = Limits::production();
    for (name, bytes, kind) in [
        (
            "photo lossless",
            include_bytes!("fixtures/photo-lossless.webp").as_slice(),
            "image/webp",
        ),
        (
            "photo transforms",
            include_bytes!("fixtures/photo-transforms.webp").as_slice(),
            "image/webp",
        ),
        (
            "animated alpha",
            include_bytes!("fixtures/animated-alpha.webp").as_slice(),
            "image/webp",
        ),
        (
            "graphic lossless",
            include_bytes!("fixtures/graphic-lossless.webp").as_slice(),
            "image/webp",
        ),
        (
            "photo lossy",
            include_bytes!("fixtures/photo-lossy.webp").as_slice(),
            "image/jpeg",
        ),
        (
            "smooth alpha",
            include_bytes!("fixtures/smooth-alpha.webp").as_slice(),
            "image/webp",
        ),
        (
            "sticker alpha",
            include_bytes!("fixtures/sticker-alpha.webp").as_slice(),
            "image/webp",
        ),
        (
            "animated",
            include_bytes!("fixtures/animated.webp").as_slice(),
            "image/jpeg",
        ),
    ] {
        let out =
            ingest(bytes, None, Some("image/webp"), &l).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(out.content_type, kind, "{name}");
        assert_eq!(ingest(bytes, None, None, &l).unwrap().sha256, out.sha256);
        if kind == "image/webp" {
            let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(&out.bytes)).unwrap();
            assert!(!decoder.has_animation());
            assert_eq!(
                ingest(&out.bytes, None, None, &l).unwrap().sha256,
                out.sha256
            );
            // Lossless storage retains decoded alpha and pixels exactly.
            assert_eq!(
                image::load_from_memory(&out.bytes).unwrap().to_rgba8(),
                image::load_from_memory(bytes).unwrap().to_rgba8()
            );
        } else {
            assert_eq!(image::guess_format(&out.bytes).unwrap(), ImageFormat::Jpeg);
        }
    }
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "invalid ingest Limits")]
fn invalid_configuration_is_a_programmer_error() {
    let l = Limits {
        thumb_max_edge: 0,
        ..Limits::production()
    };
    let _ = ingest(b"hello", None, None, &l);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, failure_persistence: None, rng_seed: proptest::test_runner::RngSeed::Fixed(0x706f726368666978), ..ProptestConfig::default() })]
    #[test]
    fn real_webp_mutations_and_truncations_never_panic(which in 0usize..5, pos in any::<usize>(), byte in any::<u8>(), truncate in any::<bool>()) {
        let source = [
            include_bytes!("fixtures/photo-transforms.webp").as_slice(),
            include_bytes!("fixtures/graphic-lossless.webp").as_slice(),
            include_bytes!("fixtures/smooth-alpha.webp").as_slice(),
            include_bytes!("fixtures/sticker-alpha.webp").as_slice(),
            include_bytes!("fixtures/animated-alpha.webp").as_slice(),
        ][which];
        let mut bytes = source.to_vec();
        let pos = pos % bytes.len();
        if truncate { bytes.truncate(pos); } else { bytes[pos] = byte; }
        if let Ok(out) = ingest(&bytes, None, None, &Limits::production()) {
            prop_assert_eq!(ingest(&bytes, None, None, &Limits::production()).unwrap().sha256, out.sha256);
        }
    }
}

#[test]
fn mixed_webp_animation_uses_first_frame_codec() {
    let lossless = include_bytes!("fixtures/photo-lossless.webp");
    let mut mixed = include_bytes!("fixtures/animated.webp").to_vec();
    let mut pos = 12;
    loop {
        let n = u32::from_le_bytes(mixed[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if &mixed[pos..pos + 4] == b"ANMF" {
            let mut frame = mixed[pos + 8..pos + 24].to_vec();
            frame.extend_from_slice(&lossless[12..]);
            mixed.splice(pos..pos + 8 + n + (n & 1), webp_chunk(b"ANMF", &frame));
            break;
        }
        pos += 8 + n + (n & 1);
    }
    let size = (mixed.len() - 8) as u32;
    mixed[4..8].copy_from_slice(&size.to_le_bytes());
    let out = ingest(&mixed, None, None, &Limits::production()).unwrap();
    assert_eq!(out.content_type, "image/webp");
    assert_eq!(
        image::load_from_memory(&out.bytes).unwrap().to_rgb8(),
        image::load_from_memory(lossless).unwrap().to_rgb8()
    );
}

#[test]
fn main_meta_huffman_after_nonconstant_palette_is_rejected() {
    let mut stream = Vec::new();
    let mut bit_count = 0;
    let mut put = |value: u32, n: usize| {
        for i in 0..n {
            if bit_count % 8 == 0 {
                stream.push(0);
            }
            *stream.last_mut().unwrap() |= (((value >> i) & 1) as u8) << (bit_count % 8);
            bit_count += 1;
        }
    };
    put(1, 1);
    put(3, 2);
    put(1, 8); // colour-index transform, two-entry palette
    put(0, 1); // auxiliary cache absent (no meta bit in auxiliaries)
    // Green single 0; red two symbols 0/255; blue single 0; alpha single 255;
    // distance single 0. Two nonconstant palette pixels consume two red bits.
    for (two, symbol, second) in [
        (false, 0, 0),
        (true, 0, 255),
        (false, 0, 0),
        (false, 255, 0),
        (false, 0, 0),
    ] {
        put(1, 1);
        put(u32::from(two), 1);
        put(u32::from(symbol > 1), 1);
        put(symbol, if symbol > 1 { 8 } else { 1 });
        if two {
            put(second, 8);
        }
    }
    put(0, 1);
    put(1, 1); // two auxiliary literals
    put(0, 1);
    put(0, 1);
    put(1, 1); // no more transforms; no cache; forbidden MAIN meta
    let mut vp8l = vec![0x2f, 0, 0, 0, 0];
    vp8l.extend(stream);
    let mut bytes = b"RIFF\0\0\0\0WEBP".to_vec();
    bytes.extend(webp_chunk(b"VP8L", &vp8l));
    let size = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&size.to_le_bytes());
    assert_eq!(
        ingest(&bytes, None, None, &Limits::production()),
        Err(IngestError::UnsupportedType)
    );
}
