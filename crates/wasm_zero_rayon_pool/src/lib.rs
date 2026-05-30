//! Rayon-over-shared-memory thread-pool bootstrap, reusable across wasm_zero
//! cdylib crates. The actual FFI exports are thin `#[no_mangle]` wrappers in
//! each cdylib (a dependency's `#[no_mangle]` symbols get dropped by the wasm
//! linker), so this crate exposes the logic as ordinary functions:
//!
//!   - [`init`]  → make the ThreadBuilder channel, return the receiver pointer
//!   - [`run_thread`] → helper Worker entry: block on the channel, run rayon
//!   - [`alloc_thread_stack`] → carve a private stack + TLS out of the shared heap
//!   - [`build`] → build the pool once all helpers are waiting on the channel
//!   - [`shutdown`] → drop the pool so helpers' `run()` returns and Workers free
//!   - [`is_threaded`] / [`install`] → run compute on the pool when present
//!
//! Why this exact ordering (channel, not a queue): `ThreadPoolBuilder::build()`
//! blocks in `wait_until_primed` until every spawned thread runs, so helpers
//! must already be blocked on `recv()` before [`build`] is called. This mirrors
//! wasm-bindgen-rayon. See the `exu-rayon-threading` notes.

use std::sync::Mutex;

use crossbeam_channel::{Receiver, Sender, bounded};
use rayon::{ThreadBuilder, ThreadPool};

/// The bootstrapped pool, if any. Held (not `build_global`) so it can be dropped
/// to release the helper Workers for reuse.
static POOL: Mutex<Option<ThreadPool>> = Mutex::new(None);

struct PoolBuilder {
    num_threads: usize,
    sender: Sender<ThreadBuilder>,
    receiver: Receiver<ThreadBuilder>,
}

/// Boxed so the receiver has a stable address to hand to JS as a pointer; kept
/// alive (channel open) until [`shutdown`].
static BUILDER: Mutex<Option<Box<PoolBuilder>>> = Mutex::new(None);

/// Step 1: make the channel for `num_threads` helper threads; return a pointer
/// to the receiver that each helper will block on in [`run_thread`].
pub fn init(num_threads: usize) -> u32 {
    let (sender, receiver) = bounded(num_threads);
    let builder = Box::new(PoolBuilder {
        num_threads,
        sender,
        receiver,
    });
    let receiver_ptr = (&builder.receiver as *const Receiver<ThreadBuilder>) as u32;
    *BUILDER.lock().unwrap() = Some(builder);
    receiver_ptr
}

/// Step 2 (helper Worker entry): block until this thread's `ThreadBuilder`
/// arrives, then run its rayon loop. Returns when the pool is dropped.
///
/// # Safety
/// `receiver_ptr` must be the value returned by [`init`] for the current task.
pub unsafe fn run_thread(receiver_ptr: u32) {
    let receiver = unsafe { &*(receiver_ptr as *const Receiver<ThreadBuilder>) };
    if let Ok(thread) = receiver.recv() {
        thread.run();
    }
}

/// Allocate a private stack + TLS block out of the shared heap. Runs on the
/// coordinator (which has a valid stack); returns packed `(stack_top << 32) |
/// tls_base`. Pass `stack_size = 0` to allocate TLS only (coordinator keeps its
/// default stack). `tls_size`/`tls_align` come from the module's exported
/// `__tls_size`/`__tls_align` globals (read by JS). No free — the whole memory
/// is dropped per task.
pub fn alloc_thread_stack(stack_size: usize, tls_size: usize, tls_align: usize) -> u64 {
    let align = tls_align.max(1);
    let total = stack_size + tls_size + align;
    let base = wasm_zero::mem::malloc(total as u32) as usize;
    let stack_top = base + stack_size;
    let tls_base = (stack_top + align - 1) & !(align - 1);
    ((stack_top as u64) << 32) | (tls_base as u64 & 0xffff_ffff)
}

/// Step 3: build the pool. The spawn handler sends each `ThreadBuilder` over the
/// channel to a helper already blocked in `recv()`, so it primes while `build()`
/// waits — no deadlock.
pub fn build() {
    let (num_threads, sender) = {
        let guard = BUILDER.lock().unwrap();
        let builder = guard.as_ref().expect("init must run before build");
        (builder.num_threads, builder.sender.clone())
    };

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .spawn_handler(move |thread| {
            sender.send(thread).expect("no helper waiting to run thread");
            Ok(())
        })
        .build();

    if let Ok(pool) = pool {
        *POOL.lock().unwrap() = Some(pool);
    }
}

/// Drop the pool (helpers' `run()` returns → their Workers free up) and close
/// the channel.
pub fn shutdown() {
    let pool = POOL.lock().unwrap().take();
    drop(pool);
    *BUILDER.lock().unwrap() = None;
}

/// Whether a pool is currently bootstrapped (i.e. run multi-threaded).
pub fn is_threaded() -> bool {
    POOL.lock().unwrap().is_some()
}

/// Run `f` on the pool (work dispatched across helper threads). Only meaningful
/// when [`is_threaded`]; callers run their sequential variant otherwise so they
/// never touch rayon's (unusable on wasm) global pool.
pub fn install<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    match POOL.lock().unwrap().as_ref() {
        Some(pool) => pool.install(f),
        None => f(),
    }
}
