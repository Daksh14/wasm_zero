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

/// A `#[wasm_zero]` function's arguments as `(js_name, type)` pairs.
fn fn_args(f: &ItemFn) -> Vec<(String, Type)> {
    f.sig
        .inputs
        .iter()
        .enumerate()
        .filter_map(|(i, input)| match input {
            syn::FnArg::Typed(pt) => {
                let name = match &*pt.pat {
                    syn::Pat::Ident(id) => id.ident.to_string(),
                    _ => format!("arg{i}"),
                };
                Some((name, (*pt.ty).clone()))
            }
            syn::FnArg::Receiver(_) => None,
        })
        .collect()
}

/// The rkyv-js codec for a function's argument list: `null` when nullary, the
/// bare codec for one arg, or `r.tuple(...)` for several.
fn args_codec(args: &[(String, Type)], names: &BTreeSet<String>) -> String {
    match args {
        [] => "null".to_string(),
        [(_, ty)] => codec_expr(ty, names),
        many => format!(
            "r.tuple({})",
            many.iter()
                .map(|(_, ty)| codec_expr(ty, names))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// If `ty` is a `Vec<P>` of a fixed-width numeric primitive `P`, return the JS
/// TypedArray constructor for a zero-copy view over its archived elements.
/// (The archived elements are native little-endian and contiguous, so they map
/// directly onto a typed array.)
fn numeric_vec_view(ty: &Type) -> Option<&'static str> {
    let (ident, args) = path_parts(ty);
    if !matches!(
        ident.as_str(),
        "Vec" | "ThinVec" | "SmallVec" | "TinyVec" | "ArrayVec"
    ) {
        return None;
    }
    let (elem, _) = path_parts(args.first()?);
    Some(match elem.as_str() {
        "u8" => "Uint8Array",
        "i8" => "Int8Array",
        "u16" => "Uint16Array",
        "i16" => "Int16Array",
        "u32" => "Uint32Array",
        "i32" => "Int32Array",
        "f32" => "Float32Array",
        "f64" => "Float64Array",
        "u64" => "BigUint64Array",
        "i64" => "BigInt64Array",
        _ => return None,
    })
}

/// True if `ty` is a primitive passed directly as a wasm scalar argument
/// (matches `wasm_zero_macro::is_direct_scalar`).
fn is_scalar(ty: &Type) -> bool {
    let (ident, args) = path_parts(ty);
    args.is_empty()
        && matches!(
            ident.as_str(),
            "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64"
                | "f32" | "f64" | "usize" | "isize" | "bool"
        )
}

/// Render one method on the bound object. Argument passing has three modes:
/// nullary, all-scalar (passed directly as wasm params), or rkyv-encoded into
/// the input buffer. `Vec<numeric>` returns use the zero-copy `callView` path;
/// everything else uses lazy `call`.
fn render_method(
    f: &ItemFn,
    names: &BTreeSet<String>,
    typed: bool,
) -> String {
    let name = f.sig.ident.to_string();
    let shim = format!("__wasm_zero_{name}");
    let ret = return_type(f);
    let args = fn_args(f);

    let names_csv =
        || args.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ");

    let params = if typed {
        args.iter()
            .map(|(n, ty)| format!("{n}: {}", ts_type(ty, names)))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        names_csv()
    };

    // Fully-scalar fast path: scalar/unit return with nullary or all-scalar
    // args. The shim is a plain wasm export, so we call it directly — no buffer,
    // no rkyv-js, matching wasm_bindgen.
    let unit_ret = matches!(ret, Type::Tuple(t) if t.elems.is_empty());
    let scalar_args = !args.is_empty() && args.iter().all(|(_, t)| is_scalar(t));
    if (unit_ret || is_scalar(ret)) && (args.is_empty() || scalar_args) {
        let call = format!("wasm[{shim:?}]({})", names_csv());
        if unit_ret {
            let ann = if typed { ": void" } else { "" };
            return format!("    {name}({params}){ann} {{ {call}; }},\n");
        }
        let ret_ann = if typed {
            format!(": {}", ts_type(ret, names))
        } else {
            String::new()
        };
        // `bool` returns come back as a wasm i32 (0/1).
        let expr = if path_parts(ret).0 == "bool" {
            format!("{call} !== 0")
        } else {
            call
        };
        return format!("    {name}({params}){ret_ann} {{ return {expr}; }},\n");
    }

    // (argCodec, argValue, directArgs) for the chosen arg-passing mode.
    let (arg_codec, arg_value, direct_args) = if args.is_empty() {
        ("null".to_string(), "null".to_string(), "null".to_string())
    } else if args.iter().all(|(_, ty)| is_scalar(ty)) {
        // Scalars go straight through as wasm params.
        ("null".to_string(), "null".to_string(), format!("[{}]", names_csv()))
    } else {
        // Non-scalar args: rkyv-encode (bare for one, array for several).
        let value = match args.as_slice() {
            [(n, _)] => n.clone(),
            _ => format!("[{}]", names_csv()),
        };
        (args_codec(&args, names), value, "null".to_string())
    };

    if let Some(ctor) = numeric_vec_view(ret) {
        let ret_ann = if typed { format!(": {ctor}") } else { String::new() };
        return format!(
            "    {name}({params}){ret_ann} {{ return callView({shim:?}, {arg_codec}, {arg_value}, {ctor}, {direct_args}); }},\n"
        );
    }

    let ret_codec = codec_expr(ret, names);
    let ret_ann = if typed {
        format!(": {}", ts_type(ret, names))
    } else {
        String::new()
    };
    format!(
        "    {name}({params}){ret_ann} {{ return call({shim:?}, {ret_codec}, {arg_codec}, {arg_value}, {direct_args}); }},\n"
    )
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
         export const MAX_BUFFER_SIZE = 64 * 1024;\n\n\
         // Buffer framing: [len: u32 @0][archive @ HEADER]. Must match\n\
         // wasm_zero::mem::HEADER (16 — keeps the archive 16-aligned so\n\
         // zero-copy typed-array views are correctly aligned).\n\
         const HEADER = 16;\n\n",
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
         \x20 // Persistent scratch buffers, allocated once and reused across\n\
         \x20 // calls to avoid a malloc/free per call. Not re-entrant: a call\n\
         \x20 // must finish before the next one on the same instance.\n\
         \x20 const outPtr = wasm.malloc(HEADER + MAX_BUFFER_SIZE);\n\
         \x20 let inPtr = 0; // allocated lazily on first call that takes args\n\n\
         \x20 // Cached views over wasm memory, rebuilt only when a memory.grow\n\
         \x20 // replaces (detaches) the underlying ArrayBuffer.\n\
         \x20 let buf = wasm.memory.buffer;\n\
         \x20 let dv = new DataView(buf);\n\
         \x20 let u8 = new Uint8Array(buf);\n\
         \x20 function views() {{\n\
         \x20   if (buf !== wasm.memory.buffer) {{\n\
         \x20     buf = wasm.memory.buffer;\n\
         \x20     dv = new DataView(buf);\n\
         \x20     u8 = new Uint8Array(buf);\n\
         \x20   }}\n\
         \x20 }}\n\n\
         \x20 // Run the shim. Scalar args (directArgs) are passed straight as\n\
         \x20 // wasm params; non-scalar args are rkyv-encoded into the input buffer.\n\
         \x20 function invoke(shim{}, argCodec{}, argValue{}, directArgs{}) {{\n\
         \x20   if (directArgs !== null) return wasm[shim](...directArgs, outPtr);\n\
         \x20   if (argCodec === null) return wasm[shim](outPtr);\n\
         \x20   const inBytes = r.encode(argCodec, argValue);\n\
         \x20   if (inBytes.length > MAX_BUFFER_SIZE)\n\
         \x20     throw new RangeError(`wasm_zero: input ${{inBytes.length}} bytes exceeds MAX_BUFFER_SIZE`);\n\
         \x20   if (inPtr === 0) inPtr = wasm.malloc(HEADER + MAX_BUFFER_SIZE);\n\
         \x20   views(); // malloc may have grown memory\n\
         \x20   dv.setUint32(inPtr, inBytes.length, true);\n\
         \x20   u8.set(inBytes, inPtr + HEADER);\n\
         \x20   return wasm[shim](inPtr, outPtr);\n\
         \x20 }}\n\n\
         \x20 // Eagerly decode the output archive into an owned JS value (one\n\
         \x20 // pass; reads the view but copies fields out, so the result is\n\
         \x20 // safe to keep across later calls). Numeric Vec returns instead use\n\
         \x20 // callView for a zero-copy typed-array view.\n\
         \x20 function call(shim{}, retCodec{}, argCodec{}, argValue{}, directArgs{}) {{\n\
         \x20   const code = invoke(shim, argCodec, argValue, directArgs);\n\
         \x20   if (code !== ErrorCode.Ok) throw new WasmError(code);\n\
         \x20   views(); // the call may have grown memory\n\
         \x20   const len = dv.getUint32(outPtr, true);\n\
         \x20   return r.decode(retCodec, u8.subarray(outPtr + HEADER, outPtr + HEADER + len));\n\
         \x20 }}\n\n\
         \x20 // Zero-copy view over an archived Vec<numeric> in wasm memory.\n\
         \x20 // WARNING: aliases the shared scratch buffer — only valid until the\n\
         \x20 // next call on this instance (or a memory.grow). Copy if you need to\n\
         \x20 // keep it: e.g. `wasmzero.foo().slice()`.\n\
         \x20 function callView(shim{}, argCodec{}, argValue{}, Ctor{}, directArgs{}) {{\n\
         \x20   const code = invoke(shim, argCodec, argValue, directArgs);\n\
         \x20   if (code !== ErrorCode.Ok) throw new WasmError(code);\n\
         \x20   views();\n\
         \x20   const archiveLen = dv.getUint32(outPtr, true);\n\
         \x20   const base = outPtr + HEADER;\n\
         \x20   const rootPos = base + archiveLen - 8; // ArchivedVec is 8 bytes\n\
         \x20   const off = dv.getInt32(rootPos, true);\n\
         \x20   const len = dv.getUint32(rootPos + 4, true);\n\
         \x20   return new Ctor(wasm.memory.buffer, rootPos + off, len);\n\
         \x20 }}\n\n\
         \x20 return {{\n\
         \x20   wasm,\n",
        t(": any"),
        t(": string"), t(": any"), t(": any"), t(": any"),
        t(": string"), t(": any"), t(": any"), t(": any"), t(": any"),
        t(": string"), t(": any"), t(": any"), t(": any"), t(": any"),
    ));
    for f in funcs {
        out.push_str(&render_method(f, names, typed));
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

        #[wasm_zero]
        pub fn add(a: i32, b: i32) -> i32 { todo!() }

        #[wasm_zero]
        pub fn tick() -> () { todo!() }

        #[wasm_zero]
        pub fn count() -> u32 { todo!() }

        #[wasm_zero]
        pub fn shout(msg: String) -> String { todo!() }

        #[wasm_zero]
        pub fn nums() -> Vec<u32> { todo!() }

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
        // Nullary methods pass `null` for codec/value/directArgs.
        assert!(g.js.contains(
            "get_adult_person() { return call(\"__wasm_zero_get_adult_person\", ArchivedPerson, null, null, null); }"
        ));
        assert!(g.js.contains(
            "greet() { return call(\"__wasm_zero_greet\", r.string, null, null, null); }"
        ));
        // Scalar return + scalar args: a plain wasm call, no buffer/rkyv.
        assert!(g.js.contains(
            "add(a, b) { return wasm[\"__wasm_zero_add\"](a, b); }"
        ));
        // Scalar return, nullary: direct call.
        assert!(g.js.contains(
            "count() { return wasm[\"__wasm_zero_count\"](); }"
        ));
        // Unit return: direct call, no return value.
        assert!(g.js.contains("tick() { wasm[\"__wasm_zero_tick\"](); }"));
        assert!(g.ts.contains("tick(): void {"));
        // Non-scalar args are rkyv-encoded into the input buffer.
        assert!(g.js.contains(
            "shout(msg) { return call(\"__wasm_zero_shout\", r.string, r.string, msg, null); }"
        ));
        assert!(g.ts.contains("add(a: number, b: number): number {"));
        assert!(g.ts.contains("get_adult_person(): Person {"));
        assert!(g.js.contains("export async function initWasmZero"));
        // Struct/string returns are eagerly decoded into owned values.
        assert!(g.js.contains("r.decode(retCodec, u8.subarray(outPtr + HEADER, outPtr + HEADER + len))"));
        assert!(g.js.contains("r.encode(argCodec, argValue)"));
        // Scratch buffer is allocated once in bindWasmZero, not per call.
        assert!(g.js.contains("const outPtr = wasm.malloc(HEADER + MAX_BUFFER_SIZE);"));
        assert_eq!(g.js.matches("wasm.malloc(HEADER + MAX_BUFFER_SIZE)").count(), 2);
        // Memory views are cached and rebuilt only on grow.
        assert!(g.js.contains("function views()"));
        // Vec<numeric> returns use the zero-copy typed-array view path.
        assert!(g.js.contains(
            "nums() { return callView(\"__wasm_zero_nums\", null, null, Uint32Array, null); }"
        ));
        assert!(g.ts.contains("nums(): Uint32Array {"));
        assert!(!g.js.contains("ignored"));
    }
}
