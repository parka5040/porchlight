//! Differential checks use image's adapter to the pinned image-webp decoder, without new crates.
use super::*;

#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    pos: usize,
}
impl Bits {
    fn put(&mut self, value: u32, n: usize) {
        for i in 0..n {
            if self.pos.is_multiple_of(8) {
                self.bytes.push(0);
            }
            *self.bytes.last_mut().unwrap() |= (((value >> i) & 1) as u8) << (self.pos % 8);
            self.pos += 1;
        }
    }
    fn simple(&mut self, first: u32, second: Option<u32>, wide: bool) {
        self.put(1, 1);
        self.put(u32::from(second.is_some()), 1);
        self.put(u32::from(wide), 1);
        self.put(first, if wide { 8 } else { 1 });
        if let Some(second) = second {
            self.put(second, 8);
        }
    }
    // A normal canonical tree whose green symbols cannot be expressed by a simple code.
    fn green(&mut self, symbols: &[usize], alphabet: usize) {
        self.put(0, 1);
        self.put(0, 4); // four code-length codes: 17,18,0,1
        for n in [0, 0, 1, 1] {
            self.put(n, 3);
        }
        self.put(0, 1); // full alphabet
        for i in 0..alphabet {
            self.put(u32::from(symbols.contains(&i)), 1);
        }
    }
    fn constant_group(&mut self) {
        for value in [0, 0, 0, 255, 0] {
            self.simple(value, None, value > 1);
        }
    }
}
fn chunk(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = tag.to_vec();
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if !data.len().is_multiple_of(2) {
        out.push(0);
    }
    out
}
fn riff(chunks: &[u8]) -> Vec<u8> {
    let mut out = b"RIFF\0\0\0\0WEBP".to_vec();
    out.extend_from_slice(chunks);
    let size = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&size.to_le_bytes());
    out
}
fn vp8l(w: u32, h: u32, bits: &Bits) -> Vec<u8> {
    let mut out = vec![0x2f];
    out.extend_from_slice(&((w - 1) | ((h - 1) << 14)).to_le_bytes());
    out.extend_from_slice(&bits.bytes);
    out
}
fn finish(bits: &mut Bits, meta: bool) {
    bits.put(0, 1); // no more transforms
    bits.put(0, 1); // main cache absent
    bits.put(u32::from(meta), 1);
    if meta {
        bits.put(0, 3); // 4x4 meta blocks; every block names only group zero
        bits.put(0, 1); // entropy cache absent
        bits.constant_group();
    }
    bits.constant_group();
    // Decoder lookahead needs padding, not additional semantic bits.
    bits.bytes.extend_from_slice(&[0; 16]);
}

#[test]
fn webp_simple_code_shapes_match_pinned_decoder_meta_position() {
    // Both encodings of narrow symbols, single/two, ascending/descending/equal,
    // and both branch bits, in every tree. Equal/descending may fail closed.
    let shapes = [
        (0, None, false),
        (1, None, false),
        (0, None, true),
        (1, None, true),
        (7, None, true),
        (0, Some(1), false),
        (1, Some(7), false),
        (7, Some(9), true),
        (0, Some(7), true),
        (1, Some(0), false),
        (9, Some(7), true),
        (0, Some(0), false),
        (7, Some(7), true),
    ];
    for slot in 0..5 {
        for (first, second, wide) in shapes {
            for meta in [false, true] {
                let mut bits = Bits::default();
                bits.put(1, 1);
                bits.put(1, 2); // colour transform, 8x4 -> two auxiliary pixels
                bits.put(0, 3);
                bits.put(0, 1); // aux cache absent
                for tree in 0..5 {
                    if tree == slot {
                        bits.simple(first, second, wide);
                    } else {
                        bits.simple(0, None, false);
                    }
                }
                if second.is_some() && slot < 4 {
                    bits.put(0, 1);
                    bits.put(1, 1);
                }
                finish(&mut bits, meta);
                let data = vp8l(8, 4, &bits);
                let guard = preflight_webp_entropy(b"VP8L", &data, (8, 4));
                if second.is_some_and(|s| s < first) {
                    assert!(guard.is_err(), "slot={slot} {first:?}/{second:?}");
                    continue;
                }
                assert_eq!(
                    guard,
                    if meta {
                        Err(IngestError::UnsupportedType)
                    } else {
                        Ok(())
                    }
                );
                let bytes = riff(&chunk(b"VP8L", &data));
                // Both clear and set meta-bit streams decode. Set uses exactly one group,
                // so this test cannot itself trigger the unbounded meta allocation.
                let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).unwrap();
                let mut pixels = vec![0; decoder.total_bytes() as usize];
                decoder.read_image(&mut pixels).unwrap();
            }
        }
    }
}

#[test]
fn webp_descending_auxiliary_distance_code_is_unsupported() {
    let mut bits = Bits::default();
    bits.put(1, 1);
    bits.put(1, 2); // colour transform, 8x4 -> two pixels
    bits.put(0, 3);
    bits.put(0, 1);
    bits.green(&[0, 256], 280); // literal then length-one reference
    for _ in 0..3 {
        bits.simple(0, None, false);
    }
    bits.simple(4, Some(1), true); // different extra-bit counts, descending!
    bits.put(0, 1); // first literal
    bits.put(1, 1); // reference
    bits.put(1, 1); // decoder chooses distance symbol 1 (no extra bits)
    finish(&mut bits, true);
    assert_eq!(
        preflight_webp_entropy(b"VP8L", &vp8l(8, 4, &bits), (8, 4)),
        Err(IngestError::UnsupportedType)
    );
}

#[test]
fn webp_distance_shapes_match_decoder_after_references() {
    // Sixteen auxiliary pixels allow the small distance prefixes (including extra bits).
    for (first, second, wide) in [
        (0, None, false),
        (1, None, false),
        (4, None, true),
        (0, Some(1), false),
        (1, Some(4), false),
        (4, Some(5), true),
        (4, Some(1), true),
        (1, Some(1), false),
    ] {
        for branch in [0, 1] {
            for meta in [false, true] {
                let mut bits = Bits::default();
                bits.put(1, 1);
                bits.put(1, 2);
                bits.put(0, 3);
                bits.put(0, 1);
                bits.green(&[0, 256], 280);
                for _ in 0..3 {
                    bits.simple(0, None, false);
                }
                bits.simple(first, second, wide);
                for _ in 0..15 {
                    bits.put(0, 1);
                }
                bits.put(1, 1); // last pixel is length-one reference
                let symbol = if let Some(second) = second {
                    bits.put(branch, 1);
                    if branch == 0 { first } else { second }
                } else {
                    first
                };
                if symbol >= 4 {
                    bits.put(0, ((symbol - 2) >> 1) as usize);
                }
                finish(&mut bits, meta);
                let data = vp8l(16, 16, &bits);
                let guard = preflight_webp_entropy(b"VP8L", &data, (16, 16));
                if second.is_some_and(|s| s < first) {
                    assert!(guard.is_err());
                    continue;
                }
                assert_eq!(
                    guard,
                    if meta {
                        Err(IngestError::UnsupportedType)
                    } else {
                        Ok(())
                    }
                );
                let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(riff(&chunk(
                    b"VP8L", &data,
                ))))
                .unwrap();
                let mut pixels = vec![0; decoder.total_bytes() as usize];
                decoder.read_image(&mut pixels).unwrap();
            }
        }
    }
}

#[test]
fn webp_duplicates_are_rejected_before_entropy_walk() {
    let data = [0x2f, 0, 0, 0, 0, 4]; // forbidden meta, but duplicates take precedence
    let mut chunks = chunk(b"VP8L", &data);
    chunks.extend(chunk(b"VP8L", &data));
    assert_eq!(
        prepare_webp(&riff(&chunks), &Limits::production()),
        Err(IngestError::Invalid)
    );
    let mut frame = vec![0; 16];
    frame.extend_from_slice(&chunks);
    let mut chunks = chunk(b"VP8X", &[2, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    chunks.extend(chunk(b"ANMF", &frame));
    assert_eq!(
        prepare_webp(&riff(&chunks), &Limits::production()),
        Err(IngestError::Invalid)
    );
}

#[test]
fn webp_guard_walks_only_first_animation_frame() {
    let mut chunks = chunk(b"VP8X", &[2, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    for meta in [0, 4] {
        let mut frame = vec![0; 16];
        frame.extend(chunk(b"VP8L", &[0x2f, 0, 0, 0, 0, meta]));
        chunks.extend(chunk(b"ANMF", &frame));
    }
    assert!(prepare_webp(&riff(&chunks), &Limits::production()).is_ok());
}

#[test]
fn webp_repeated_zero_bit_chunks_do_no_pixel_loop_work() {
    let mut bits = Bits::default();
    bits.put(1, 1);
    bits.put(1, 2);
    bits.put(0, 3);
    bits.put(0, 1);
    bits.green(&[256], 280); // zero-bit backward references: invalid, but old guard loops
    for _ in 0..4 {
        bits.simple(0, None, false);
    }
    finish(&mut bits, false);
    let repeated = chunk(b"VP8L", &vp8l(5000, 5000, &bits));
    let mut chunks = Vec::with_capacity(25 * 1024 * 1024);
    while chunks.len() + repeated.len() <= 25 * 1024 * 1024 - 12 {
        chunks.extend_from_slice(&repeated);
    }
    let bytes = riff(&chunks);
    let (send, recv) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        WEBP_AUXILIARY_STEPS.with(|steps| steps.set(0));
        WEBP_CHUNK_STEPS.with(|steps| steps.set(0));
        let result = prepare_webp(&bytes, &Limits::production());
        let steps = WEBP_AUXILIARY_STEPS.with(|steps| steps.get());
        let chunks = WEBP_CHUNK_STEPS.with(|steps| steps.get());
        send.send((result, steps, chunks)).ok();
    });
    let (result, steps, chunks) = recv
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("guard failed to terminate");
    assert_eq!(result, Err(IngestError::Invalid));
    assert_eq!(steps, 0, "zero-bit references must not loop over pixels");
    assert_eq!(
        chunks, 2,
        "duplicate images must reject after exactly two chunks"
    );
}

#[test]
fn webp_zero_bit_paths_and_total_auxiliary_pixel_budget() {
    for green in [0, 256, 259, 280] {
        let mut encoded = Bits::default();
        if green >= 280 {
            encoded.put(1, 1);
            encoded.put(1, 4);
        } else {
            encoded.put(0, 1);
        }
        if green < 256 {
            encoded.simple(green, None, false);
        } else {
            encoded.green(&[green as usize], if green >= 280 { 282 } else { 280 });
        }
        for _ in 0..4 {
            encoded.simple(0, None, false);
        }
        let end = encoded.pos;
        let mut bits = LsbBits {
            bytes: &encoded.bytes,
            pos: 0,
        };
        let mut remaining = 25_000_000;
        WEBP_AUXILIARY_STEPS.with(|steps| steps.set(0));
        let result = skip_webp_auxiliary(&mut bits, 5000, 5000, &mut remaining);
        assert_eq!(WEBP_AUXILIARY_STEPS.with(|steps| steps.get()), 0);
        assert_eq!(
            result,
            if (256..260).contains(&green) {
                Err(IngestError::Invalid)
            } else {
                Ok(())
            }
        );
        assert_eq!(bits.pos, end);
        assert_eq!(remaining, 0);
        // Charge the total, including zero-bit paths, before parsing the next image.
        bits.pos = 0;
        assert_eq!(
            skip_webp_auxiliary(&mut bits, 1, 1, &mut remaining),
            Err(IngestError::Invalid)
        );
        assert_eq!(bits.pos, 0);
    }
}

#[test]
fn admission_estimate_is_checked_and_shared() {
    assert_eq!(
        image_admission_bytes(ImageFormat::Png, 10, 20, 30),
        Ok(1024 * 1024 + 120)
    );
    assert_eq!(
        image_admission_bytes(ImageFormat::WebP, 10, 20, 30),
        Ok(1024 * 1024 + 140)
    );
    assert_eq!(
        image_admission_bytes(ImageFormat::Jpeg, u64::MAX, 0, 0),
        Err(IngestError::TooLarge)
    );
    assert_eq!(
        image_admission_bytes(ImageFormat::Png, u64::MAX / 2, 1, 0),
        Err(IngestError::TooLarge)
    );
}

#[test]
fn webp_many_animation_frames_do_no_pixel_loop_work() {
    let mut bits = Bits::default();
    bits.put(1, 1);
    bits.put(1, 2);
    bits.put(0, 3);
    bits.put(1, 1);
    bits.put(1, 4); // auxiliary cache: two entries
    bits.green(&[280], 282); // zero-bit cache references for 1.56M auxiliary pixels
    for _ in 0..4 {
        bits.simple(0, None, false);
    }
    finish(&mut bits, false);
    let mut frame = vec![0; 16];
    frame[6..9].copy_from_slice(&4999u32.to_le_bytes()[..3]);
    frame[9..12].copy_from_slice(&4999u32.to_le_bytes()[..3]);
    frame.extend(chunk(b"VP8L", &vp8l(5000, 5000, &bits)));
    let repeated = chunk(b"ANMF", &frame);
    let mut canvas = vec![2, 0, 0, 0];
    canvas.extend_from_slice(&4999u32.to_le_bytes()[..3]);
    canvas.extend_from_slice(&4999u32.to_le_bytes()[..3]);
    let mut chunks = chunk(b"VP8X", &canvas);
    let mut frame_count = 0u64;
    while chunks.len() + repeated.len() <= 25 * 1024 * 1024 - 12 {
        chunks.extend_from_slice(&repeated);
        frame_count += 1;
    }
    let bytes = riff(&chunks);
    let (send, recv) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        WEBP_AUXILIARY_STEPS.with(|steps| steps.set(0));
        WEBP_CHUNK_STEPS.with(|steps| steps.set(0));
        let result = prepare_webp(&bytes, &Limits::production());
        let steps = WEBP_AUXILIARY_STEPS.with(|steps| steps.get());
        let chunks = WEBP_CHUNK_STEPS.with(|steps| steps.get());
        send.send((result.is_ok(), steps, chunks)).ok();
    });
    let (ok, steps, chunks) = recv
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("animation guard failed to terminate");
    assert!(ok);
    assert_eq!(steps, 0, "zero-bit cache frames must not loop over pixels");
    assert_eq!(
        chunks,
        3 * frame_count + 2,
        "two outer passes and one nested pass per frame"
    );
}
