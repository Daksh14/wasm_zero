# wasm_zero

A lightweight WASM FFI binding generator and [rkyv](https://rkyv.org/)
serialization wrapper between TypeScript and Rust. Built for **`no_std`** wasm
targets — no `wasm-bindgen`, no JS glue runtime, no allocator assumptions beyond
your own.

🌐 **Live demo:** https://daksh14.github.io/wasm_zero/

Annotate a Rust function with `#[wasm_zero]`, and wasm_zero gives you:

- an exported FFI shim that returns the function's result as an
  [rkyv](https://rkyv.org/) zero-copy archive, and
- generated, **self-contained** TypeScript/JavaScript bindings that call the
  shim and read each field straight out of wasm memory — no runtime library on
  the read path, no extra serialization layer.

```rust
#[wasm_zero]
pub fn get_adult_person() -> Person { /* ... */ }
```

```js
import { initWasmZero } from "./pkg/bindings.js";

const wasmzero = await initWasmZero("app.wasm");
const person = wasmzero.get_adult_person(); // -> { name, age, email, scores }
```

## Why

### Motivation

This library exists to enable and skyrocket WASM application development that
touches the frontend — and to make the web a faster and more secure place. Rust
compiled to WASM gives you memory safety and near-native speed; wasm_zero
removes the friction of getting that power to the browser. You annotate a Rust
function, and wasm_zero emits a JavaScript/TypeScript shim you can drop straight
into a bare `.html` file — no bundler, no glue runtime, no `std` assumption. The 
same emitted wasm can be run on the server with wasmtime or a self hosted VM.
To allow wasi targets set  `--target wasm32-wasi` as target.
`wasm-bindgen` is excellent but pulls in a JS glue. This library is meant for
rust to js communication and not the other way.

For small `no_std` wasm modules that just need to hand structured data to a
JavaScript host, that's a lot of machinery. wasm_zero takes a different tack:

- The Rust side serializes return values with rkyv into a flat byte buffer.
- The JS side reads that buffer field-by-field straight from wasm memory, using
  readers generated from your Rust types — no hand-maintained schema files and
  no runtime decode library. Strings still use TextDecoder which has a runtime
  cost along with branching for SSO.
- The only ABI surface is a handful of integer-in/integer-out functions plus
  linear memory. In the future we want to provide a way to provide a validated
  abi at compile time for custom ABI needs

## At Dusk Network: exu

At [Dusk Network](https://dusk.network/) we engineered a glue layer that runs on
`no_std`, is secure and sandboxed, and executes efficiently in the browser
*without blocking* the main thread by running inside Web Workers — alongside
wasm_zero.

That runtime is [`exu`](https://github.com/dusk-network/exu). It works in the
web browser or the Node.js runtime to run your WASM with:

- **Web Workers** in the browser and **actual OS-level threads** in Node.js, so
  invocations run off the main thread, non-blockingly.
- **Sandboxing** — your WASM runs isolated. After a function invocation exu can
  delete the WASM memory and drop the worker entirely, so nothing leaks between
  calls.

Note there are caveats with running thread pool like rayon in wasm and rust
version doesn't map to directly to web. At least until the shared everything
proposal. [Read more about this in wasm-bindgen documentation](https://wasm-bindgen.github.io/wasm-bindgen/examples/raytrace.html?highlight=threading#caveats)

We maintain a **fork of exu** (vendored in [`exu/`](exu)) that extends upstream
with a [**rayon**](https://github.com/rayon-rs/rayon) thread pool running over
*shared* wasm memory — no `wasm-bindgen` required. The fork keeps exu's
sandboxing model intact: the sandbox boundary is the *wasm instance + memory*,
not the worker. Workers are pooled and reused across tasks, while a **fresh
`WebAssembly.Memory` is minted per task** so nothing leaks between
invocations — yet *within* a task, the rayon pool's worker threads all share
that one memory for zero-copy parallelism (`SharedArrayBuffer` + atomics).

### Example: sandboxing + a shared-memory rayon pool

Each `module.task(...)` runs in its own sandbox over fresh memory (terminated
when the task resolves). Inside the task, `initThreadPool(threads)` stands up a
rayon pool whose workers share that sandbox's memory, so a parallel compute call
fans out across cores without copying:

```js
import { Module } from "./exu/src/mod.js";

const module = new Module(new URL("wasm_zero_rayon.wasm", import.meta.url));
module.defaultImports = new URL("imports.js", import.meta.url);

// Run a task: fresh sandbox + memory, dropped when this resolves (sandboxing).
const result = await module.task(async (exports, { initThreadPool }) => {
  // Spin up a rayon pool over this sandbox's shared memory.
  const threads = await initThreadPool(navigator.hardwareConcurrency);
  const primes = await exports.__wasm_zero_parallel_count_primes(1_000_000);
  return { primes, threads };
})();

console.log(result); // { primes: 78498, threads: <cores> }
```

The next `module.task(...)` gets a brand-new memory (sandboxed), while the
underlying workers are recycled from the pool. The
[`wasm_zero_rayon`](crates/wasm_zero_rayon) crate provides the compute functions
and the thread-pool bootstrap; see
[`exu/tests/rayon_deno_test.mjs`](exu/tests/rayon_deno_test.mjs) for the
end-to-end run (verified under Deno, ~3.8× on 10 threads) and
[`exu/tests/mandelbrot_deno_test.mjs`](exu/tests/mandelbrot_deno_test.mjs) for a
parallel Mandelbrot render that reads the RGBA buffer straight out of shared
memory.

> **Note:** shared memory needs cross-origin isolation —
> `wasm_zero_serve` stamps `COOP`/`COEP` on every response so
> `SharedArrayBuffer` is available in the browser.

## How it works

```text
#[wasm_zero] fn foo() -> T
        │
        ├── wasm_zero_macro (proc macro, compile time)
        │     emits  __wasm_zero_foo(out_ptr: u32) -> u32
        │     which rkyv-serializes T and writes [len: u32][bytes] at out_ptr
        │     + embeds foo's signature as a record in the `__wasm_zero`
        │       custom section of the binary (a #[link_section] static)
        │
        └── wasm_zero_build (post-build, on the compiled .wasm)
              reads the `__wasm_zero` section and emits pkg/bindings.{ts,js}:
                • per-struct readers (decode_Person reads each field)
                • FFI client         (initWasmZero / wasmzero.foo())
```

The macro is the single source of truth: it already has every annotated item's
exact signature as tokens, so it serializes that into a custom section of the
wasm binary itself (the linker concatenates the per-item records). The binding
generator then works from the compiled artifact — it never parses your source,
so `cfg`s, macro-generated functions, multi-file crates, and re-exports all
just work. This is the same architecture `wasm-bindgen` uses. The metadata
section is build-time-only; `--strip-to` writes a shipping copy without it.

### The FFI / memory protocol

For each `#[wasm_zero] fn foo(args...) -> T`:

1. The module exports a shim plus `malloc` / `free` (from `wasm_zero::mem`).
   Nullary functions export `__wasm_zero_foo(out_ptr) -> u32`; functions with
   arguments export `__wasm_zero_foo(in_ptr, out_ptr) -> u32`.
2. Arguments that are scalar primitives (`i8`…`u64`, `f32`, `f64`, `bool`) are
   passed **directly as wasm function parameters** — no encoding, no input
   buffer (the fast path). If any argument is non-scalar (`String`, a struct,
   `Vec`, …), all args are instead rkyv-archived **directly into** the input
   buffer (`[len][bytes]`) — rkyv-js writes through a fixed `RkyvWriter` bound
   to that region of wasm memory, so nothing is copied — and `in_ptr` is passed.
3. If the return type is a scalar primitive or `()`, the shim **returns it
   directly** as the wasm function's return value — no output buffer, no rkyv,
   no error code (it can't fail). Otherwise it writes
   `[len: u32 little-endian][rkyv archive bytes]` at `out_ptr` and returns an
   [`ErrorCode`](crates/wasm_zero/src/error.rs) (`Ok == 0`).
4. For buffer returns, JS reads `len` and decodes **straight from wasm memory**
   using a generated per-struct reader — each field read at its archived offset
   (scalars via `DataView`, numeric vecs as zero-copy typed-array views, strings
   transcoded on demand). No runtime library is involved on the read path.
   Scalar/unit returns are just the call's return value.

Both directions use the same `[len][bytes]` framing (the archive starts at a
16-byte-aligned offset so typed-array views are correctly aligned).

rkyv is configured for the standard v0.8 format: little-endian, aligned
primitives, **32-bit relative pointers**, root at the end of the buffer.

## Workspace layout

| Crate | Role |
|-------|------|
| [`wasm_zero`](crates/wasm_zero) | `no_std` runtime library: the `#[wasm_zero]` re-export, `ErrorCode`, and `mem` (malloc/free + buffer helpers). |
| [`wasm_zero_macro`](crates/wasm_zero_macro) | The `#[wasm_zero]` attribute proc macro that emits the FFI shim. |
| [`wasm_zero_build`](crates/wasm_zero_build) | Post-build binding generator: reads the `__wasm_zero` metadata section from the compiled `.wasm` and emits the self-contained TS/JS bindings (`bindings.ts` + `bindings.js`). |
| [`wasm_zero_serve`](crates/wasm_zero_serve) | Tiny axum static-file server for running the demo pages. |
| [`wasm_zero_test_nostd`](crates/wasm_zero_test_nostd) | `no_std` demo: rkyv types + `#[wasm_zero]` functions + a browser page. |
| [`wasm_zero_test`](crates/wasm_zero_test) | A `wasm-bindgen`/`std` comparison crate. |
| [`wasm_zero_rayon`](crates/wasm_zero_rayon) | Shared-memory rayon compute fns (`parallel_count_primes`, …) + the thread-pool bootstrap, driven by the exu fork. |
| [`wasm_zero_rayon_demo`](crates/wasm_zero_rayon_demo) | Parallel Mandelbrot render over shared wasm memory (returns an RGBA pointer JS reads zero-copy). |

There's also a standalone [`benchmark/`](benchmark) workspace comparing
`wasm-bindgen` and `wasm_zero` head-to-head — call overhead, data transfer, and
**bundle size** (`benchmark/sizes.sh`: wasm_zero ships ~2.2× smaller gzipped —
5.8 KB vs 12.8 KB — with no glue runtime and no read-path dependency at all).

## Usage

### 1. Add the dependencies

```toml
[dependencies]
wasm_zero = "0.1"
rkyv = { version = "0.8", default-features = false, features = ["alloc", "pointer_width_32"] }
```

### 2. Annotate your types and functions

```rust
#![no_std]
extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

use rkyv::{Archive, Deserialize, Serialize};
use wasm_zero::wasm_zero;

#[wasm_zero] // record the struct's layout for the binding generator
#[derive(Archive, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
    pub age: u32,
    pub email: Option<String>,
    pub scores: Vec<u32>,
}

#[wasm_zero]
pub fn get_adult_person() -> Person {
    Person { /* ... */ }
}
```

Structs that appear in a `#[wasm_zero]` function signature carry the attribute
too, so their field layout is recorded alongside the functions.

### 3. Generate the bindings from the compiled wasm

```bash
cargo build --target wasm32-unknown-unknown -p your_crate

# Reads the __wasm_zero metadata section from the binary and writes
# pkg/bindings.ts and pkg/bindings.js. Optionally add
# `--strip-to <out.wasm>` to also write a copy without the metadata section.
cargo run -p wasm_zero_build --example generate -- \
    target/wasm32-unknown-unknown/debug/your_crate.wasm pkg
```

(Or call `wasm_zero_build::generate(wasm_path, out_dir)` from your own tool.)

This produces a **self-contained** FFI client — a TypeScript interface plus a
zero-copy reader per struct, with no runtime dependency:

```ts
export interface Person {
  name: string;
  age: number;
  email: string | null;
  scores: Uint32Array; // zero-copy view over wasm memory
}

// reads each field straight from wasm memory at its archived offset
function decode_Person(dv, u8, p) { /* ... */ }

export async function initWasmZero(wasmUrl: string | URL, imports?: WebAssembly.Imports): Promise<WasmZero>;
```

Two files are emitted from one model:

- **`bindings.ts`** — plain TypeScript interfaces + typed client.
- **`bindings.js`** — the same module with types stripped, importable directly
  in a browser (no bundler, no CDN).

`rkyv-js` is imported **only** if a function takes a non-scalar argument
(`String`/struct/…) — and then only its encoder-only `rkyv-js/encode` entry,
which archives the argument straight into the input buffer (hand-rolling the
rkyv *writer* is out of scope). The read path never needs it.

### 4. Call it

```html
<script type="module">
  import { initWasmZero } from './pkg/bindings.js';
  const wasmzero = await initWasmZero('your_crate.wasm');
  console.log(wasmzero.get_adult_person());
</script>
<!-- Only if a function takes a non-scalar argument, the bindings import
     rkyv-js/encode (needs rkyv-js >= 0.2.0 for the external-buffer
     RkyvWriter); add an import map then:
     <script type="importmap">
       { "imports": { "rkyv-js/": "https://esm.sh/rkyv-js@0.3.0/" } }
     </script> -->
```

## Running the demo

```bash
# Build the no_std demo wasm
cargo build --target wasm32-unknown-unknown -p wasm_zero_test_nostd

# Regenerate the bindings from the compiled wasm (committed, so only needed
# after changing the demo's #[wasm_zero] items)
cargo run -p wasm_zero_build --example generate -- \
    target/wasm32-unknown-unknown/debug/wasm_zero_test_nostd.wasm \
    crates/wasm_zero_test_nostd/pkg

# Serve the workspace (defaults to http://127.0.0.1:8000)
cargo run -p wasm_zero_serve
```

Then open <http://127.0.0.1:8000/crates/wasm_zero_test_nostd/index.html>. The
page imports the generated `pkg/bindings.js`, calls `greet()` and
`get_adult_person()`, and renders the decoded values.

## `no_std` notes

The demo crate shows the expected setup for a `no_std` cdylib:

- a `#[global_allocator]` (the demo uses `dlmalloc`),
- a `#[panic_handler]` (`core::arch::wasm32::unreachable()`),
- `panic = "abort"` (via `.cargo/config.toml`) so no `eh_personality` is needed.

## Optimizing the wasm

wasm_zero already minimizes per-call work: scalar args/returns are bare wasm
calls (no buffer), the shim **serializes directly into the output buffer** (no
intermediate allocation or copy), and numeric vecs return zero-copy views.

For the smallest, fastest module, tune the consuming crate's release profile:

```toml
[profile.release]
opt-level = "z"    # smallest; use 3 for fastest
lto = "fat"        # cross-crate inlining
codegen-units = 1  # max optimization
strip = true       # drop symbols
```

Then run [`wasm-opt`](https://github.com/WebAssembly/binaryen) on the output
(`wasm-opt -O3 app.wasm -o app.wasm`; needs a recent Binaryen). Building with
`RUSTFLAGS="-C target-feature=+bulk-memory"` enables `memory.copy`/`fill` for
faster byte copies where your targets support it.

## Supported types

wasm_zero reads these Rust types from the archive into TypeScript:

| Rust | TypeScript |
|------|------------|
| `u8`–`i32`, `f32`, `f64`, `usize`/`isize` | `number` |
| `u64`, `i64` | `bigint` |
| `bool` | `boolean` |
| `char`, `String` | `string` |
| `Vec<T>` of a numeric primitive | `Uint32Array` / `Float64Array` / … (zero-copy view) |
| `Vec<T>` (other) | `T[]` |
| `Option<T>` | `T \| null` |
| `Box<T>` / `Rc<T>` / `Arc<T>` | `T` |
| `#[derive(Archive)] struct` | `interface` |

Enums, maps, tuples, and `[T; N]` arrays aren't read yet (the build fails with a
clear message). The reader is generated from the rkyv archived layout; for the
non-scalar **argument** path, encoding uses [rkyv-js](https://github.com/cometkim/rkyv-js).

### Argument handling

wasm_zero picks the cheapest way to pass arguments based on their types:

| Arguments | How they cross | Cost |
|-----------|----------------|------|
| none | nullary shim | — |
| all scalar primitives (`i8`…`u64`, `f32`, `f64`, `bool`) | passed **directly as wasm params** | none — like a bare call |
| any non-scalar (`String`, struct, `Vec`, …) | rkyv-archived **in place** into the input buffer via a fixed `RkyvWriter` | one archive, no copy |

(i64/u64 args are passed as JS `BigInt`.)

### Return handling

Return values are read **straight from wasm memory** by a generated per-struct
reader (`decode_<Struct>`) — each field at its archived offset, no runtime
library:

| Return type | How it crosses | Result |
|-------------|----------------|--------|
| scalar primitive or `()` | **returned directly** as the wasm function's value — a bare call, no buffer (matches wasm_bindgen) | `number` / `bigint` / `boolean` / `void` |
| `Vec<T>` of a numeric primitive | **zero-copy typed-array view** (`Uint32Array`, `Float64Array`, …) over the archived elements | a view that **aliases** wasm memory |
| struct / `String` / `Option` / `Vec<T>` / nested | fields read directly at their offsets (`DataView`/views; strings transcoded) | a plain object; numeric-vec fields are views |

Notes:

- **Views alias the shared scratch buffer**, so a returned struct's numeric-vec
  field (or a top-level numeric-vec return) is only valid until the next call on
  that instance (or a `memory.grow`). `.slice()` / copy it to keep it. Scalar and
  string fields are owned (copied) and safe to keep.
- The rkyv layout for numeric vecs is native little-endian, so the view needs no
  copy and no per-element decode. Strings are always materialized (UTF-8 →
  UTF-16) when read.

This is why, in the [benchmark](benchmark), `wasm_zero` matches or beats
wasm_bindgen on scalar calls and numeric-array returns; for full struct
materialization with strings, serde-wasm-bindgen's single-pass build is still
a touch faster, while wasm_zero wins when you read only some fields.

## Limitations

- Arguments must be **owned** rkyv types (e.g. `i32`, `String`, a `#[derive(Archive)]`
  struct) — borrowed parameters like `&str` aren't decodable from the input
  buffer. Non-scalar arguments require `rkyv-js` >= 0.2.0 (its encoder-only
  `rkyv-js/encode` entry, for the external-buffer `RkyvWriter`).
- The generated readers cover scalars, `String`, `Option`, `Vec`, `Box`/`Rc`/`Arc`,
  and `#[derive(Archive)]` structs. Enums, maps, and tuples aren't read yet
  (the build fails with a clear message).
- Only the standard rkyv v0.8 format is supported (little-endian, aligned,
  32-bit pointers).

## License

See individual crate headers; portions are MPL-2.0 (Dusk Network).
