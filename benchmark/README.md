# wasm_bindgen vs wasm_zero benchmark

Modeled on [alexcrichton/rust-wasm-benchmark](https://github.com/alexcrichton/rust-wasm-benchmark),
this compares the per-call and data-transfer cost of [`wasm-bindgen`](https://github.com/rustwasm/wasm-bindgen)
against `wasm_zero` (rkyv round-trip via [rkyv-js](https://github.com/cometkim/rkyv-js)),
with a pure-JS baseline.

It is a **standalone cargo workspace** so the heavy `wasm-bindgen` / `web-sys`
dependencies stay out of the core `wasm_zero` crates.

```
benchmark/
├── wasm-bindgen/   crate: the wasm-bindgen suite (cdylib)
├── wasm-zero/      crate: the wasm_zero suite (no_std cdylib, build.rs emits bindings)
├── web/
│   ├── index.html  harness page + importmap for rkyv-js
│   ├── index.js    measurement harness
│   └── pkg/        build output (gitignored)
└── build.sh
```

## Tests

| Test | What it measures | js | wasm_bindgen | wasm_zero |
|------|------------------|----|--------------|-----------|
| `thunk` | empty-call overhead | ✓ | ✓ | ✓ (rkyv `()` round-trip) |
| `add` | two `i32` args → `i32` | ✓ | ✓ | ✓ (args rkyv-encoded) |
| `fibonacci` | one `i32` arg → result | ✓ | ✓ | ✓ (arg rkyv-encoded) |
| `get_person` | return a struct to JS | ✓ | ✓ (serde-wasm-bindgen) | ✓ (rkyv struct) |
| `get_scores` | return 1024×`u32` to JS | ✓ | ✓ (typed array) | ✓ (zero-copy `Uint32Array` view) |

`wasm_zero` pays an encode/decode + malloc/free tax per call (it's built for
*structured data*, not bare calls), so expect it to trail on `thunk`/`add` and
become competitive on `get_person`/`get_scores`. The numbers are whatever your
browser reports — run it and see.

## Build & run

Requires `wasm-pack` and the `wasm32-unknown-unknown` target.

```bash
./build.sh                     # builds both suites into web/pkg/
cargo run -p wasm_zero_serve   # serve the repo root (from the parent workspace)
```

Open <http://127.0.0.1:8000/benchmark/web/index.html> and click **Run
benchmarks**. (rkyv-js is loaded from GitHub via esm.sh, so the page needs
network access the first time.)

## Notes

- The wasm-bindgen suite is built with `wasm-pack build --target web` (ES module
  output) so the harness can `import` it; the upstream `--no-modules` +
  `delete WebAssembly.instantiateStreaming` hacks aren't needed.
- The wasm-bindgen `extern` imports (`thunk`, `add`, `Foo`, `doesnt_throw`)
  resolve against globals defined in `index.html`, matching upstream.
- The upstream "raw `.wast`" and `stdweb` suites are omitted (stdweb is
  unmaintained); add a raw suite later if you want a hand-written-wasm floor.
