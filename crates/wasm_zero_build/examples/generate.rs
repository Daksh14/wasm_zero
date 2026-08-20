//! Tiny CLI wrapper around [`wasm_zero_build::generate`]. Run it after
//! `cargo build --target wasm32-unknown-unknown` — it reads the `__wasm_zero`
//! metadata section from the compiled binary and writes the bindings.
//!
//! ```text
//! cargo run -p wasm_zero_build --example generate -- \
//!     <module.wasm> <out_dir> [--strip-to <stripped.wasm>]
//! ```
//!
//! `--strip-to` additionally writes a copy of the module with the metadata
//! section removed — use that copy as the shipped artifact.

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(wasm), Some(out_dir)) = (args.next(), args.next()) else {
        usage();
    };
    let strip_to = match (args.next().as_deref(), args.next()) {
        (None, _) => None,
        (Some("--strip-to"), Some(out)) => Some(out),
        _ => usage(),
    };

    wasm_zero_build::generate(&wasm, out_dir);
    if let Some(out) = strip_to {
        wasm_zero_build::strip(&wasm, out);
    }
}

fn usage() -> ! {
    eprintln!("usage: generate <module.wasm> <out_dir> [--strip-to <stripped.wasm>]");
    std::process::exit(2);
}
