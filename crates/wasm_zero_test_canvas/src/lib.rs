//! Interactive immediate-mode UI rendered entirely in Rust (wasm), shipped to
//! JS as a zero-copy RGBA buffer via `wasm_zero`, and blitted to a `<canvas>`.
//!
//! The whole point: **Rust never touches the DOM.** JS owns the single canvas
//! element and its event listeners, and pumps the current mouse state *into*
//! [`render`] as plain scalar arguments each frame (the wasm_zero direct-param
//! fast path — no buffers, no rkyv, no JS→Rust imports). Rust does all the
//! layout, hit-testing, and drawing, then returns the finished frame.
//!
//! UI state (paused?, click count, last Julia constant) is *retained* in wasm
//! linear memory between calls — that's how immediate-mode widgets get hover,
//! press, and toggle behavior without any DOM state.
//!
//! Layout: a left sidebar (panel + a PAUSE/PLAY button + status + click count)
//! and a fractal viewport on the right. When playing, the Julia constant orbits
//! with time (JS supplies the cheap cos/sin). When paused, moving the mouse over
//! the viewport drives the constant live — drag to explore the set.

#![no_std]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use core::ptr::addr_of_mut;

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

use wasm_zero::wasm_zero;

// --- frame geometry (WIDTH*HEIGHT*4 must fit in MAX_BUFFER_SIZE) ----------
const WIDTH: usize = 320;
const HEIGHT: usize = 240;

const SIDEBAR_W: usize = 96;
const VIEW_X0: usize = SIDEBAR_W;
const VIEW_W: usize = WIDTH - SIDEBAR_W;

// --- button geometry (in the sidebar) -------------------------------------
const BTN_X: usize = 12;
const BTN_Y: usize = 64;
const BTN_W: usize = 72;
const BTN_H: usize = 30;

const MAX_ITER: u32 = 128;

type Rgb = (u8, u8, u8);

// ---------------------------------------------------------------------------
// Retained UI state — lives in wasm memory across `render` calls.
// ---------------------------------------------------------------------------

struct Ui {
    paused: bool,
    clicks: u32,
    prev_down: bool,
    cx: f32,
    cy: f32,
}

impl Ui {
    const fn new() -> Self {
        Ui { paused: false, clicks: 0, prev_down: false, cx: 0.0, cy: 0.0 }
    }
}

static mut UI: Ui = Ui::new();

/// Single-threaded wasm: route all access through one raw-pointer deref so we
/// never form a reference to the `static mut` directly.
fn ui() -> &'static mut Ui {
    unsafe { &mut *addr_of_mut!(UI) }
}

// ---------------------------------------------------------------------------
// Exports
// ---------------------------------------------------------------------------

/// Canvas width — scalar fast-path export (JS reads it to size the canvas).
#[wasm_zero]
pub fn width() -> u32 {
    WIDTH as u32
}

/// Canvas height.
#[wasm_zero]
pub fn height() -> u32 {
    HEIGHT as u32
}

/// Render one interactive frame.
///
/// `mouse_x`/`mouse_y` are canvas-space pixel coords; `mouse_down` is 0/1;
/// `orbit_cx`/`orbit_cy` are the time-animated Julia constant computed by JS
/// (used while playing). All scalars → direct wasm params. Returns a packed
/// `WIDTH*HEIGHT` RGBA8 buffer (a zero-copy `Uint8Array` view on the JS side).
#[wasm_zero]
pub fn render(
    mouse_x: f32,
    mouse_y: f32,
    mouse_down: u32,
    orbit_cx: f32,
    orbit_cy: f32,
) -> Vec<u8> {
    let ui = ui();
    let down = mouse_down != 0;

    // --- input: hit-test the button + detect a click (press edge) ---------
    let over_btn = point_in(mouse_x, mouse_y, BTN_X, BTN_Y, BTN_W, BTN_H);
    if over_btn && down && !ui.prev_down {
        ui.paused = !ui.paused;
        ui.clicks += 1;
    }
    ui.prev_down = down;

    // --- decide the Julia constant for this frame -------------------------
    let over_view = mouse_x >= VIEW_X0 as f32 && mouse_x < WIDTH as f32;
    if !ui.paused {
        ui.cx = orbit_cx;
        ui.cy = orbit_cy;
    } else if over_view {
        // Map the mouse position in the viewport to the constant's plane.
        let nx = (mouse_x - VIEW_X0 as f32) / VIEW_W as f32;
        let ny = mouse_y / HEIGHT as f32;
        ui.cx = (nx * 2.0 - 1.0) * 0.8;
        ui.cy = (ny * 2.0 - 1.0) * 0.8;
    }
    let (cx, cy) = (ui.cx, ui.cy);

    // --- draw -------------------------------------------------------------
    let mut buf = vec![0u8; WIDTH * HEIGHT * 4];
    render_fractal(&mut buf, cx, cy);
    draw_banner(&mut buf, b"HELLO WORLD");

    // Paused: a crosshair shows the sampled point.
    if ui.paused && over_view && mouse_y >= 0.0 && mouse_y < HEIGHT as f32 {
        let mx = mouse_x as usize;
        let my = mouse_y as usize;
        fill_rect(&mut buf, mx.saturating_sub(4), my, 9, 1, (255, 255, 255));
        fill_rect(&mut buf, mx, my.saturating_sub(4), 1, 9, (255, 255, 255));
    }

    draw_sidebar(&mut buf, ui, over_btn, down);
    buf
}

// ---------------------------------------------------------------------------
// Fractal viewport
// ---------------------------------------------------------------------------

fn render_fractal(buf: &mut [u8], cx: f32, cy: f32) {
    let scale = 3.0 / HEIGHT as f32;
    let half_w = VIEW_W as f32 * 0.5;
    let half_h = HEIGHT as f32 * 0.5;

    for py in 0..HEIGHT {
        let fy = (py as f32 - half_h) * scale;
        for px in VIEW_X0..WIDTH {
            let fx = ((px - VIEW_X0) as f32 - half_w) * scale;

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
}

/// Smooth polynomial palette (no trig): bright bands outside, black inside.
fn palette(iter: u32) -> Rgb {
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
// Sidebar UI
// ---------------------------------------------------------------------------

fn draw_sidebar(buf: &mut [u8], ui: &Ui, over_btn: bool, down: bool) {
    // Panel + right divider.
    fill_rect(buf, 0, 0, SIDEBAR_W, HEIGHT, (24, 26, 32));
    fill_rect(buf, SIDEBAR_W - 2, 0, 2, HEIGHT, (52, 56, 70));

    // Title.
    draw_text(buf, 11, 12, 1, b"WASM UI", (205, 215, 235));
    fill_rect(buf, 11, 28, 60, 1, (52, 56, 70));

    // Button: background reflects hover / press state.
    let bg = if over_btn && down {
        (58, 92, 168)
    } else if over_btn {
        (70, 82, 116)
    } else {
        (44, 50, 66)
    };
    fill_rect(buf, BTN_X, BTN_Y, BTN_W, BTN_H, bg);
    // 1px border.
    fill_rect(buf, BTN_X, BTN_Y, BTN_W, 1, (90, 100, 130));
    fill_rect(buf, BTN_X, BTN_Y + BTN_H - 1, BTN_W, 1, (90, 100, 130));
    fill_rect(buf, BTN_X, BTN_Y, 1, BTN_H, (90, 100, 130));
    fill_rect(buf, BTN_X + BTN_W - 1, BTN_Y, 1, BTN_H, (90, 100, 130));

    let label: &[u8] = if ui.paused { b"PLAY" } else { b"PAUSE" };
    let lw = text_width(label, 1);
    let lx = BTN_X + (BTN_W - lw) / 2;
    let ly = BTN_Y + (BTN_H - 8) / 2;
    draw_text(buf, lx, ly, 1, label, (240, 244, 252));

    // Status line.
    let (status, color): (&[u8], Rgb) = if ui.paused {
        (b"PAUSED", (245, 185, 85))
    } else {
        (b"LIVE", (120, 220, 145))
    };
    draw_text(buf, 12, BTN_Y + BTN_H + 10, 1, status, color);

    // Click counter (retained state, rendered as text).
    draw_text(buf, 12, 132, 1, b"CLICKS", (150, 160, 180));
    let mut digits = [0u8; 10];
    draw_text(buf, 12, 146, 1, fmt_u32(ui.clicks, &mut digits), (220, 226, 238));

    // Hint at the bottom.
    if ui.paused {
        draw_text(buf, 12, HEIGHT - 26, 1, b"MOVE MOUSE", (120, 132, 152));
        draw_text(buf, 12, HEIGHT - 16, 1, b"TO EXPLORE", (120, 132, 152));
    } else {
        draw_text(buf, 12, HEIGHT - 16, 1, b"CLICK PAUSE", (110, 120, 140));
    }
}

// ---------------------------------------------------------------------------
// Drawing primitives
// ---------------------------------------------------------------------------

fn point_in(mx: f32, my: f32, x: usize, y: usize, w: usize, h: usize) -> bool {
    mx >= x as f32
        && mx < (x + w) as f32
        && my >= y as f32
        && my < (y + h) as f32
}

fn fill_rect(buf: &mut [u8], x: usize, y: usize, w: usize, h: usize, c: Rgb) {
    let y1 = (y + h).min(HEIGHT);
    let x1 = (x + w).min(WIDTH);
    for yy in y..y1 {
        for xx in x..x1 {
            let o = (yy * WIDTH + xx) * 4;
            buf[o] = c.0;
            buf[o + 1] = c.1;
            buf[o + 2] = c.2;
            buf[o + 3] = 255;
        }
    }
}

/// Halve the brightness of a rectangle (a cheap translucent plate).
fn darken_rect(buf: &mut [u8], x: usize, y: usize, w: usize, h: usize) {
    let y1 = (y + h).min(HEIGHT);
    let x1 = (x + w).min(WIDTH);
    for yy in y..y1 {
        for xx in x..x1 {
            let o = (yy * WIDTH + xx) * 4;
            buf[o] /= 3;
            buf[o + 1] /= 3;
            buf[o + 2] /= 3;
        }
    }
}

/// Advance per glyph cell, in source pixels (glyphs are 6px wide in an 8px box).
const ADVANCE: usize = 7;

fn text_width(text: &[u8], scale: usize) -> usize {
    text.len() * ADVANCE * scale
}

fn draw_text(buf: &mut [u8], x: usize, y: usize, scale: usize, text: &[u8], c: Rgb) {
    for (i, &ch) in text.iter().enumerate() {
        let rows = glyph(ch);
        let gx = x + i * ADVANCE * scale;
        for (ry, &bits) in rows.iter().enumerate() {
            for rx in 0..8 {
                if bits & (0x80 >> rx) != 0 {
                    fill_rect(buf, gx + rx * scale, y + ry * scale, scale, scale, c);
                }
            }
        }
    }
}

/// Centered "HELLO WORLD" banner over the fractal viewport, on a darkened plate.
fn draw_banner(buf: &mut [u8], text: &[u8]) {
    let scale = 2;
    let tw = text_width(text, scale);
    let th = 8 * scale;
    let x0 = VIEW_X0 + (VIEW_W - tw) / 2;
    let y0 = (HEIGHT - th) / 2;
    let pad = 6;
    darken_rect(
        buf,
        x0.saturating_sub(pad),
        y0.saturating_sub(pad),
        tw + pad * 2,
        th + pad * 2,
    );
    draw_text(buf, x0, y0, scale, text, (255, 255, 255));
}

/// Format a `u32` into `out` (decimal), returning the written slice.
fn fmt_u32(mut n: u32, out: &mut [u8; 10]) -> &[u8] {
    if n == 0 {
        out[0] = b'0';
        return &out[..1];
    }
    let mut i = out.len();
    while n > 0 {
        i -= 1;
        out[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    &out[i..]
}

// ---------------------------------------------------------------------------
// 6×7 bitmap font in an 8×8 cell (MSB = leftmost column). Uppercase + digits.
// ---------------------------------------------------------------------------

fn glyph(ch: u8) -> [u8; 8] {
    match ch {
        b'A' => [0b0111_1000, 0b1000_0100, 0b1000_0100, 0b1111_1100, 0b1000_0100, 0b1000_0100, 0b1000_0100, 0],
        b'C' => [0b0111_1000, 0b1000_0100, 0b1000_0000, 0b1000_0000, 0b1000_0000, 0b1000_0100, 0b0111_1000, 0],
        b'D' => [0b1111_0000, 0b1000_1000, 0b1000_0100, 0b1000_0100, 0b1000_0100, 0b1000_1000, 0b1111_0000, 0],
        b'E' => [0b1111_1100, 0b1000_0000, 0b1000_0000, 0b1111_1000, 0b1000_0000, 0b1000_0000, 0b1111_1100, 0],
        b'G' => [0b0111_1000, 0b1000_0100, 0b1000_0000, 0b1001_1100, 0b1000_0100, 0b1000_0100, 0b0111_1000, 0],
        b'H' => [0b1000_0100, 0b1000_0100, 0b1000_0100, 0b1111_1100, 0b1000_0100, 0b1000_0100, 0b1000_0100, 0],
        b'I' => [0b1111_1100, 0b0011_0000, 0b0011_0000, 0b0011_0000, 0b0011_0000, 0b0011_0000, 0b1111_1100, 0],
        b'K' => [0b1000_0100, 0b1000_1000, 0b1001_0000, 0b1110_0000, 0b1001_0000, 0b1000_1000, 0b1000_0100, 0],
        b'L' => [0b1000_0000, 0b1000_0000, 0b1000_0000, 0b1000_0000, 0b1000_0000, 0b1000_0000, 0b1111_1100, 0],
        b'M' => [0b1000_0100, 0b1100_1100, 0b1011_0100, 0b1000_0100, 0b1000_0100, 0b1000_0100, 0b1000_0100, 0],
        b'O' => [0b0111_1000, 0b1000_0100, 0b1000_0100, 0b1000_0100, 0b1000_0100, 0b1000_0100, 0b0111_1000, 0],
        b'P' => [0b1111_1000, 0b1000_0100, 0b1000_0100, 0b1111_1000, 0b1000_0000, 0b1000_0000, 0b1000_0000, 0],
        b'R' => [0b1111_1000, 0b1000_0100, 0b1000_0100, 0b1111_1000, 0b1001_0000, 0b1000_1000, 0b1000_0100, 0],
        b'S' => [0b0111_1100, 0b1000_0000, 0b1000_0000, 0b0111_1000, 0b0000_0100, 0b0000_0100, 0b1111_1000, 0],
        b'T' => [0b1111_1100, 0b0011_0000, 0b0011_0000, 0b0011_0000, 0b0011_0000, 0b0011_0000, 0b0011_0000, 0],
        b'U' => [0b1000_0100, 0b1000_0100, 0b1000_0100, 0b1000_0100, 0b1000_0100, 0b1000_0100, 0b0111_1000, 0],
        b'V' => [0b1000_0100, 0b1000_0100, 0b1000_0100, 0b0100_1000, 0b0100_1000, 0b0011_0000, 0b0011_0000, 0],
        b'W' => [0b1000_0100, 0b1000_0100, 0b1000_0100, 0b1001_0100, 0b1011_0100, 0b1110_1100, 0b1000_0100, 0],
        b'Y' => [0b1000_0100, 0b1000_0100, 0b0100_1000, 0b0011_0000, 0b0011_0000, 0b0011_0000, 0b0011_0000, 0],
        b'0' => [0b0111_1000, 0b1000_1100, 0b1001_0100, 0b1001_0100, 0b1010_0100, 0b1100_0100, 0b0111_1000, 0],
        b'1' => [0b0011_0000, 0b0111_0000, 0b0011_0000, 0b0011_0000, 0b0011_0000, 0b0011_0000, 0b1111_1100, 0],
        b'2' => [0b0111_1000, 0b1000_0100, 0b0000_0100, 0b0001_1000, 0b0110_0000, 0b1000_0000, 0b1111_1100, 0],
        b'3' => [0b0111_1000, 0b1000_0100, 0b0000_0100, 0b0011_1000, 0b0000_0100, 0b1000_0100, 0b0111_1000, 0],
        b'4' => [0b0000_1000, 0b0001_1000, 0b0010_1000, 0b0100_1000, 0b1111_1100, 0b0000_1000, 0b0000_1000, 0],
        b'5' => [0b1111_1100, 0b1000_0000, 0b1111_1000, 0b0000_0100, 0b0000_0100, 0b1000_0100, 0b0111_1000, 0],
        b'6' => [0b0011_1000, 0b0100_0000, 0b1000_0000, 0b1111_1000, 0b1000_0100, 0b1000_0100, 0b0111_1000, 0],
        b'7' => [0b1111_1100, 0b0000_0100, 0b0000_1000, 0b0001_0000, 0b0010_0000, 0b0010_0000, 0b0010_0000, 0],
        b'8' => [0b0111_1000, 0b1000_0100, 0b1000_0100, 0b0111_1000, 0b1000_0100, 0b1000_0100, 0b0111_1000, 0],
        b'9' => [0b0111_1000, 0b1000_0100, 0b1000_0100, 0b0111_1100, 0b0000_0100, 0b0000_1000, 0b0111_0000, 0],
        _ => [0; 8], // space / unknown
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
