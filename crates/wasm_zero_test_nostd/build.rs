use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // Scan this crate's source for #[wasm_zero] functions and emit the matching
    // TypeScript bindings next to the wasm output.
    wasm_zero_build::generate(
        manifest_dir.join("src/lib.rs"),
        manifest_dir.join("pkg/bindings.ts"),
    );

    println!("cargo:rerun-if-changed=build.rs");
}
