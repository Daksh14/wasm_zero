//! Parallel Mandelbrot renderer — a wasm_zero + exu port of the wasm-bindgen-rayon
//! demo. Computes an RGBA image of the Mandelbrot set, parallelizing the rows
//! across a rayon pool of Web Workers that share this module's linear memory.
//!
//! The full-canvas RGBA buffer is far larger than wasm_zero's rkyv return limit,
//! so [`mandelbrot`] returns a *pointer* into the (shared) linear memory and JS
//! reads the pixels zero-copy via the SharedArrayBuffer, then `putImageData`s
//! them. The thread-pool bootstrap is the shared [`wasm_zero_rayon_pool`]; the
//! `#[no_mangle]` wrappers live here so the wasm linker exports them.

use std::sync::Mutex;

use rayon::prelude::*;
use wasm_zero::wasm_zero;
use wasm_zero_rayon_pool as pool;

/// Reused output buffer (lives in shared linear memory). Single JS caller drives
/// renders sequentially, so reuse is safe; JS reads it before the next call.
static BUF: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// HSL→RGBA. `h` in [0,360), `s`/`l` in [0,1].
fn hsl_to_rgba(h: f64, s: f64, l: f64) -> [u8; 4] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
        255,
    ]
}

/// A spread-out color palette (golden-angle hues), indexed by iteration count.
fn palette() -> Vec<[u8; 4]> {
    (0..512)
        .map(|i| hsl_to_rgba((i as f64 * 137.508) % 360.0, 0.5, 0.6))
        .collect()
}

/// Render one pixel row `y` of the `w × h` image into `row` (w*4 RGBA bytes).
fn render_row(row: &mut [u8], y: usize, w: usize, h: usize, max_iter: u32, palette: &[[u8; 4]]) {
    let cy = (y as f64 / h as f64) * 4.0 - 2.0;
    for x in 0..w {
        let cx = (x as f64 / w as f64) * 4.0 - 2.0;
        // Iterate z = z² + c from z = 0 until |z|² > 4 or max_iter reached.
        let (mut zx, mut zy) = (0.0f64, 0.0f64);
        let mut i = 0u32;
        while i < max_iter && zx * zx + zy * zy <= 4.0 {
            let nx = zx * zx - zy * zy + cx;
            zy = 2.0 * zx * zy + cy;
            zx = nx;
            i += 1;
        }
        let color = if i >= max_iter {
            [0, 0, 0, 255]
        } else {
            palette[i as usize % palette.len()]
        };
        let o = x * 4;
        row[o..o + 4].copy_from_slice(&color);
    }
}

/// Render the Mandelbrot set into an RGBA buffer and return a pointer to it.
/// Scalar args + scalar (pointer) return → wasm_zero's direct-param fast path.
/// JS reads `width*height*4` bytes from the returned pointer in shared memory.
#[wasm_zero]
pub fn mandelbrot(width: u32, height: u32, max_iter: u32) -> u32 {
    let (w, h) = (width as usize, height as usize);
    let pal = palette();
    let stride = w * 4;

    let mut buf = BUF.lock().unwrap();
    buf.resize(w * h * 4, 0);
    let ptr = buf.as_ptr() as u32;
    let pixels = buf.as_mut_slice();

    if pool::is_threaded() {
        pool::install(|| {
            pixels
                .par_chunks_mut(stride)
                .enumerate()
                .for_each(|(y, row)| render_row(row, y, w, h, max_iter, &pal));
        });
    } else {
        pixels
            .chunks_mut(stride)
            .enumerate()
            .for_each(|(y, row)| render_row(row, y, w, h, max_iter, &pal));
    }

    ptr
}

// --- thread-pool FFI: thin wrappers over wasm_zero_rayon_pool. These live here
// in the cdylib root so the wasm linker exports them (a dependency's
// #[no_mangle] symbols get GC'd). See the pool crate for the protocol. --------

#[unsafe(no_mangle)]
pub extern "C" fn init_thread_pool(num_threads: usize) -> u32 {
    pool::init(num_threads)
}

#[unsafe(no_mangle)]
pub extern "C" fn build_thread_pool() {
    pool::build()
}

#[unsafe(no_mangle)]
pub extern "C" fn alloc_thread_stack(stack_size: usize, tls_size: usize, tls_align: usize) -> u64 {
    pool::alloc_thread_stack(stack_size, tls_size, tls_align)
}

#[unsafe(no_mangle)]
pub extern "C" fn shutdown_thread_pool() {
    pool::shutdown()
}

/// # Safety
/// `receiver_ptr` must be the value returned by [`init_thread_pool`] this task.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasm_thread_entry(receiver_ptr: u32) {
    unsafe { pool::run_thread(receiver_ptr) }
}
