# Ingest regression fixtures

Generated offline using Pillow 12.3.0 / its bundled libwebp; synthetic pixels only,
no uploaded/private photographs or metadata. Tests embed these bytes and do not
require Python or libwebp executables at runtime.

- `photo-lossless.webp`: 64×48 RGB; pixel `(x*17+y*37, x*x+y*13, x*3+y*y)` modulo 256; lossless, method 0.
- `photo-transforms.webp`: the same decoded pixels, lossless, method 6 (predictor/colour transforms).
- `photo-lossy.webp`: those RGB pixels, WebP quality 80.
- `graphic-lossless.webp`: 64×64 checkerboard of red/blue 8×8 blocks, lossless default method (palette transform).
- `smooth-alpha.webp`: the RGB photo above with alpha `x*4`, WebP quality 80 (compressed ALPH).
- `sticker-alpha.webp`: checkerboard with alpha 255 inside `8<x<56 && 8<y<56`, otherwise 0; quality 80.
- `animated.webp`: RGB photo and constant `(20,40,60)` RGB frame, 64×48; duration 100ms, loop 0, quality 80.
- `animated-alpha.webp`: decoded smooth-alpha fixture and constant `(20,40,60,128)` frame, 64×48; duration 100ms, loop 0, quality 80.
- `progressive-camera.jpg`: 6048×4056 constant `(30,80,130)` RGB; JPEG q90, progressive=True, subsampling=0. This exercises progressive coefficient scratch allocation independently of entropy complexity.

Large PNG/WebP/GIF/baseline JPEG allocation fixtures are constructed in Rust by
`tests/support/ingest_alloc_probe.rs`, outside its measured ingest interval. The
16 Mpx JPEG is padded after EOI to 2.7 MB to exercise admission independently of
camera entropy complexity. The 12 Mpx RGB/RGBA PNGs have a deterministic noisy
channel and are 22.4/25.5 MB (larger than the review's 11/14 MB examples).
