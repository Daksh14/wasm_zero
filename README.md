# wasm_zero

A lightweight WASM FFI binding generator and [rkyv](https://rkyv.org/)
serialization wrapper between TypeScript and Rust. Built for **`no_std`** wasm
targets — no `wasm-bindgen`, no JS glue runtime, no allocator assumptions beyond
your own.

Annotate a Rust function with `#[wasm_zero]`, and wasm_zero gives you:

- an exported FFI shim that returns the function's result as an
  [rkyv](https://rkyv.org/) zero-copy archive, and
- generated TypeScript/JavaScript bindings that call the shim and decode the
  archive in JS using [rkyv-js](https://www.npmjs.com/package/rkyv-js) — no
  extra serialization layer.

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

`wasm-bindgen` is excellent but pulls in a JS glue runtime and assumes `std`.
For small `no_std` wasm modules that just need to hand structured data to a
JavaScript host, that's a lot of machinery. wasm_zero takes a different tack:

- The Rust side serializes return values with rkyv into a flat byte buffer.
- The JS side reads that buffer directly with rkyv-js — the schema is derived
  from your Rust types, so there are no hand-maintained schema files.
- The only ABI surface is a handful of integer-in/integer-out functions plus
  linear memory.

## How it works

```text
#[wasm_zero] fn foo() -> T
        │
        ├── wasm_zero_macro (proc macro, compile time)
        │     emits  __wasm_zero_foo(out_ptr: u32) -> u32
        │     which rkyv-serializes T and writes [len: u32][bytes] at out_ptr
        │
        └── wasm_zero_build (build.rs, before compile)
              scans the source and emits pkg/bindings.{ts,js}:
                • rkyv-js codecs   (ArchivedPerson = r.struct({...}))
                • FFI client       (initWasmZero / wasmzero.foo())
```

A proc macro can only return tokens to the compiler — it can't write files. A
`build.rs` runs *before* the compiler and can. So the two responsibilities are
split: the macro transforms code, the build helper emits the binding artifacts.
This is the same division `prost`/`prost-build` and `uniffi` use.

### The FFI / memory protocol

For each `#[wasm_zero] fn foo() -> T` (functions must be **nullary**):

1. The module exports `__wasm_zero_foo(out_ptr: u32) -> u32` plus `malloc` /
   `free` (from `wasm_zero::mem`).
2. JS allocates a scratch buffer and calls the shim with its pointer.
3. The shim writes `[len: u32 little-endian][rkyv archive bytes]` at `out_ptr`
   and returns an [`ErrorCode`](crates/wasm_zero/src/error.rs) (`Ok == 0`).
4. JS reads `len`, copies out the archive bytes, and decodes them with
   `r.decode(codec, bytes)`.

rkyv is configured for the format rkyv-js expects: little-endian, aligned
primitives, **32-bit relative pointers**, root at the end of the buffer.

## Workspace layout

| Crate | Role |
|-------|------|
| [`wasm_zero`](crates/wasm_zero) | `no_std` runtime library: the `#[wasm_zero]` re-export, `ErrorCode`, and `mem` (malloc/free + buffer helpers). |
| [`wasm_zero_macro`](crates/wasm_zero_macro) | The `#[wasm_zero]` attribute proc macro that emits the FFI shim. |
| [`wasm_zero_build`](crates/wasm_zero_build) | `build.rs` helper that generates the rkyv-js bindings (`bindings.ts` + `bindings.js`). |
| [`wasm_zero_serve`](crates/wasm_zero_serve) | Tiny axum static-file server for running the demo pages. |
| [`wasm_zero_test_nostd`](crates/wasm_zero_test_nostd) | `no_std` demo: rkyv types + `#[wasm_zero]` functions + a browser page. |
| [`wasm_zero_test`](crates/wasm_zero_test) | A `wasm-bindgen`/`std` comparison crate. |

## Usage

### 1. Add the dependencies

```toml
[dependencies]
wasm_zero = "0.1"
rkyv = { version = "0.8", default-features = false, features = ["alloc", "pointer_width_32"] }

[build-dependencies]
wasm_zero_build = "0.1"
```

### 2. Annotate your types and functions

```rust
#![no_std]
extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

use rkyv::{Archive, Deserialize, Serialize};
use wasm_zero::wasm_zero;

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

### 3. Generate the bindings from `build.rs`

```rust
fn main() {
    // Writes pkg/bindings.ts and pkg/bindings.js
    wasm_zero_build::generate("src/lib.rs", "pkg");
}
```

This produces rkyv-js codecs and the FFI client:

```ts
import * as r from 'rkyv-js';

export const ArchivedPerson = r.struct({
  name: r.string,
  age: r.u32,
  email: r.option(r.string),
  scores: r.vec(r.u32),
});
export type Person = r.Infer<typeof ArchivedPerson>;

export async function initWasmZero(wasmUrl: string | URL, imports?: WebAssembly.Imports): Promise<...>;
```

Two files are emitted from one model:

- **`bindings.ts`** — idiomatic rkyv-js with `r.Infer<>` types, for
  TypeScript / bundler consumers (`yarn add rkyv-js`).
- **`bindings.js`** — the same module with types stripped, importable directly
  in a browser (resolve the `rkyv-js` specifier via an
  [import map](https://developer.mozilla.org/en-US/docs/Web/HTML/Element/script/type/importmap)).

### 4. Build the wasm and call it

```bash
cargo build --target wasm32-unknown-unknown -p your_crate
```

```html
<script type="importmap">
  { "imports": { "rkyv-js": "https://esm.sh/rkyv-js@latest" } }
</script>
<script type="module">
  import { initWasmZero } from './pkg/bindings.js';
  const wasmzero = await initWasmZero('your_crate.wasm');
  console.log(wasmzero.get_adult_person());
</script>
```

## Running the demo

```bash
# Build the no_std demo wasm
cargo build --target wasm32-unknown-unknown -p wasm_zero_test_nostd

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

## Supported types

wasm_zero maps Rust types to rkyv-js codecs:

| Rust | Codec | TypeScript |
|------|-------|------------|
| `u8`–`i32`, `f32`, `f64`, `usize`/`isize` | `r.u8` … `r.f64` | `number` |
| `u64`, `i64` | `r.u64`, `r.i64` | `bigint` |
| `bool` | `r.bool` | `boolean` |
| `char`, `String`, `&str` | `r.char` / `r.string` | `string` |
| `Vec<T>` | `r.vec(T)` | `T[]` |
| `Option<T>` | `r.option(T)` | `T \| null` |
| `Box<T>` / `Rc<T>` / `Arc<T>` | `r.box` / `r.rc` | `T` |
| `[T; N]` | `r.array(T, N)` | `T[]` |
| `(A, B, …)` | `r.tuple(A, B, …)` | `[A, B, …]` |
| `#[derive(Archive)] struct` | `r.struct({...})` | `interface` |

For the full codec catalogue (enums, maps, external crate types), see the
[rkyv-js](https://www.npmjs.com/package/rkyv-js) docs.

## Limitations

- `#[wasm_zero]` functions must currently be **nullary** — the shim ABI only
  carries the output pointer.
- Only the default rkyv v0.8 format is supported (little-endian, aligned,
  32-bit pointers), matching rkyv-js.
- Enums and map types are generated by the underlying rkyv-js codec set but are
  not yet exercised by wasm_zero's own demos.

## License

See individual crate headers; portions are MPL-2.0 (Dusk Network).
