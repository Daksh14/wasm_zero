//! Tiny CLI wrapper around [`wasm_zero_build::generate`], handy for generating
//! bindings outside of a `build.rs` (e.g. in CI or by hand).
//!
//! ```text
//! cargo run -p wasm_zero_build --example generate -- <src.rs> <out_dir>
//! ```

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(src), Some(out_dir)) = (args.next(), args.next()) else {
        eprintln!("usage: generate <src.rs> <out_dir>");
        std::process::exit(2);
    };
    wasm_zero_build::generate(src, out_dir);
}
