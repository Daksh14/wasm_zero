//! `wasm_zero` + **rayon** on `wasm32-unknown-unknown`.
//!
//! Unlike the `no_std` demos in this workspace, this crate is a *`std`* crate:
//! threaded wasm needs the standard library itself recompiled with the atomics
//! ABI, which is what `-Z build-std` (see `.cargo/config.toml`) does on the
//! pinned nightly. The module is linked with a **shared** memory + `panic=abort`
//! so that every Web Worker which instantiates it aliases the same linear
//! memory — the precondition for rayon's workers to see each other's data.
//!
//! ## How threads actually start
//!
//! There is no OS-thread spawning on wasm, so we can't let rayon spawn threads
//! itself. Instead [`init_thread_pool`] builds the global pool with a custom
//! `spawn_handler` that *hands each rayon worker out to JS* (as a leaked
//! [`rayon::ThreadBuilder`] pointer) rather than spawning. JS then starts one
//! Web Worker per pointer; each Worker re-instantiates this module against the
//! shared memory and calls [`wasm_thread_entry`], which runs that worker's
//! rayon loop. This is the same dance `wasm-bindgen-rayon` performs, but done
//! by hand so the crate keeps `wasm_zero`'s "no wasm-bindgen runtime" property.
//!
//! Bootstrap sequence (driven from JS — see `index.html` + `exu`; needs a
//! cross-origin-isolated page so the imported memory is a SharedArrayBuffer).
//! Each helper is a *separate* instance over the one shared memory; wasm globals
//! are per-instance, so each helper must get a private stack + TLS first. The
//! ordering mirrors wasm-bindgen-rayon: helpers must block on the channel
//! *before* the pool is built, or `build()` deadlocks waiting for them to prime.
//!   1. coordinator: `init_thread_pool(n)` → makes the channel, returns the
//!      receiver pointer.
//!   2. per helper: `alloc_thread_stack(..)` (on the coordinator) → packed
//!      (stack_top, tls_base); ship to the helper Worker, which sets its
//!      `__stack_pointer`, calls `__wasm_init_tls(tls_base)`, then
//!      `wasm_thread_entry(receiver_ptr)` — blocking in `recv()`.
//!   3. coordinator: `build_thread_pool()` → hands a `ThreadBuilder` to each
//!      waiting helper; they prime and the pool is ready.
//!   4. coordinator: call the parallel `#[wasm_zero]` exports (run on the pool).
//!   5. teardown: `shutdown_thread_pool()` drops the pool → helpers' `run()`
//!      returns → those Workers go back to the pool; the memory is then dropped.

use rayon::prelude::*;
use wasm_zero::wasm_zero;
use wasm_zero_rayon_pool as pool;

fn is_prime(x: u32) -> bool {
    let mut d = 2u32;
    while d * d <= x {
        if x % d == 0 {
            return false;
        }
        d += 1;
    }
    true
}

// --- parallel compute exposed through wasm_zero ---------------------------
//
// Each runs on the bootstrapped pool when one exists (work dispatched across
// the helper threads), and *sequentially* otherwise — the honest single-thread
// baseline. We deliberately don't touch rayon's global pool: it can't spawn
// threads on wasm and leaves stale state in linear memory.

/// Sum of squares `0..n`. Scalar in, scalar out → rides wasm_zero's direct-param
/// fast path (no buffers, no rkyv).
#[wasm_zero]
pub fn parallel_sum_squares(n: u32) -> u64 {
    let range = 0u64..n as u64;
    if pool::is_threaded() {
        pool::install(|| range.into_par_iter().map(|x| x * x).sum())
    } else {
        range.map(|x| x * x).sum()
    }
}

/// Count primes below `n` by trial division — embarrassingly parallel and heavy
/// enough that the speedup from extra threads is visible.
#[wasm_zero]
pub fn parallel_count_primes(n: u32) -> u32 {
    if pool::is_threaded() {
        pool::install(|| (2u32..n).into_par_iter().filter(|&x| is_prime(x)).count() as u32)
    } else {
        (2u32..n).filter(|&x| is_prime(x)).count() as u32
    }
}

// --- thread-pool FFI: thin wrappers over wasm_zero_rayon_pool. These must live
// here in the cdylib root so the wasm linker exports them (a dependency's
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
