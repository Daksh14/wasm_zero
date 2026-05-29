use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // Scan this crate's source for #[wasm_zero] functions and emit the
    // self-contained bindings (bindings.ts + bindings.js) into pkg/.
    wasm_zero_build::generate(
        manifest_dir.join("src/lib.rs"),
        manifest_dir.join("pkg"),
    );

    println!("cargo:rerun-if-changed=build.rs");
}
