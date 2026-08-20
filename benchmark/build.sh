#!/usr/bin/env bash
# Builds both benchmark suites into web/pkg/ for serving.
#
#   wasm-bindgen suite -> web/pkg/wasm-bindgen/  (via wasm-pack)
#   wasm-zero suite    -> web/pkg/wasm-zero/     (cargo + generated bindings)
#
# Then serve the repo root (e.g. `cargo run -p wasm_zero_serve`) and open
#   http://127.0.0.1:8000/benchmark/web/index.html
set -euo pipefail
cd "$(dirname "$0")"

mkdir -p web/pkg/wasm-bindgen web/pkg/wasm-zero

echo ">> building wasm-bindgen suite (wasm-pack)"
wasm-pack build wasm-bindgen \
  --target web --release \
  --out-dir ../web/pkg/wasm-bindgen \
  --out-name bench_wasm_bindgen

echo ">> building wasm-zero suite (cargo, then bindings from the wasm metadata)"
( cd wasm-zero && cargo build --release )

# Generate bindings from the __wasm_zero custom section of the compiled wasm,
# then ship a copy with that section stripped (it's build-time-only data).
( cd .. && cargo run -q -p wasm_zero_build --example generate -- \
    benchmark/target/wasm32-unknown-unknown/release/bench_wasm_zero.wasm \
    benchmark/web/pkg/wasm-zero \
    --strip-to benchmark/web/pkg/wasm-zero/bench_wasm_zero.wasm )

# wasm-opt both outputs (best-effort: an old Binaryen may not validate the
# LTO'd modules — if so it's skipped for both, keeping the comparison fair).
if command -v wasm-opt >/dev/null; then
  for w in web/pkg/wasm-bindgen/bench_wasm_bindgen_bg.wasm \
           web/pkg/wasm-zero/bench_wasm_zero.wasm; do
    if wasm-opt -O3 "$w" -o "$w.tmp" 2>/dev/null; then
      mv "$w.tmp" "$w"; echo ">> wasm-opt -O3 $(basename "$w")"
    else
      rm -f "$w.tmp"; echo ">> wasm-opt skipped for $(basename "$w") (Binaryen too old?)"
    fi
  done
fi

echo ">> sizes:"
./sizes.sh

echo ">> done. Serve the repo root and open:"
echo "   http://127.0.0.1:8000/benchmark/web/index.html"
