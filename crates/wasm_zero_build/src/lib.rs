//! Build-helper for `wasm_zero`.
//!
//! Runs in a consumer crate's `build.rs` (the proc macro can't write files; a
//! build script can). It scans the source for rkyv structs and `#[wasm_zero]`
//! functions and emits `bindings.ts` + `bindings.js`.
//!
//! The read path is **self-contained and zero-copy**: for each struct it emits
//! a `decode_<Struct>` function that reads each field straight out of wasm
//! memory at its archived offset — scalars as `DataView` reads, numeric vecs as
//! typed-array views, strings transcoded on demand, options inline. No rkyv-js
//! runtime is needed to decode.
//!
//! rkyv-js is imported only when a function takes a **non-scalar argument**
//! (`String`/struct/…), to `r.encode` it into the input buffer (hand-rolling
//! the rkyv *writer* is out of scope). Scalar args pass directly as wasm params,
//! and scalar/unit returns come back as the wasm function's value.
//!
//! ```no_run
//! fn main() { wasm_zero_build::generate("src/lib.rs", "pkg"); }
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use syn::{Fields, Item, ItemFn, ItemStruct, ReturnType, Type};

/// Struct name -> its fields (in declaration order).
type StructMap = BTreeMap<String, Vec<(String, Type)>>;

/// The two files to write.
pub struct Generated {
    pub ts: String,
    pub js: String,
}

/// Scan `src` and write `bindings.ts` + `bindings.js` into `out_dir`.
pub fn generate(src: impl AsRef<Path>, out_dir: impl AsRef<Path>) {
    let src = src.as_ref();
    let out_dir = out_dir.as_ref();

    let source = std::fs::read_to_string(src).unwrap_or_else(|e| {
        panic!("wasm_zero_build: failed to read {}: {e}", src.display())
    });
    let g = generate_bindings(&source).unwrap_or_else(|e| {
        panic!("wasm_zero_build: failed to parse {}: {e}", src.display())
    });

    std::fs::create_dir_all(out_dir).unwrap_or_else(|e| {
        panic!("wasm_zero_build: failed to create {}: {e}", out_dir.display())
    });
    for (name, contents) in [("bindings.ts", &g.ts), ("bindings.js", &g.js)] {
        let path = out_dir.join(name);
        std::fs::write(&path, contents).unwrap_or_else(|e| {
            panic!("wasm_zero_build: failed to write {}: {e}", path.display())
        });
    }
    println!("cargo:rerun-if-changed={}", src.display());
}

/// Parse `source` and render both files. Pure — unit-testable.
pub fn generate_bindings(source: &str) -> syn::Result<Generated> {
    let file = syn::parse_file(source)?;

    let mut structs: StructMap = BTreeMap::new();
    for item in &file.items {
        if let Item::Struct(s) = item {
            if derives_archive(s) {
                if let Fields::Named(f) = &s.fields {
                    structs.insert(
                        s.ident.to_string(),
                        f.named
                            .iter()
                            .map(|f| {
                                (f.ident.as_ref().unwrap().to_string(), f.ty.clone())
                            })
                            .collect(),
                    );
                }
            }
        }
    }

    let funcs: Vec<&ItemFn> = file
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Fn(f) if has_wasm_zero_attr(f) => Some(f),
            _ => None,
        })
        .collect();

    Ok(Generated {
        ts: render_module(true, &structs, &funcs),
        js: render_module(false, &structs, &funcs),
    })
}

fn has_wasm_zero_attr(f: &ItemFn) -> bool {
    f.attrs.iter().any(|a| a.path().is_ident("wasm_zero"))
}

fn derives_archive(s: &ItemStruct) -> bool {
    s.attrs.iter().any(|attr| {
        if !attr.path().is_ident("derive") {
            return false;
        }
        let mut found = false;
        let _ = attr.parse_nested_meta(|m| {
            if m.path.is_ident("Archive") {
                found = true;
            }
            Ok(())
        });
        found
    })
}

fn return_type(f: &ItemFn) -> &Type {
    match &f.sig.output {
        ReturnType::Type(_, ty) => ty,
        ReturnType::Default => {
            panic!("wasm_zero_build: `{}` has no return type", f.sig.ident)
        }
    }
}

/// A `#[wasm_zero]` function's args as `(js_name, type)`.
fn fn_args(f: &ItemFn) -> Vec<(String, Type)> {
    f.sig
        .inputs
        .iter()
        .enumerate()
        .filter_map(|(i, a)| match a {
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

// ---------------------------------------------------------------------------
// Archived layout: each field's (size, align), used only to compute offsets.
// ---------------------------------------------------------------------------

fn align_up(x: u32, a: u32) -> u32 {
    (x + a - 1) / a * a
}

/// Last path-segment ident + its generic type args.
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

fn unref(ty: &Type) -> &Type {
    match ty {
        Type::Reference(r) => unref(&r.elem),
        Type::Paren(p) => unref(&p.elem),
        Type::Group(g) => unref(&g.elem),
        other => other,
    }
}

fn scalar_size_align(ident: &str) -> Option<(u32, u32)> {
    Some(match ident {
        "bool" | "u8" | "i8" => (1, 1),
        "u16" | "i16" => (2, 2),
        "u32" | "i32" | "f32" | "usize" | "isize" | "char" => (4, 4),
        "u64" | "i64" | "f64" => (8, 8),
        "u128" | "i128" => (16, 16),
        _ => return None,
    })
}

/// (size, align) of the archived form of `ty`.
fn archived(ty: &Type, structs: &StructMap) -> (u32, u32) {
    let ty = unref(ty);
    let (id, args) = path_parts(ty);
    if let Some(sa) = scalar_size_align(&id) {
        return sa;
    }
    match id.as_str() {
        "String" | "str" | "SmolStr" => (8, 4),
        "Vec" | "ThinVec" | "SmallVec" | "TinyVec" | "ArrayVec" => (8, 4),
        "Box" | "Rc" | "Arc" => (4, 4),
        "Option" => {
            let (s, a) = archived(arg0(&args), structs);
            (align_up(align_up(1, a) + s, a), a)
        }
        name if structs.contains_key(name) => struct_layout(&structs[name], structs),
        other => panic!(
            "wasm_zero_build: cannot read type `{other}`. Supported: scalars, \
             String, Option, Vec, Box/Rc/Arc, and #[derive(Archive)] structs."
        ),
    }
}

fn struct_layout(fields: &[(String, Type)], structs: &StructMap) -> (u32, u32) {
    let mut off = 0u32;
    let mut align = 1u32;
    for (_, ty) in fields {
        let (s, a) = archived(ty, structs);
        off = align_up(off, a) + s;
        align = align.max(a);
    }
    (align_up(off, align), align)
}

fn arg0(args: &[Type]) -> &Type {
    args.first().expect("wasm_zero_build: generic type needs an argument")
}

// ---------------------------------------------------------------------------
// JS field readers — a read expression per field, reading at address `addr`.
// ---------------------------------------------------------------------------

/// TypedArray constructor for a numeric primitive element type (for vec views).
fn numeric_ctor(ty: &Type) -> Option<&'static str> {
    let (id, args) = path_parts(unref(ty));
    if !args.is_empty() {
        return None;
    }
    Some(match id.as_str() {
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

/// JS expression that reads a value of `ty` at byte address `addr`.
fn read_field(ty: &Type, structs: &StructMap, addr: &str) -> String {
    let ty = unref(ty);
    let (id, args) = path_parts(ty);
    match id.as_str() {
        "bool" => format!("(u8[{addr}] !== 0)"),
        "u8" => format!("u8[{addr}]"),
        "i8" => format!("dv.getInt8({addr})"),
        "u16" => format!("dv.getUint16({addr}, true)"),
        "i16" => format!("dv.getInt16({addr}, true)"),
        "u32" | "usize" => format!("dv.getUint32({addr}, true)"),
        "i32" | "isize" => format!("dv.getInt32({addr}, true)"),
        "u64" => format!("dv.getBigUint64({addr}, true)"),
        "i64" => format!("dv.getBigInt64({addr}, true)"),
        "f32" => format!("dv.getFloat32({addr}, true)"),
        "f64" => format!("dv.getFloat64({addr}, true)"),
        "char" => format!("String.fromCodePoint(dv.getUint32({addr}, true))"),
        "String" | "str" | "SmolStr" => format!("rdStr(dv, u8, {addr})"),
        "Vec" | "ThinVec" | "SmallVec" | "TinyVec" | "ArrayVec" => {
            let el = arg0(&args);
            if let Some(ctor) = numeric_ctor(el) {
                format!("rdVec(dv, {addr}, {ctor})")
            } else {
                let stride = archived(el, structs).0;
                format!(
                    "rdVecOf(dv, u8, {addr}, {stride}, (a) => {})",
                    read_field(el, structs, "a")
                )
            }
        }
        "Option" => {
            let el = arg0(&args);
            let payload = align_up(1, archived(el, structs).1);
            format!(
                "(u8[{addr}] === 0 ? null : {})",
                read_field(el, structs, &format!("{addr} + {payload}"))
            )
        }
        "Box" | "Rc" | "Arc" => {
            // Relative pointer at `addr`; target at addr + offset.
            read_field(arg0(&args), structs, &format!("({addr}) + dv.getInt32({addr}, true)"))
        }
        name if structs.contains_key(name) => {
            format!("decode_{name}(dv, u8, {addr})")
        }
        other => panic!("wasm_zero_build: no JS reader for `{other}`"),
    }
}

/// TypeScript type for `ty` as produced by the readers above.
fn ts_type(ty: &Type, structs: &StructMap) -> String {
    let ty = unref(ty);
    let (id, args) = path_parts(ty);
    match id.as_str() {
        "Vec" | "ThinVec" | "SmallVec" | "TinyVec" | "ArrayVec" => {
            if let Some(ctor) = numeric_ctor(arg0(&args)) {
                ctor.to_string() // a typed-array view
            } else {
                format!("{}[]", ts_type(arg0(&args), structs))
            }
        }
        "Option" => format!("{} | null", ts_type(arg0(&args), structs)),
        "Box" | "Rc" | "Arc" => ts_type(arg0(&args), structs),
        "u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "f32" | "f64" | "usize"
        | "isize" => "number".to_string(),
        "u64" | "i64" | "u128" | "i128" => "bigint".to_string(),
        "bool" => "boolean".to_string(),
        "char" | "String" | "str" | "SmolStr" => "string".to_string(),
        name if structs.contains_key(name) => name.to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// rkyv-js codecs — only for non-scalar *argument* encoding.
// ---------------------------------------------------------------------------

fn is_scalar(ty: &Type) -> bool {
    scalar_size_align(&path_parts(unref(ty)).0).is_some()
}

fn codec_expr(ty: &Type, structs: &StructMap) -> String {
    let ty = unref(ty);
    let (id, args) = path_parts(ty);
    match id.as_str() {
        "Vec" | "ThinVec" | "SmallVec" | "TinyVec" | "ArrayVec" => {
            format!("r.vec({})", codec_expr(arg0(&args), structs))
        }
        "Option" => format!("r.option({})", codec_expr(arg0(&args), structs)),
        "Box" => format!("r.box({})", codec_expr(arg0(&args), structs)),
        "Rc" | "Arc" => format!("r.rc({})", codec_expr(arg0(&args), structs)),
        "u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "u64" | "i64" | "f32"
        | "f64" | "bool" | "char" => format!("r.{id}"),
        "usize" => "r.u32".to_string(),
        "isize" => "r.i32".to_string(),
        "String" | "str" | "SmolStr" => "r.string".to_string(),
        name if structs.contains_key(name) => format!("Archived{name}"),
        other => panic!("wasm_zero_build: no rkyv-js codec for arg type `{other}`"),
    }
}

fn args_codec(args: &[(String, Type)], structs: &StructMap) -> String {
    match args {
        [] => "null".to_string(),
        [(_, ty)] => codec_expr(ty, structs),
        many => format!(
            "r.tuple({})",
            many.iter()
                .map(|(_, t)| codec_expr(t, structs))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

// ---------------------------------------------------------------------------
// Module rendering
// ---------------------------------------------------------------------------

fn render_module(typed: bool, structs: &StructMap, funcs: &[&ItemFn]) -> String {
    let t = |s: &str| if typed { s.to_string() } else { String::new() };

    // rkyv-js is needed only to encode non-scalar arguments.
    let needs_rkyv = funcs
        .iter()
        .any(|f| fn_args(f).iter().any(|(_, ty)| !is_scalar(ty)));

    let mut out = String::new();
    out.push_str("// Auto-generated by wasm_zero. Do not edit by hand.\n");
    if needs_rkyv {
        out.push_str("import * as r from 'rkyv-js';\n");
    }
    out.push('\n');

    out.push_str(
        "export const ErrorCode = Object.freeze({ Ok: 0, UnarchivingError: 254, ArchivingError: 255 });\n\n\
         export class WasmError extends Error {\n\
         \x20 constructor(code) {\n\
         \x20   super(`wasm_zero call failed with ErrorCode ${code}`);\n\
         \x20   this.name = 'WasmError';\n\
         \x20   this.code = code;\n\
         \x20 }\n\
         }\n\n\
         export const MAX_BUFFER_SIZE = 512 * 1024;\n\
         const HEADER = 16; // archive offset; must match wasm_zero::mem::HEADER\n\n\
         // ---- zero-copy readers (read straight from wasm memory) ----\n\
         const __td = new TextDecoder();\n\
         function rdStr(dv, u8, a) {\n\
         \x20 if ((u8[a] & 0xc0) !== 0x80) { // inline (SSO)\n\
         \x20   let n = 0; while (n < 8 && u8[a + n] !== 0xff) n++;\n\
         \x20   return __td.decode(u8.subarray(a, a + n));\n\
         \x20 }\n\
         \x20 const raw = dv.getUint32(a, true);\n\
         \x20 const len = (raw & 0x3f) | ((raw & 0xffffff00) >>> 2);\n\
         \x20 const off = dv.getInt32(a + 4, true);\n\
         \x20 return __td.decode(u8.subarray(a + off, a + off + len));\n\
         }\n\
         function rdVec(dv, a, Ctor) { // zero-copy typed-array view\n\
         \x20 const off = dv.getInt32(a, true), len = dv.getUint32(a + 4, true);\n\
         \x20 return new Ctor(dv.buffer, a + off, len);\n\
         }\n\
         function rdVecOf(dv, u8, a, stride, rd) {\n\
         \x20 const off = dv.getInt32(a, true), len = dv.getUint32(a + 4, true), b = a + off;\n\
         \x20 const out = new Array(len);\n\
         \x20 for (let i = 0; i < len; i++) out[i] = rd(b + i * stride);\n\
         \x20 return out;\n\
         }\n\n",
    );

    // rkyv-js codecs for non-scalar argument types only.
    if needs_rkyv {
        out.push_str("// ---- codecs (for non-scalar argument encoding) ----\n");
        for (name, fields) in structs {
            out.push_str(&format!("export const Archived{name} = r.struct({{\n"));
            for (field, ty) in fields {
                out.push_str(&format!("  {field}: {},\n", codec_expr(ty, structs)));
            }
            out.push_str("});\n\n");
        }
    }

    // One decoder per struct: reads each field at its archived offset.
    out.push_str("// ---- per-struct field readers ----\n");
    for (name, fields) in structs {
        out.push_str(&format!("function decode_{name}(dv, u8, p) {{\n  return {{\n"));
        let mut off = 0u32;
        for (field, ty) in fields {
            let (s, a) = archived(ty, structs);
            off = align_up(off, a);
            out.push_str(&format!(
                "    {field}: {},\n",
                read_field(ty, structs, &format!("p + {off}"))
            ));
            off += s;
        }
        out.push_str("  };\n}\n\n");
    }

    if typed {
        for (name, fields) in structs {
            out.push_str(&format!("export interface {name} {{\n"));
            for (field, ty) in fields {
                out.push_str(&format!("  {field}: {};\n", ts_type(ty, structs)));
            }
            out.push_str("}\n\n");
        }
    }

    // ---- client ----
    out.push_str(&format!(
        "export function bindWasmZero(wasm{}) {{\n\
         \x20 const outPtr = wasm.malloc(HEADER + MAX_BUFFER_SIZE);\n\
         \x20 let inPtr = 0; // lazily allocated for non-scalar args\n\
         \x20 let buf = wasm.memory.buffer, dv = new DataView(buf), u8 = new Uint8Array(buf);\n\
         \x20 function views() {{ if (buf !== wasm.memory.buffer) {{ buf = wasm.memory.buffer; dv = new DataView(buf); u8 = new Uint8Array(buf); }} }}\n\n\
         \x20 function invoke(shim{}, argCodec{}, argValue{}, directArgs{}) {{\n\
         \x20   if (directArgs !== null) return wasm[shim](...directArgs, outPtr);\n\
         \x20   if (argCodec === null) return wasm[shim](outPtr);\n",
        t(": any"), t(": string"), t(": any"), t(": any"), t(": any"),
    ));
    if needs_rkyv {
        out.push_str(
            "         \x20   const inBytes = r.encode(argCodec, argValue);\n\
             \x20   if (inBytes.length > MAX_BUFFER_SIZE) throw new RangeError('wasm_zero: input too large');\n\
             \x20   if (inPtr === 0) inPtr = wasm.malloc(HEADER + MAX_BUFFER_SIZE);\n\
             \x20   views();\n\
             \x20   dv.setUint32(inPtr, inBytes.length, true);\n\
             \x20   u8.set(inBytes, inPtr + HEADER);\n\
             \x20   return wasm[shim](inPtr, outPtr);\n",
        );
    } else {
        out.push_str("         \x20   throw new Error('unreachable: no non-scalar args');\n");
    }
    out.push_str(&format!(
        "         \x20 }}\n\n\
         \x20 // Run the shim, then read the result straight from wasm memory at the\n\
         \x20 // archive root. Struct/vec results alias the scratch buffer — valid\n\
         \x20 // until the next call (or memory.grow); copy if you need to keep them.\n\
         \x20 function call(shim{}, size{}, read{}, argCodec{}, argValue{}, directArgs{}) {{\n\
         \x20   const code = invoke(shim, argCodec, argValue, directArgs);\n\
         \x20   if (code !== ErrorCode.Ok) throw new WasmError(code);\n\
         \x20   views();\n\
         \x20   const len = dv.getUint32(outPtr, true);\n\
         \x20   return read(dv, u8, outPtr + HEADER + len - size);\n\
         \x20 }}\n\n\
         \x20 return {{\n\
         \x20   wasm,\n",
        t(": string"), t(": number"), t(": any"), t(": any"), t(": any"), t(": any"),
    ));
    for f in funcs {
        out.push_str(&render_method(f, structs, typed));
    }
    out.push_str("  };\n}\n\n");

    out.push_str(&format!(
        "export async function initWasmZero(wasmUrl{}, imports{}) {{\n\
         \x20 const importObject =\n\
         \x20   imports ?? new Proxy({{}}, {{ get: () => new Proxy({{}}, {{ get: () => () => {{}} }}) }});\n\
         \x20 const {{ instance }} = await WebAssembly.instantiateStreaming(fetch(wasmUrl), importObject);\n\
         \x20 return bindWasmZero(instance.exports);\n\
         }}\n",
        t(": string | URL"), t("?: WebAssembly.Imports"),
    ));

    out
}

/// Render one method on the bound object.
fn render_method(f: &ItemFn, structs: &StructMap, typed: bool) -> String {
    let name = f.sig.ident.to_string();
    let shim = format!("__wasm_zero_{name}");
    let ret = return_type(f);
    let args = fn_args(f);

    let names_csv =
        || args.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ");
    let params = if typed {
        args.iter()
            .map(|(n, ty)| format!("{n}: {}", ts_type(ty, structs)))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        names_csv()
    };

    // Fully-scalar fast path: scalar/unit return + nullary-or-all-scalar args.
    let unit_ret = matches!(ret, Type::Tuple(t) if t.elems.is_empty());
    if (unit_ret || is_scalar(ret))
        && (args.is_empty() || args.iter().all(|(_, t)| is_scalar(t)))
    {
        let call = format!("wasm[{shim:?}]({})", names_csv());
        if unit_ret {
            let ann = if typed { ": void" } else { "" };
            return format!("    {name}({params}){ann} {{ {call}; }},\n");
        }
        let ann = if typed { format!(": {}", ts_type(ret, structs)) } else { String::new() };
        let expr = if path_parts(unref(ret)).0 == "bool" {
            format!("{call} !== 0")
        } else {
            call
        };
        return format!("    {name}({params}){ann} {{ return {expr}; }},\n");
    }

    // Arg mode: nullary / all-scalar (direct) / non-scalar (rkyv-encoded).
    let (arg_codec, arg_value, direct_args) = if args.is_empty() {
        ("null".into(), "null".into(), "null".into())
    } else if args.iter().all(|(_, t)| is_scalar(t)) {
        ("null".into(), "null".into(), format!("[{}]", names_csv()))
    } else {
        let value = match args.as_slice() {
            [(n, _)] => n.clone(),
            _ => format!("[{}]", names_csv()),
        };
        (args_codec(&args, structs), value, "null".into())
    };

    let size = archived(ret, structs).0;
    let read = read_field(ret, structs, "p");
    let ann = if typed { format!(": {}", ts_type(ret, structs)) } else { String::new() };
    format!(
        "    {name}({params}){ann} {{ return call({shim:?}, {size}, (dv, u8, p) => {read}, {arg_codec}, {arg_value}, {direct_args}); }},\n"
    )
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

        #[wasm_zero] pub fn get_adult_person() -> Person { todo!() }
        #[wasm_zero] pub fn greet() -> String { todo!() }
        #[wasm_zero] pub fn add(a: i32, b: i32) -> i32 { todo!() }
        #[wasm_zero] pub fn tick() -> () { todo!() }
        #[wasm_zero] pub fn nums() -> Vec<u32> { todo!() }
        pub fn ignored() -> u32 { 0 }
    "#;

    #[test]
    fn struct_field_offsets() {
        let g = generate_bindings(SAMPLE).unwrap();
        // ArchivedPerson: name@0(8) age@8(4) email@12(Option<String>=12) scores@24(8)
        assert!(g.js.contains("name: rdStr(dv, u8, p + 0)"));
        assert!(g.js.contains("age: dv.getUint32(p + 8, true)"));
        assert!(g.js.contains("email: (u8[p + 12] === 0 ? null : rdStr(dv, u8, p + 12 + 4))"));
        assert!(g.js.contains("scores: rdVec(dv, p + 24, Uint32Array)"));
        // No rkyv-js needed (no non-scalar args).
        assert!(!g.js.contains("import * as r from 'rkyv-js'"));
    }

    #[test]
    fn methods() {
        let g = generate_bindings(SAMPLE).unwrap();
        // scalar/unit fast paths
        assert!(g.js.contains("add(a, b) { return wasm[\"__wasm_zero_add\"](a, b); }"));
        assert!(g.js.contains("tick() { wasm[\"__wasm_zero_tick\"](); }"));
        // struct return: read at root via decode_Person (size 32)
        assert!(g.js.contains(
            "get_adult_person() { return call(\"__wasm_zero_get_adult_person\", 32, (dv, u8, p) => decode_Person(dv, u8, p), null, null, null); }"
        ));
        // numeric vec return: zero-copy view (size 8)
        assert!(g.js.contains(
            "nums() { return call(\"__wasm_zero_nums\", 8, (dv, u8, p) => rdVec(dv, p, Uint32Array), null, null, null); }"
        ));
        // plain TS interface, view-typed field
        assert!(g.ts.contains("export interface Person {"));
        assert!(g.ts.contains("scores: Uint32Array;"));
        assert!(g.ts.contains("email: string | null;"));
        assert!(!g.js.contains("ignored"));
    }

    #[test]
    fn non_scalar_arg_uses_rkyv() {
        let src = r#"
            #[derive(Archive)] pub struct P { pub x: u32 }
            #[wasm_zero] pub fn shout(msg: String) -> String { todo!() }
        "#;
        let g = generate_bindings(src).unwrap();
        assert!(g.js.contains("import * as r from 'rkyv-js'"));
        assert!(g.js.contains(
            "shout(msg) { return call(\"__wasm_zero_shout\", 8, (dv, u8, p) => rdStr(dv, u8, p), r.string, msg, null); }"
        ));
        assert!(g.js.contains("r.encode(argCodec, argValue)"));
    }
}
