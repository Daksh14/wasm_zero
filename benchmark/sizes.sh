#!/usr/bin/env bash
# Compare shipped sizes: wasm_bindgen vs wasm_zero (raw + gzipped).
# Run ./build.sh first. Gzip size is the relevant over-the-wire number.
set -euo pipefail
cd "$(dirname "$0")/web/pkg"

raw() { wc -c < "$1" | tr -d ' '; }
gz()  { gzip -9 -c "$1" | wc -c | tr -d ' '; }

row() { # label file
  if [ -f "$2" ]; then
    printf "  %-26s %9s %9s\n" "$1" "$(raw "$2")" "$(gz "$2")"
  else
    printf "  %-26s %9s %9s\n" "$1" "(missing)" "-"
  fi
}

total() { # label file1 file2...
  local label="$1"; shift
  local r=0 g=0 f
  for f in "$@"; do [ -f "$f" ] && { r=$((r + $(raw "$f"))); g=$((g + $(gz "$f"))); }; done
  printf "  %-26s %9s %9s\n" "$label" "$r" "$g"
}

WB_WASM=wasm-bindgen/bench_wasm_bindgen_bg.wasm
WB_JS=wasm-bindgen/bench_wasm_bindgen.js
WZ_WASM=wasm-zero/bench_wasm_zero.wasm
WZ_JS=wasm-zero/bindings.js

printf "  %-26s %9s %9s\n" "component" "raw (B)" "gzip (B)"
printf "  %s\n" "------------------------------------------------"
echo "  wasm_bindgen:"
row ".wasm"            "$WB_WASM"
row ".js (glue)"       "$WB_JS"
total "= shipped total" "$WB_WASM" "$WB_JS"
echo "  wasm_zero:"
row ".wasm"            "$WZ_WASM"
row "bindings.js"      "$WZ_JS"
total "= shipped total" "$WZ_WASM" "$WZ_JS"
echo
echo "  Notes:"
echo "  - wasm_bindgen ships the .wasm + a per-module JS glue file."
echo "  - wasm_zero ships the .wasm + a small bindings.js, and shares one"
echo "    rkyv-js runtime across all modules (loaded once; from a CDN in the"
echo "    demo), so it is excluded from the per-module total above."
