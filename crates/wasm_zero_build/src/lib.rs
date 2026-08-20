//! Binding generator for `wasm_zero`.
//!
//! Runs **after** `cargo build`, on the compiled `.wasm`. The `#[wasm_zero]`
//! macro embeds each annotated function's and struct's signature as a metadata
//! record in a `__wasm_zero` custom section of the binary; this crate extracts
//! that section — no source parsing — and emits `bindings.ts` + `bindings.js`.
//!
//! The read path is **self-contained and zero-copy**: for each struct it emits
//! a `decode_<Struct>` function that reads each field straight out of wasm
//! memory at its archived offset — scalars as `DataView` reads, numeric vecs as
//! typed-array views, strings transcoded on demand, options inline. No rkyv-js
//! runtime is needed to decode.
//!
//! rkyv-js is imported only when a function takes a **non-scalar argument**
//! (`String`/struct/…), to codec-encode it into the input buffer (hand-rolling
//! the rkyv *writer* is out of scope). Scalar args pass directly as wasm params,
//! and scalar/unit returns come back as the wasm function's value.
//!
//! ```text
//! cargo build --target wasm32-unknown-unknown -p my_crate
//! cargo run -p wasm_zero_build --example generate -- \
//!     target/wasm32-unknown-unknown/debug/my_crate.wasm pkg
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use syn::Type;

/// Struct name -> its fields (in declaration order).
type StructMap = BTreeMap<String, Vec<(String, Type)>>;

/// A `#[wasm_zero]` function, as recorded by the macro.
pub struct FnMeta {
    pub name: String,
    pub args: Vec<(String, Type)>,
    pub ret: Type,
}

/// The two files to write.
#[derive(Debug)]
pub struct Generated {
    pub ts: String,
    pub js: String,
}

/// Read the compiled `wasm`, extract the `__wasm_zero` metadata section, and
/// write `bindings.ts` + `bindings.js` into `out_dir`.
pub fn generate(wasm: impl AsRef<Path>, out_dir: impl AsRef<Path>) {
    let wasm = wasm.as_ref();
    let out_dir = out_dir.as_ref();

    let bytes = std::fs::read(wasm)
        .unwrap_or_else(|e| panic!("wasm_zero_build: failed to read {}: {e}", wasm.display()));
    let g = generate_bindings(&bytes)
        .unwrap_or_else(|e| panic!("wasm_zero_build: {}: {e}", wasm.display()));

    std::fs::create_dir_all(out_dir).unwrap_or_else(|e| {
        panic!(
            "wasm_zero_build: failed to create {}: {e}",
            out_dir.display()
        )
    });
    for (name, contents) in [("bindings.ts", &g.ts), ("bindings.js", &g.js)] {
        let path = out_dir.join(name);
        std::fs::write(&path, contents)
            .unwrap_or_else(|e| panic!("wasm_zero_build: failed to write {}: {e}", path.display()));
    }
}

/// Copy the wasm at `wasm` to `out` with the `__wasm_zero` metadata sections
/// removed (they're only needed to generate bindings, not at runtime).
pub fn strip(wasm: impl AsRef<Path>, out: impl AsRef<Path>) {
    let wasm = wasm.as_ref();
    let out = out.as_ref();
    let bytes = std::fs::read(wasm)
        .unwrap_or_else(|e| panic!("wasm_zero_build: failed to read {}: {e}", wasm.display()));
    let stripped = strip_meta(&bytes)
        .unwrap_or_else(|e| panic!("wasm_zero_build: {}: {e}", wasm.display()));
    std::fs::write(out, stripped)
        .unwrap_or_else(|e| panic!("wasm_zero_build: failed to write {}: {e}", out.display()));
}

/// `wasm` with every `__wasm_zero` custom section removed. Pure.
pub fn strip_meta(wasm: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(wasm.len());
    for_each_section(wasm, |chunk, meta| {
        if meta.is_none() {
            out.extend_from_slice(chunk);
        }
    })?;
    Ok(out)
}

/// Decode the metadata embedded in a wasm binary and render both files.
/// Pure — unit-testable.
pub fn generate_bindings(wasm: &[u8]) -> Result<Generated, String> {
    let blob = extract_meta_section(wasm)?;
    if blob.is_empty() {
        return Err(
            "no `__wasm_zero` metadata section found — does the crate use #[wasm_zero]? \
             (note: wasm-opt/strip may remove custom sections; generate bindings first)"
                .into(),
        );
    }
    let (structs, funcs) = parse_records(&blob)?;
    Ok(Generated {
        ts: render_module(true, &structs, &funcs),
        js: render_module(false, &structs, &funcs),
    })
}

// ---------------------------------------------------------------------------
// Metadata extraction: wasm custom section -> records.
//
// Encoding, kept in sync with wasm_zero_macro::meta_static:
//   record  = [payload_len: u32 LE][payload: utf8]
//   payload = fields joined by U+001F (unit separator)
//   fields  = version("1"), kind("fn"|"struct"), name, then per-item data;
//             name/type pairs use U+001E between name and type.
// The linker concatenates the per-item `#[link_section]` statics, so the
// section is a back-to-back sequence of length-prefixed records.
// ---------------------------------------------------------------------------

const META_SECTION: &str = "__wasm_zero";
const META_VERSION: &str = "1";
const FIELD_SEP: char = '\u{1f}';
const PAIR_SEP: char = '\u{1e}';

fn leb_u32(bytes: &[u8], pos: &mut usize) -> Result<u32, String> {
    let mut result = 0u32;
    let mut shift = 0;
    loop {
        let b = *bytes.get(*pos).ok_or("truncated wasm (leb128)")?;
        *pos += 1;
        result |= u32::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift >= 32 {
            return Err("leb128 overflow".into());
        }
    }
}

/// Walk the wasm header and sections. For each chunk, calls `f(chunk, meta)`
/// where `chunk` is the raw bytes (the 8-byte header, or a whole section
/// including its id/size prefix) and `meta` is `Some(data)` iff the chunk is a
/// `__wasm_zero` custom section (data = the contents after the section name).
fn for_each_section<'a>(
    wasm: &'a [u8],
    mut f: impl FnMut(&'a [u8], Option<&'a [u8]>),
) -> Result<(), String> {
    if wasm.len() < 8 || &wasm[..4] != b"\0asm" {
        return Err("not a wasm binary".into());
    }
    f(&wasm[..8], None);
    let mut pos = 8;
    while pos < wasm.len() {
        let start = pos;
        let id = wasm[pos];
        pos += 1;
        let size = leb_u32(wasm, &mut pos)? as usize;
        let end = pos
            .checked_add(size)
            .filter(|&e| e <= wasm.len())
            .ok_or("truncated wasm section")?;
        let mut meta = None;
        if id == 0 {
            let mut p = pos;
            let name_len = leb_u32(wasm, &mut p)? as usize;
            if let Some(name_end) = p.checked_add(name_len).filter(|&e| e <= end) {
                if &wasm[p..name_end] == META_SECTION.as_bytes() {
                    meta = Some(&wasm[name_end..end]);
                }
            }
        }
        f(&wasm[start..end], meta);
        pos = end;
    }
    Ok(())
}

/// Concatenated contents of every `__wasm_zero` custom section in `wasm`.
fn extract_meta_section(wasm: &[u8]) -> Result<Vec<u8>, String> {
    let mut blob = Vec::new();
    for_each_section(wasm, |_, meta| {
        if let Some(m) = meta {
            blob.extend_from_slice(m);
        }
    })?;
    Ok(blob)
}

fn parse_ty(s: &str) -> Result<Type, String> {
    syn::parse_str::<Type>(s).map_err(|e| format!("bad type `{s}` in metadata: {e}"))
}

fn parse_pair(s: &str) -> Result<(String, Type), String> {
    let (name, ty) = s
        .split_once(PAIR_SEP)
        .ok_or_else(|| format!("malformed name/type pair `{s}` in metadata"))?;
    Ok((name.to_string(), parse_ty(ty)?))
}

/// Split the section blob into records and decode them.
fn parse_records(blob: &[u8]) -> Result<(StructMap, Vec<FnMeta>), String> {
    let mut structs = StructMap::new();
    let mut funcs: Vec<FnMeta> = Vec::new();

    let mut pos = 0usize;
    while pos < blob.len() {
        let len_bytes: [u8; 4] = blob
            .get(pos..pos + 4)
            .ok_or("truncated metadata record header")?
            .try_into()
            .unwrap();
        let len = u32::from_le_bytes(len_bytes) as usize;
        pos += 4;
        let payload = blob
            .get(pos..pos + len)
            .ok_or("truncated metadata record")?;
        pos += len;

        let payload =
            std::str::from_utf8(payload).map_err(|e| format!("non-utf8 metadata record: {e}"))?;
        let mut fields = payload.split(FIELD_SEP);

        let version = fields.next().unwrap_or_default();
        if version != META_VERSION {
            return Err(format!(
                "metadata version `{version}` doesn't match this wasm_zero_build \
                 (expected `{META_VERSION}`) — rebuild with matching crate versions"
            ));
        }
        let kind = fields.next().unwrap_or_default();
        let name = fields
            .next()
            .filter(|n| !n.is_empty())
            .ok_or("metadata record missing item name")?
            .to_string();

        match kind {
            "fn" => {
                let ret = parse_ty(fields.next().ok_or_else(|| {
                    format!("metadata for fn `{name}` missing return type")
                })?)?;
                let args = fields.map(parse_pair).collect::<Result<Vec<_>, _>>()?;
                funcs.push(FnMeta { name, args, ret });
            }
            "struct" => {
                let fields = fields.map(parse_pair).collect::<Result<Vec<_>, _>>()?;
                structs.insert(name, fields);
            }
            other => return Err(format!("unknown metadata record kind `{other}`")),
        }
    }
    // Linker section order isn't source order — sort for deterministic output.
    funcs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((structs, funcs))
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
             String, Option, Vec, Box/Rc/Arc, and #[wasm_zero] structs. \
             (structs must carry the #[wasm_zero] attribute to be recorded)"
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
    args.first()
        .expect("wasm_zero_build: generic type needs an argument")
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
            read_field(
                arg0(&args),
                structs,
                &format!("({addr}) + dv.getInt32({addr}, true)"),
            )
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
        "u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "f32" | "f64" | "usize" | "isize" => {
            "number".to_string()
        }
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
        "u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "u64" | "i64" | "f32" | "f64" | "bool"
        | "char" => format!("r.{id}"),
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

fn render_module(typed: bool, structs: &StructMap, funcs: &[FnMeta]) -> String {
    let t = |s: &str| if typed { s.to_string() } else { String::new() };

    // rkyv-js is needed only to encode non-scalar arguments.
    let needs_rkyv = funcs
        .iter()
        .any(|f| f.args.iter().any(|(_, ty)| !is_scalar(ty)));

    let mut out = String::new();
    out.push_str("// Auto-generated by wasm_zero. Do not edit by hand.\n");
    if needs_rkyv {
        // Encoder-only entry: the read path is hand-rolled, so the generated
        // module never needs rkyv-js's decode/access machinery.
        out.push_str("import * as r from 'rkyv-js/encode';\n");
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

        // One hoisted codec per rkyv-encoded function: containers are
        // constructed once at module setup instead of on every call.
        out.push_str("// ---- per-function argument codecs ----\n");
        for f in funcs {
            if !f.args.is_empty() && !f.args.iter().all(|(_, ty)| is_scalar(ty)) {
                out.push_str(&format!(
                    "const __argCodec_{} = {};\n",
                    f.name,
                    args_codec(&f.args, structs)
                ));
            }
        }
        out.push('\n');
    }

    // One decoder per struct: reads each field at its archived offset.
    out.push_str("// ---- per-struct field readers ----\n");
    for (name, fields) in structs {
        out.push_str(&format!(
            "function decode_{name}(dv, u8, p) {{\n  return {{\n"
        ));
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
    // The argument writer archives in place into the input buffer, so it is
    // bound to that region of wasm memory and goes stale — along with the
    // views — whenever the memory grows.
    let writer_decl = if needs_rkyv {
        format!(
            "\x20 let argWriter{} = null; // fixed rkyv writer over the input buffer\n",
            t(": r.RkyvWriter | null"),
        )
    } else {
        String::new()
    };
    let writer_reset = if needs_rkyv { " argWriter = null;" } else { "" };
    out.push_str(&format!(
        "export function bindWasmZero(wasm{}) {{\n\
         \x20 const outPtr = wasm.malloc(HEADER + MAX_BUFFER_SIZE);\n\
         \x20 let inPtr = 0; // lazily allocated for non-scalar args\n\
         {}\
         \x20 let buf = wasm.memory.buffer, dv = new DataView(buf), u8 = new Uint8Array(buf);\n\
         \x20 function views() {{ if (buf !== wasm.memory.buffer) {{ buf = wasm.memory.buffer; dv = new DataView(buf); u8 = new Uint8Array(buf);{} }} }}\n\n\
         \x20 function invoke(shim{}, argCodec{}, argValue{}, directArgs{}) {{\n\
         \x20   if (directArgs !== null) return wasm[shim](...directArgs, outPtr);\n\
         \x20   if (argCodec === null) return wasm[shim](outPtr);\n",
        t(": any"), writer_decl, writer_reset,
        t(": string"), t(": any"), t(": any"), t(": any"),
    ));
    if needs_rkyv {
        out.push_str(
            "         \x20   if (inPtr === 0) inPtr = wasm.malloc(HEADER + MAX_BUFFER_SIZE);\n\
             \x20   views();\n\
             \x20   if (argWriter === null) argWriter = new r.RkyvWriter({ buffer: u8.subarray(inPtr + HEADER, inPtr + HEADER + MAX_BUFFER_SIZE) });\n\
             \x20   argWriter.reset();\n\
             \x20   argCodec.encodeInto(argWriter, argValue); // archived in place; overflow throws RangeError\n\
             \x20   dv.setUint32(inPtr, argWriter.pos, true);\n\
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
        t(": string"),
        t(": number"),
        t(": any"),
        t(": any"),
        t(": any"),
        t(": any"),
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
        t(": string | URL"),
        t("?: WebAssembly.Imports"),
    ));

    out
}

/// Render one method on the bound object.
fn render_method(f: &FnMeta, structs: &StructMap, typed: bool) -> String {
    let name = &f.name;
    let shim = format!("__wasm_zero_{name}");
    let ret = &f.ret;
    let args = &f.args;

    let names_csv = || {
        args.iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
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
    if (unit_ret || is_scalar(ret)) && (args.is_empty() || args.iter().all(|(_, t)| is_scalar(t))) {
        let call = format!("wasm[{shim:?}]({})", names_csv());
        if unit_ret {
            let ann = if typed { ": void" } else { "" };
            return format!("    {name}({params}){ann} {{ {call}; }},\n");
        }
        let ann = if typed {
            format!(": {}", ts_type(ret, structs))
        } else {
            String::new()
        };
        let expr = if path_parts(unref(ret)).0 == "bool" {
            format!("{call} !== 0")
        } else {
            call
        };
        return format!("    {name}({params}){ann} {{ return {expr}; }},\n");
    }

    // Arg mode: nullary / all-scalar (direct) / non-scalar (rkyv-encoded,
    // via the module-level hoisted codec).
    let (arg_codec, arg_value, direct_args) = if args.is_empty() {
        ("null".into(), "null".into(), "null".into())
    } else if args.iter().all(|(_, t)| is_scalar(t)) {
        ("null".into(), "null".into(), format!("[{}]", names_csv()))
    } else {
        let value = match args.as_slice() {
            [(n, _)] => n.clone(),
            _ => format!("[{}]", names_csv()),
        };
        (format!("__argCodec_{name}"), value, "null".into())
    };

    let size = archived(ret, structs).0;
    let read = read_field(ret, structs, "p");
    let ann = if typed {
        format!(": {}", ts_type(ret, structs))
    } else {
        String::new()
    };
    format!(
        "    {name}({params}){ann} {{ return call({shim:?}, {size}, (dv, u8, p) => {read}, {arg_codec}, {arg_value}, {direct_args}); }},\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode one metadata record the way the macro does.
    fn record(fields: &[&str]) -> Vec<u8> {
        let payload = fields.join("\u{1f}");
        let mut v = (payload.len() as u32).to_le_bytes().to_vec();
        v.extend_from_slice(payload.as_bytes());
        v
    }

    fn pair(name: &str, ty: &str) -> String {
        format!("{name}\u{1e}{ty}")
    }

    /// Wrap a metadata blob in a minimal-but-valid wasm binary with a
    /// `__wasm_zero` custom section (plus an unrelated custom section).
    fn fake_wasm(blob: &[u8]) -> Vec<u8> {
        fn leb(mut v: u32, out: &mut Vec<u8>) {
            loop {
                let b = (v & 0x7f) as u8;
                v >>= 7;
                if v == 0 {
                    out.push(b);
                    break;
                }
                out.push(b | 0x80);
            }
        }
        fn custom(name: &str, data: &[u8], out: &mut Vec<u8>) {
            let mut payload = Vec::new();
            leb(name.len() as u32, &mut payload);
            payload.extend_from_slice(name.as_bytes());
            payload.extend_from_slice(data);
            out.push(0); // custom section id
            leb(payload.len() as u32, out);
            out.extend_from_slice(&payload);
        }
        let mut w = b"\0asm\x01\0\0\0".to_vec();
        custom("producers", b"whatever", &mut w);
        custom("__wasm_zero", blob, &mut w);
        w
    }

    /// Metadata matching the old source-scanning test sample.
    fn sample() -> Vec<u8> {
        let mut blob = Vec::new();
        blob.extend(record(&[
            "1",
            "struct",
            "Person",
            &pair("name", "String"),
            &pair("age", "u32"),
            &pair("email", "Option<String>"),
            &pair("scores", "Vec<u32>"),
        ]));
        blob.extend(record(&["1", "fn", "get_adult_person", "Person"]));
        blob.extend(record(&["1", "fn", "greet", "String"]));
        blob.extend(record(&[
            "1",
            "fn",
            "add",
            "i32",
            &pair("a", "i32"),
            &pair("b", "i32"),
        ]));
        blob.extend(record(&["1", "fn", "tick", "()"]));
        blob.extend(record(&["1", "fn", "nums", "Vec<u32>"]));
        blob
    }

    #[test]
    fn extracts_custom_section() {
        let wasm = fake_wasm(&sample());
        let blob = extract_meta_section(&wasm).unwrap();
        assert_eq!(blob, sample());
    }

    #[test]
    fn struct_field_offsets() {
        let g = generate_bindings(&fake_wasm(&sample())).unwrap();
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
        let g = generate_bindings(&fake_wasm(&sample())).unwrap();
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
        let mut blob = Vec::new();
        blob.extend(record(&["1", "struct", "P", &pair("x", "u32")]));
        blob.extend(record(&["1", "fn", "shout", "String", &pair("msg", "String")]));
        let g = generate_bindings(&fake_wasm(&blob)).unwrap();
        assert!(g.js.contains("import * as r from 'rkyv-js/encode'"));
        // Arg codec built once at module setup, not per call.
        assert!(g.js.contains("const __argCodec_shout = r.string;"));
        assert!(g.js.contains(
            "shout(msg) { return call(\"__wasm_zero_shout\", 8, (dv, u8, p) => rdStr(dv, u8, p), __argCodec_shout, msg, null); }"
        ));
        // Arguments archive in place through a fixed writer over the wasm
        // input buffer — no intermediate encode buffer, no copy.
        assert!(g.js.contains("argCodec.encodeInto(argWriter, argValue)"));
        assert!(g.js.contains("argWriter = new r.RkyvWriter({ buffer: u8.subarray(inPtr + HEADER, inPtr + HEADER + MAX_BUFFER_SIZE) })"));
        assert!(g.js.contains("dv.setUint32(inPtr, argWriter.pos, true)"));
    }

    #[test]
    fn multi_arg_codec_is_a_hoisted_tuple() {
        let mut blob = Vec::new();
        blob.extend(record(&[
            "1",
            "fn",
            "send",
            "u32",
            &pair("tags", "Vec<u32>"),
            &pair("note", "String"),
        ]));
        let g = generate_bindings(&fake_wasm(&blob)).unwrap();
        assert!(g.js.contains("const __argCodec_send = r.tuple(r.vec(r.u32), r.string);"));

        // Scalar-only modules never import rkyv-js and keep the writer-free path.
        let mut scalar_blob = Vec::new();
        scalar_blob.extend(record(&[
            "1",
            "fn",
            "add",
            "i32",
            &pair("a", "i32"),
            &pair("b", "i32"),
        ]));
        let scalar = generate_bindings(&fake_wasm(&scalar_blob)).unwrap();
        assert!(!scalar.js.contains("rkyv-js"));
        assert!(!scalar.js.contains("argWriter"));
    }

    #[test]
    fn strip_removes_only_the_meta_section() {
        let wasm = fake_wasm(&sample());
        let stripped = strip_meta(&wasm).unwrap();
        assert!(extract_meta_section(&stripped).unwrap().is_empty());
        // Everything else survives (header + the "producers" section).
        assert_eq!(&stripped[..8], &wasm[..8]);
        assert!(stripped
            .windows(b"producers".len())
            .any(|w| w == b"producers"));
        assert!(stripped.len() < wasm.len());
    }

    #[test]
    fn version_mismatch_is_an_error() {
        let blob = record(&["9", "fn", "f", "u32"]);
        let err = generate_bindings(&fake_wasm(&blob)).unwrap_err();
        assert!(err.contains("version"), "{err}");
    }

    #[test]
    fn missing_section_is_an_error() {
        let err = generate_bindings(b"\0asm\x01\0\0\0").unwrap_err();
        assert!(err.contains("__wasm_zero"), "{err}");
    }

    #[test]
    fn types_with_token_spaces_parse() {
        // The macro stringifies types via token streams: `Vec < u32 >`.
        let blob = record(&["1", "fn", "nums", "Vec < u32 >"]);
        let g = generate_bindings(&fake_wasm(&blob)).unwrap();
        assert!(g.js.contains("rdVec(dv, p, Uint32Array)"));
    }
}
