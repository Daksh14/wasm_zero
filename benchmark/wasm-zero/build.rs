use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // Emit the rkyv-js bindings next to the served assets.
    wasm_zero_build::generate(
        manifest_dir.join("src/lib.rs"),
        manifest_dir.join("../web/pkg/wasm-zero"),
    );

    println!("cargo:rerun-if-changed=build.rs");
}
