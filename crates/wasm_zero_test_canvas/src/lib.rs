//! Heavy per-pixel compute on the Rust (wasm) side, transferred to JS as a
//! zero-copy RGBA pixel buffer via `wasm_zero`, then blitted to a `<canvas>`.
//!
//! Each frame JS calls [`render`] with an orbiting Julia constant `c = (cx, cy)`
//! (JS does the cheap `cos`/`sin`; Rust does the expensive part). For every one
//! of the `WIDTH * HEIGHT` pixels we iterate `z = z² + c` up to `MAX_ITER`
//! times — that's the GPU-style data-parallel workload, just run on the CPU in
//! wasm. The result is an `RGBA8` `Vec<u8>` returned through `wasm_zero`, which
//! the bindings expose to JS as a `Uint8Array` view aliasing wasm memory (no
//! copy on the read side). Finally we stamp "HELLO WORLD" over the fractal.
//!
//! Everything here is `#![no_std]` and uses only `core` f32 arithmetic — no
//! `libm`, no trig — so the binary stays tiny and dependency-free.

#![no_std]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

use wasm_zero::wasm_zero;

/// Frame dimensions. `WIDTH * HEIGHT * 4` (RGBA) must fit in
/// `wasm_zero::mem::MAX_BUFFER_SIZE` — 320×240×4 = 307_200 B ≤ 512 KiB.
const WIDTH: usize = 320;
const HEIGHT: usize = 240;

/// Julia escape-iteration cap. Higher = sharper detail and heavier compute
/// (WIDTH * HEIGHT * MAX_ITER ≈ 9.8M iterations per frame at 128).
const MAX_ITER: u32 = 128;

/// Canvas width in pixels (scalar fast-path export — JS reads it to size the
/// canvas, no buffer/rkyv involved).
#[wasm_zero]
pub fn width() -> u32 {
    WIDTH as u32
}

/// Canvas height in pixels.
#[wasm_zero]
pub fn height() -> u32 {
    HEIGHT as u32
}

/// Render one animated frame.
///
/// `cx`/`cy` are the real/imaginary parts of the Julia constant for this frame.
/// Returns a tightly-packed `WIDTH * HEIGHT` RGBA8 buffer. Both args are
/// scalars (passed as direct wasm params); the `Vec<u8>` return comes back to
/// JS as a zero-copy `Uint8Array` view over wasm memory.
#[wasm_zero]
pub fn render(cx: f32, cy: f32) -> Vec<u8> {
    let mut buf = vec![0u8; WIDTH * HEIGHT * 4];

    // Map pixels into the complex plane: ~3 units tall, centred on the origin.
    let scale = 3.0 / HEIGHT as f32;
    let half_w = WIDTH as f32 * 0.5;
    let half_h = HEIGHT as f32 * 0.5;

    for py in 0..HEIGHT {
        let fy = (py as f32 - half_h) * scale;
        for px in 0..WIDTH {
            let fx = (px as f32 - half_w) * scale;

            // z_{n+1} = z_n² + c, escaping when |z|² > 4.
            let mut zx = fx;
            let mut zy = fy;
            let mut i = 0u32;
            while i < MAX_ITER {
                let zx2 = zx * zx;
                let zy2 = zy * zy;
                if zx2 + zy2 > 4.0 {
                    break;
                }
                zy = 2.0 * zx * zy + cy;
                zx = zx2 - zy2 + cx;
                i += 1;
            }

            let (r, g, b) = palette(i);
            let o = (py * WIDTH + px) * 4;
            buf[o] = r;
            buf[o + 1] = g;
            buf[o + 2] = b;
            buf[o + 3] = 255;
        }
    }

    draw_banner(&mut buf, b"HELLO WORLD");
    buf
}

/// Smooth polynomial palette (no trig): bright bands outside the set, black
/// inside (where `t == 1`, every `(1 - t)` term vanishes).
fn palette(iter: u32) -> (u8, u8, u8) {
    let t = iter as f32 / MAX_ITER as f32;
    let inv = 1.0 - t;
    let r = 9.0 * inv * t * t * t * 255.0;
    let g = 15.0 * inv * inv * t * t * 255.0;
    let b = 8.5 * inv * inv * inv * t * 255.0;
    (to_u8(r), to_u8(g), to_u8(b))
}

fn to_u8(v: f32) -> u8 {
    v.clamp(0.0, 255.0) as u8
}

// ---------------------------------------------------------------------------
// Bitmap text overlay
// ---------------------------------------------------------------------------

/// Integer scale-up factor for the 8×8 glyphs.
const GLYPH_SCALE: usize = 3;

/// Stamp `text` centred over the frame: a darkened plate for contrast, then
/// the glyphs in white.
fn draw_banner(buf: &mut [u8], text: &[u8]) {
    let glyph_w = 8 * GLYPH_SCALE;
    let text_w = text.len() * glyph_w;
    let text_h = 8 * GLYPH_SCALE;
    let x0 = (WIDTH - text_w) / 2;
    let y0 = (HEIGHT - text_h) / 2;

    // Darken a padded plate behind the text so white glyphs stay legible over
    // the bright fractal.
    let pad = GLYPH_SCALE * 2;
    for y in y0.saturating_sub(pad)..(y0 + text_h + pad).min(HEIGHT) {
        for x in x0.saturating_sub(pad)..(x0 + text_w + pad).min(WIDTH) {
            let o = (y * WIDTH + x) * 4;
            buf[o] /= 4;
            buf[o + 1] /= 4;
            buf[o + 2] /= 4;
        }
    }

    // Stamp each glyph in white.
    for (gi, &ch) in text.iter().enumerate() {
        let rows = glyph(ch);
        let gx = x0 + gi * glyph_w;
        for (ry, &bits) in rows.iter().enumerate() {
            for rx in 0..8 {
                if bits & (0x80 >> rx) == 0 {
                    continue;
                }
                // Scale the bit up to a GLYPH_SCALE×GLYPH_SCALE block.
                for dy in 0..GLYPH_SCALE {
                    for dx in 0..GLYPH_SCALE {
                        let x = gx + rx * GLYPH_SCALE + dx;
                        let y = y0 + ry * GLYPH_SCALE + dy;
                        let o = (y * WIDTH + x) * 4;
                        buf[o] = 255;
                        buf[o + 1] = 255;
                        buf[o + 2] = 255;
                    }
                }
            }
        }
    }
}

/// 8×8 bitmap for the glyphs used by "HELLO WORLD" (MSB = leftmost column).
fn glyph(ch: u8) -> [u8; 8] {
    match ch {
        b'H' => [0xC6, 0xC6, 0xC6, 0xFE, 0xC6, 0xC6, 0xC6, 0x00],
        b'E' => [0xFE, 0xC0, 0xC0, 0xFC, 0xC0, 0xC0, 0xFE, 0x00],
        b'L' => [0xC0, 0xC0, 0xC0, 0xC0, 0xC0, 0xC0, 0xFE, 0x00],
        b'O' => [0x7C, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x7C, 0x00],
        b'W' => [0xC6, 0xC6, 0xC6, 0xD6, 0xD6, 0xFE, 0x6C, 0x00],
        b'R' => [0xFC, 0xC6, 0xC6, 0xFC, 0xD8, 0xCC, 0xC6, 0x00],
        b'D' => [0xF8, 0xCC, 0xC6, 0xC6, 0xC6, 0xCC, 0xF8, 0x00],
        _ => [0x00; 8], // space / unknown
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
