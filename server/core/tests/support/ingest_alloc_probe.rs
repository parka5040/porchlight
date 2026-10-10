//! Isolated test binary: one test, no parallel peak contamination. Input construction is
//! outside the measured interval; peak includes every live ingest allocation and its result.
use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbImage, RgbaImage};
use porchlight_core::ingest::{Limits, image_admission_bytes, ingest};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn added(n: usize) {
    let live = LIVE.fetch_add(n, SeqCst) + n;
    PEAK.fetch_max(live, SeqCst);
}
// SAFETY: all allocation operations delegate the original pointer/Layout unchanged to
// System, obeying GlobalAlloc's caller contract. Only successful live allocations are
// counted; atomics never access the allocated memory or alter allocator behavior.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            added(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            added(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        LIVE.fetch_sub(layout.size(), SeqCst);
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let new = unsafe { System.realloc(p, layout, size) };
        if !new.is_null() {
            if size >= layout.size() {
                added(size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - size, SeqCst);
            }
        }
        new
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for b in bytes {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
fn rotate_png(bytes: &mut Vec<u8>) {
    // TIFF orientation 6, no other metadata.
    let tiff = b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut chunk = (tiff.len() as u32).to_be_bytes().to_vec();
    chunk.extend_from_slice(b"eXIf");
    chunk.extend_from_slice(tiff);
    chunk.extend_from_slice(&crc32(&chunk[4..]).to_be_bytes());
    bytes.splice(33..33, chunk);
}

pub(super) fn measured_peaks_and_camera_admission() {
    let limits = Limits::production();
    for (name, w, h, format, channels, rotated) in [
        ("16 Mpx JPEG 2.7MB", 4608, 3456, ImageFormat::Jpeg, 3, false),
        ("24.5 Mpx JPEG", 6048, 4056, ImageFormat::Jpeg, 3, false),
        (
            "24.5 Mpx progressive JPEG",
            6048,
            4056,
            ImageFormat::Jpeg,
            3,
            false,
        ),
        ("12 Mpx RGB PNG", 4000, 3000, ImageFormat::Png, 3, false),
        ("12 Mpx RGBA PNG", 4000, 3000, ImageFormat::Png, 4, false),
        (
            "25 Mpx rotated RGBA16 PNG",
            5000,
            5000,
            ImageFormat::Png,
            8,
            true,
        ),
        ("25 Mpx RGBA WebP", 5000, 5000, ImageFormat::WebP, 4, false),
        ("25 Mpx RGB WebP", 5000, 5000, ImageFormat::WebP, 3, false),
        ("25 Mpx GIF", 5000, 5000, ImageFormat::Gif, 4, false),
        (
            "3 Mpx detailed rotated RGBA16 PNG",
            2000,
            1500,
            ImageFormat::Png,
            8,
            true,
        ),
    ] {
        let patterned = |x: u32, y: u32| -> u8 {
            if format == ImageFormat::Gif {
                255
            } else {
                ((x / 16 * 13 + y / 16 * 29) % 256) as u8
            }
        };
        let noise = |x: u32, y: u32| -> u8 {
            let mut n = x.wrapping_add(y.wrapping_mul(w));
            n ^= n >> 16;
            n = n.wrapping_mul(0x7feb352d);
            n ^= n >> 15;
            n = n.wrapping_mul(0x846ca68b);
            (n ^ (n >> 16)) as u8
        };
        let pixels = match channels {
            3 => DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
                image::Rgb([
                    if format == ImageFormat::Png {
                        noise(x, y)
                    } else {
                        patterned(x, y)
                    },
                    patterned(y, x),
                    77,
                ])
            })),
            4 => DynamicImage::ImageRgba8(RgbaImage::from_fn(w, h, |x, y| {
                image::Rgba([
                    if format == ImageFormat::Png {
                        noise(x, y)
                    } else {
                        patterned(x, y)
                    },
                    patterned(y, x),
                    77,
                    patterned(x + 7, y + 7),
                ])
            })),
            _ => DynamicImage::ImageRgba16(image::ImageBuffer::from_fn(w, h, |x, y| {
                if w == 5000 {
                    image::Rgba([12345u16, 23456, 34567, 45678])
                } else {
                    image::Rgba([x as u16 * 23, y as u16 * 37, (x + y) as u16 * 11, 45678])
                }
            })),
        };
        let mut output = Cursor::new(Vec::new());
        pixels.write_to(&mut output, format).unwrap();
        drop(pixels);
        let mut input = output.into_inner();
        if name.contains("progressive") {
            input = include_bytes!("../fixtures/progressive-camera.jpg").to_vec();
        }
        if rotated {
            rotate_png(&mut input);
        }
        if name.starts_with("16 Mpx") {
            input.resize(input.len().max(2_700_000), b'A');
        }
        assert!(
            input.len() <= limits.max_bytes,
            "{name}: fixture {}",
            input.len()
        );
        let decoded = ImageReader::with_format(Cursor::new(&input), format)
            .into_decoder()
            .unwrap()
            .total_bytes();
        let baseline = LIVE.load(SeqCst);
        PEAK.store(baseline, SeqCst);
        let result = ingest(&input, None, None, &limits);
        let peak = PEAK.load(SeqCst).saturating_sub(baseline);
        let budget = image_admission_bytes(format, decoded, input.len(), limits.max_bytes).unwrap();
        let allowance = image_admission_bytes(format, 0, input.len(), limits.max_bytes).unwrap();
        println!(
            "{name}: input={} decoded={decoded} peak={peak} factor={:.3} admission={budget} result={}",
            input.len(),
            peak as f64 / decoded as f64,
            if result.is_ok() {
                "accepted"
            } else {
                "rejected"
            }
        );
        println!(
            "  residual factor after input/output/scratch allowances: {:.3}",
            (peak as u64).saturating_sub(allowance) as f64 / decoded as f64
        );
        let out = result.unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            peak as u64 <= budget,
            "{name}: measured peak exceeds admission estimate"
        );
        assert!(
            peak as u64 <= limits.max_alloc_bytes,
            "{name}: measured peak exceeds production budget"
        );
        assert!(
            budget <= limits.max_alloc_bytes,
            "{name}: admission must fit production budget"
        );
        if rotated {
            assert_eq!((out.width, out.height), (Some(h), Some(w)));
        }
        drop(out);
    }
}
