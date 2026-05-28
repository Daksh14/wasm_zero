//! Build-helper for `wasm_zero`.
//!
//! `wasm_zero` is split into two halves that run at different points in the
//! build:
//!
//! ```text
//! wasm_zero_macro   (proc macro)  -> runs inside rustc, emits the FFI shim
//! wasm_zero_build   (this crate)  -> runs in build.rs, emits the JS/TS bindings
//! ```
//!
//! A proc macro can only hand a `TokenStream` back to the compiler — it cannot
//! write files to the workspace. `build.rs`, on the other hand, runs *before*
//! rustc and is free to read source files and write artifacts.
//!
//! This helper scans a source file for rkyv structs and `#[wasm_zero]`
//! functions and emits bindings that target the [`rkyv-js`] runtime: it
//! generates the `r.struct({...})` codecs (matching rkyv-js-codegen
//! conventions) plus the wasm_zero FFI client that calls the exported shims and
//! decodes their rkyv payloads.
//!
//! From a consumer crate's `build.rs`:
//!
//! ```no_run
//! fn main() {
//!     // Writes <out_dir>/bindings.ts and <out_dir>/bindings.js
//!     wasm_zero_build::generate("src/lib.rs", "pkg");
//! }
//! ```
//!
//! ```js
//! import { initWasmZero } from "./pkg/bindings.js";
//! const wasmzero = await initWasmZero("app.wasm");
//! const person = wasmzero.get_adult_person(); // decoded via rkyv-js
//! ```
//!
//! Two files are emitted from one model:
//! - `bindings.ts` — idiomatic rkyv-js output with `r.Infer<>` types, for
//!   bundler/TypeScript consumers.
//! - `bindings.js` — the same module with types stripped, importable directly
//!   in a browser (resolve `rkyv-js` via an importmap).
//!
//! [`rkyv-js`]: https://www.npmjs.com/package/rkyv-js

use std::collections::BTreeSet;
use std::path::Path;

use quote::ToTokens;
use syn::{Fields, Item, ItemFn, ItemStruct, ReturnType, Type};

/// A named rkyv struct and its fields, in declaration order.
type StructDef = (String, Vec<(String, Type)>);

/// The result of [`generate_bindings`]: the two files to write.
pub struct Generated {
    /// Contents of `bindings.ts` — idiomatic rkyv-js with `r.Infer<>` types.
    pub ts: String,
    /// Contents of `bindings.js` — runtime-only, browser-importable.
    pub js: String,
}

/// Scan `src` for rkyv structs + `#[wasm_zero]` functions and write
/// `bindings.ts` and `bindings.js` into `out_dir`, creating it if needed.
///
/// Intended to be called from a `build.rs`; emits the appropriate
/// `cargo:rerun-if-changed` line. Panics (failing the build) on
/// read/parse/write errors — a build script has no better recovery.
pub fn generate(src: impl AsRef<Path>, out_dir: impl AsRef<Path>) {
    let src = src.as_ref();
    let out_dir = out_dir.as_ref();

    let source = std::fs::read_to_string(src).unwrap_or_else(|e| {
        panic!("wasm_zero_build: failed to read {}: {e}", src.display())
    });

    let generated = generate_bindings(&source).unwrap_or_else(|e| {
        panic!("wasm_zero_build: failed to parse {}: {e}", src.display())
    });

    std::fs::create_dir_all(out_dir).unwrap_or_else(|e| {
        panic!("wasm_zero_build: failed to create {}: {e}", out_dir.display())
    });

    for (name, contents) in
        [("bindings.ts", &generated.ts), ("bindings.js", &generated.js)]
    {
        let path = out_dir.join(name);
        std::fs::write(&path, contents).unwrap_or_else(|e| {
            panic!("wasm_zero_build: failed to write {}: {e}", path.display())
        });
    }

    println!("cargo:rerun-if-changed={}", src.display());
}

/// Parse `source` and render both binding files. Pure (no I/O), so it can be
/// unit-tested directly.
pub fn generate_bindings(source: &str) -> syn::Result<Generated> {
    let file = syn::parse_file(source)?;

    let mut structs: Vec<StructDef> = Vec::new();
    for item in &file.items {
        if let Item::Struct(s) = item {
            if derives_archive(s) {
                if let Fields::Named(fields) = &s.fields {
                    let named = fields
                        .named
                        .iter()
                        .map(|f| {
                            (f.ident.as_ref().unwrap().to_string(), f.ty.clone())
                        })
                        .collect();
                    structs.push((s.ident.to_string(), named));
                }
            }
        }
    }
    let struct_names: BTreeSet<String> =
        structs.iter().map(|(n, _)| n.clone()).collect();

    let funcs: Vec<&ItemFn> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(f) if has_wasm_zero_attr(f) => Some(f),
            _ => None,
        })
        .collect();

    Ok(Generated {
        ts: render_module(true, &structs, &struct_names, &funcs),
        js: render_module(false, &structs, &struct_names, &funcs),
    })
}

/// True if the function carries the `#[wasm_zero]` attribute.
fn has_wasm_zero_attr(f: &ItemFn) -> bool {
    f.attrs.iter().any(|a| a.path().is_ident("wasm_zero"))
}

/// True if the struct's `#[derive(...)]` includes `Archive`.
fn derives_archive(s: &ItemStruct) -> bool {
    s.attrs.iter().any(|attr| {
        if !attr.path().is_ident("derive") {
            return false;
        }
        let mut found = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("Archive") {
                found = true;
            }
            Ok(())
        });
        found
    })
}

/// The return type of a `#[wasm_zero]` function (the macro guarantees one).
fn return_type(f: &ItemFn) -> &Type {
    match &f.sig.output {
        ReturnType::Type(_, ty) => ty,
        ReturnType::Default => {
            panic!("wasm_zero_build: `{}` has no return type", f.sig.ident)
        }
    }
}

/// Render the full bindings module. With `typed`, emits a `.ts` (rkyv-js
/// `r.Infer<>` types + type annotations); otherwise a runtime-only `.js`.
fn render_module(
    typed: bool,
    structs: &[StructDef],
    names: &BTreeSet<String>,
    funcs: &[&ItemFn],
) -> String {
    // `t(s)` includes a TypeScript-only fragment when generating the `.ts`.
    let t = |s: &str| if typed { s.to_string() } else { String::new() };

    let mut out = String::new();
    out.push_str("// Auto-generated by wasm_zero. Do not edit by hand.\n");
    out.push_str("import * as r from 'rkyv-js';\n\n");

    // ---- FFI runtime ----
    out.push_str(
        "export const ErrorCode = Object.freeze({ Ok: 0, UnarchivingError: 254, ArchivingError: 255 });\n\n\
         export class WasmError extends Error {\n\
         \x20 constructor(code) {\n\
         \x20   super(`wasm_zero call failed with ErrorCode ${code}`);\n\
         \x20   this.name = 'WasmError';\n\
         \x20   this.code = code;\n\
         \x20 }\n\
         }\n\n\
         // Scratch buffer the shim writes its `[len][payload]` into.\n\
         export const MAX_BUFFER_SIZE = 64 * 1024;\n\n",
    );

    // ---- rkyv-js codecs ----
    out.push_str("// ---- codecs ----\n");
    for (name, fields) in structs {
        out.push_str(&format!("export const Archived{name} = r.struct({{\n"));
        for (field, ty) in fields {
            out.push_str(&format!("  {field}: {},\n", codec_expr(ty, names)));
        }
        out.push_str("});\n");
        out.push_str(&t(&format!(
            "export type {name} = r.Infer<typeof Archived{name}>;\n"
        )));
        out.push('\n');
    }

    // ---- FFI client ----
    out.push_str("// ---- client ----\n");
    out.push_str(&format!(
        "export function bindWasmZero(wasm{}) {{\n\
         \x20 function call(shim{}, codec{}) {{\n\
         \x20   const outPtr = wasm.malloc(4 + MAX_BUFFER_SIZE);\n\
         \x20   try {{\n\
         \x20     const code = wasm[shim](outPtr);\n\
         \x20     if (code !== ErrorCode.Ok) throw new WasmError(code);\n\
         \x20     // Re-read memory after the call — malloc may have grown it.\n\
         \x20     const view = new DataView(wasm.memory.buffer);\n\
         \x20     const len = view.getUint32(outPtr, true);\n\
         \x20     // Copy the archive out before freeing the scratch buffer.\n\
         \x20     const bytes = new Uint8Array(wasm.memory.buffer, outPtr + 4, len).slice();\n\
         \x20     return r.decode(codec, bytes);\n\
         \x20   }} finally {{\n\
         \x20     wasm.free(outPtr, 4 + MAX_BUFFER_SIZE);\n\
         \x20   }}\n\
         \x20 }}\n\n\
         \x20 return {{\n\
         \x20   wasm,\n",
        t(": any"),
        t(": string"),
        t(": any"),
    ));
    for f in funcs {
        let name = f.sig.ident.to_string();
        let shim = format!("__wasm_zero_{name}");
        let ret = return_type(f);
        let codec = codec_expr(ret, names);
        let ann = t(&format!(": {}", ts_type(ret, names)));
        out.push_str(&format!(
            "    {name}(){ann} {{ return call({shim:?}, {codec}); }},\n"
        ));
    }
    out.push_str("  };\n}\n\n");

    out.push_str(&format!(
        "export async function initWasmZero(wasmUrl{}, imports{}) {{\n\
         \x20 const importObject =\n\
         \x20   imports ?? new Proxy({{}}, {{ get: () => new Proxy({{}}, {{ get: () => () => {{}} }}) }});\n\
         \x20 const {{ instance }} = await WebAssembly.instantiateStreaming(fetch(wasmUrl), importObject);\n\
         \x20 return bindWasmZero(instance.exports);\n\
         }}\n",
        t(": string | URL"),
        t("?: WebAssembly.Imports"),
    ));

    out
}

/// Map a Rust type to its rkyv-js codec expression (e.g. `r.option(r.string)`).
///
/// Follows the rkyv-js codec table. User structs resolve to their generated
/// `Archived<Name>` codec. Panics on types with no known codec, so an
/// unsupported field fails the build loudly rather than producing wrong code.
fn codec_expr(ty: &Type, structs: &BTreeSet<String>) -> String {
    match ty {
        Type::Reference(r) => codec_expr(&r.elem, structs),
        Type::Group(g) => codec_expr(&g.elem, structs),
        Type::Paren(p) => codec_expr(&p.elem, structs),
        Type::Tuple(t) if t.elems.is_empty() => "r.unit".to_string(),
        Type::Tuple(t) => {
            let inner: Vec<_> =
                t.elems.iter().map(|e| codec_expr(e, structs)).collect();
            format!("r.tuple({})", inner.join(", "))
        }
        Type::Array(a) => format!(
            "r.array({}, {})",
            codec_expr(&a.elem, structs),
            a.len.to_token_stream()
        ),
        Type::Path(_) => {
            let (ident, args) = path_parts(ty);
            let arg0 = |structs: &BTreeSet<String>| {
                codec_expr(args.first().expect("container needs a type arg"), structs)
            };
            match ident.as_str() {
                "Vec" | "ThinVec" | "SmallVec" | "TinyVec" | "ArrayVec" => {
                    format!("r.vec({})", arg0(structs))
                }
                "Option" => format!("r.option({})", arg0(structs)),
                "Box" => format!("r.box({})", arg0(structs)),
                "Rc" | "Arc" => format!("r.rc({})", arg0(structs)),
                "Weak" => format!("r.weak({})", arg0(structs)),
                "u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "u64" | "i64"
                | "f32" | "f64" | "bool" | "char" => {
                    format!("r.{ident}")
                }
                "usize" => "r.u32".to_string(),
                "isize" => "r.i32".to_string(),
                "String" | "str" | "SmolStr" => "r.string".to_string(),
                name if structs.contains(name) => format!("Archived{name}"),
                other => panic!(
                    "wasm_zero_build: no rkyv-js codec known for type `{other}`. \
                     Supported: primitives, String, Vec/Option/Box/Rc/Arc/Weak, \
                     arrays, tuples, and #[derive(Archive)] structs."
                ),
            }
        }
        other => panic!(
            "wasm_zero_build: unsupported type `{}`",
            other.to_token_stream()
        ),
    }
}

/// Map a Rust type to its TypeScript type (mirrors the rkyv-js codec table).
fn ts_type(ty: &Type, structs: &BTreeSet<String>) -> String {
    match ty {
        Type::Reference(r) => ts_type(&r.elem, structs),
        Type::Group(g) => ts_type(&g.elem, structs),
        Type::Paren(p) => ts_type(&p.elem, structs),
        Type::Tuple(t) if t.elems.is_empty() => "null".to_string(),
        Type::Tuple(t) => {
            let inner: Vec<_> =
                t.elems.iter().map(|e| ts_type(e, structs)).collect();
            format!("[{}]", inner.join(", "))
        }
        Type::Array(a) => format!("{}[]", ts_type(&a.elem, structs)),
        Type::Path(_) => {
            let (ident, args) = path_parts(ty);
            let arg0 = || ts_type(args.first().expect("container needs a type arg"), structs);
            match ident.as_str() {
                "Vec" | "ThinVec" | "SmallVec" | "TinyVec" | "ArrayVec" => {
                    format!("{}[]", arg0())
                }
                "Option" | "Weak" => format!("{} | null", arg0()),
                "Box" | "Rc" | "Arc" => arg0(),
                "u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "f32" | "f64"
                | "usize" | "isize" => "number".to_string(),
                "u64" | "i64" | "u128" | "i128" => "bigint".to_string(),
                "bool" => "boolean".to_string(),
                "String" | "str" | "SmolStr" | "char" => "string".to_string(),
                name if structs.contains(name) => name.to_string(),
                other => other.to_string(),
            }
        }
        _ => "unknown".to_string(),
    }
}

/// Extract the last path segment's identifier and its generic type arguments.
fn path_parts(ty: &Type) -> (String, Vec<Type>) {
    if let Type::Path(p) = ty {
        if let Some(seg) = p.path.segments.last() {
            let args = match &seg.arguments {
                syn::PathArguments::AngleBracketed(a) => a
                    .args
                    .iter()
                    .filter_map(|g| match g {
                        syn::GenericArgument::Type(t) => Some(t.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            return (seg.ident.to_string(), args);
        }
    }
    (String::new(), Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        #[derive(Archive, Serialize, Deserialize)]
        pub struct Person {
            pub name: String,
            pub age: u32,
            pub email: Option<String>,
            pub scores: Vec<u32>,
        }

        #[wasm_zero]
        pub fn get_adult_person() -> Person { todo!() }

        #[wasm_zero]
        pub fn greet() -> String { todo!() }

        pub fn ignored() -> u32 { 0 }
    "#;

    #[test]
    fn generates_rkyv_js_codec() {
        let g = generate_bindings(SAMPLE).unwrap();
        assert!(g.ts.contains("export const ArchivedPerson = r.struct({"));
        assert!(g.ts.contains("name: r.string,"));
        assert!(g.ts.contains("age: r.u32,"));
        assert!(g.ts.contains("email: r.option(r.string),"));
        assert!(g.ts.contains("scores: r.vec(r.u32),"));
        // Infer type alias only in the .ts.
        assert!(g.ts.contains("export type Person = r.Infer<typeof ArchivedPerson>;"));
        assert!(!g.js.contains("r.Infer"));
    }

    #[test]
    fn generates_client() {
        let g = generate_bindings(SAMPLE).unwrap();
        assert!(g.js.contains(
            "get_adult_person() { return call(\"__wasm_zero_get_adult_person\", ArchivedPerson); }"
        ));
        assert!(g
            .js
            .contains("greet() { return call(\"__wasm_zero_greet\", r.string); }"));
        assert!(g.ts.contains("get_adult_person(): Person {"));
        assert!(g.ts.contains("greet(): string {"));
        assert!(g.js.contains("export async function initWasmZero"));
        assert!(g.js.contains("r.decode(codec, bytes)"));
        assert!(!g.js.contains("ignored"));
    }
}
