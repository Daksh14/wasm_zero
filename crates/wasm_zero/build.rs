use rkyv_js_codegen::CodeGenerator;
use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let mut codegen = CodeGenerator::new();

    codegen.set_header(
        "Generated TypeScript bindings from rkyv-js-codegen\n\
         These types match the Rust structs in src/lib.rs",
    );

    codegen
        .add_source_file(manifest_dir.join("src/lib.rs"))
        .expect("Failed to parse source file");

    codegen
        .write_to_file(out_dir.join("bindings.ts"))
        .expect("Failed to write bindings");

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=build.rs");
}
