use proc_macro::TokenStream;
use proc_macro2::Literal;
use quote::{format_ident, quote, ToTokens};
use syn::{parse_macro_input, FnArg, Index, Item, ItemFn, ItemStruct, ReturnType, Type};

/// On a `fn foo(args...) -> T`: emits an FFI shim that rkyv-decodes its
/// arguments from an input buffer and rkyv-encodes its return value into an
/// output buffer.
///
/// Generated export (arguments present):
/// ```ignore
/// __wasm_zero_foo(in_ptr: u32, out_ptr: u32) -> u32
/// ```
/// Nullary functions omit `in_ptr`. Both buffers use the `[len: u32][bytes]`
/// protocol from `wasm_zero::mem`. The returned `u32` is a `wasm_zero::ErrorCode`
/// (`0 == Ok`).
///
/// On a `struct` with named fields: the struct is passed through unchanged and
/// its field layout is recorded so the binding generator can emit a zero-copy
/// reader for it. Annotate every `#[derive(Archive)]` struct that appears in a
/// `#[wasm_zero]` function signature.
///
/// In both cases the item's signature is embedded as a metadata record in a
/// `__wasm_zero` custom section of the wasm binary (a `#[link_section]` static;
/// the linker concatenates them). `wasm_zero_build` reads that section from the
/// compiled `.wasm` — no source parsing — and generates the JS/TS bindings.
#[proc_macro_attribute]
pub fn wasm_zero(_attr: TokenStream, item: TokenStream) -> TokenStream {
    match parse_macro_input!(item as Item) {
        Item::Fn(f) => expand_fn(f),
        Item::Struct(s) => expand_struct(s),
        other => syn::Error::new_spanned(
            other,
            "#[wasm_zero] supports free functions and structs with named fields",
        )
        .to_compile_error()
        .into(),
    }
}

// ---------------------------------------------------------------------------
// Metadata records (read back by wasm_zero_build from the compiled wasm).
//
// Encoding, kept in sync with wasm_zero_build::parse_records:
//   record  = [payload_len: u32 LE][payload: utf8]
//   payload = fields joined by U+001F (unit separator)
//   fields  = version("1"), kind("fn"|"struct"), name, then per-item data;
//             name/type pairs use U+001E between name and type.
// The records land in the `__wasm_zero` custom section; same-section statics
// are concatenated by the linker, so the length prefix delimits them.
// ---------------------------------------------------------------------------

const META_VERSION: &str = "1";
const FIELD_SEP: char = '\u{1f}';
const PAIR_SEP: char = '\u{1e}';

fn ty_string(ty: &Type) -> String {
    ty.to_token_stream().to_string()
}

/// `#[link_section = "__wasm_zero"]` static holding one encoded record.
fn meta_static(suffix: &str, fields: &[String]) -> proc_macro2::TokenStream {
    let payload = fields.join(&FIELD_SEP.to_string());
    let mut bytes = (payload.len() as u32).to_le_bytes().to_vec();
    bytes.extend_from_slice(payload.as_bytes());

    let ident = format_ident!("__WASM_ZERO_META_{suffix}");
    let len = bytes.len();
    let lit = Literal::byte_string(&bytes);
    quote! {
        #[cfg(target_arch = "wasm32")]
        #[doc(hidden)]
        #[used]
        #[unsafe(link_section = "__wasm_zero")]
        static #ident: [u8; #len] = *#lit;
    }
}

fn expand_struct(s: ItemStruct) -> TokenStream {
    if !s.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &s.generics,
            "#[wasm_zero] structs cannot be generic",
        )
        .to_compile_error()
        .into();
    }
    let syn::Fields::Named(fields) = &s.fields else {
        return syn::Error::new_spanned(
            &s.fields,
            "#[wasm_zero] structs need named fields",
        )
        .to_compile_error()
        .into();
    };

    let name = s.ident.to_string();
    let mut meta = vec![META_VERSION.into(), "struct".into(), name.clone()];
    for f in &fields.named {
        meta.push(format!(
            "{}{PAIR_SEP}{}",
            f.ident.as_ref().unwrap(),
            ty_string(&f.ty)
        ));
    }
    let meta = meta_static(&name, &meta);

    quote! {
        #s
        #meta
    }
    .into()
}

fn expand_fn(original: ItemFn) -> TokenStream {
    let return_ty = match &original.sig.output {
        ReturnType::Default => {
            return syn::Error::new_spanned(
                &original.sig,
                "wasm_zero requires a return type that implements rkyv::Serialize",
            )
            .to_compile_error()
            .into();
        }
        ReturnType::Type(_, ty) => (**ty).clone(),
    };

    // Collect arguments (by value — references aren't rkyv-decodable here).
    let mut arg_types: Vec<Type> = Vec::new();
    let mut arg_names: Vec<String> = Vec::new();
    for (i, input) in original.sig.inputs.iter().enumerate() {
        match input {
            FnArg::Typed(pt) => {
                arg_names.push(match &*pt.pat {
                    syn::Pat::Ident(id) => id.ident.to_string(),
                    _ => format!("arg{i}"),
                });
                arg_types.push((*pt.ty).clone());
            }
            FnArg::Receiver(recv) => {
                return syn::Error::new_spanned(
                    recv,
                    "wasm_zero does not support methods (functions taking `self`)",
                )
                .to_compile_error()
                .into();
            }
        }
    }

    let original_ident = &original.sig.ident;
    let shim_ident = format_ident!("__wasm_zero_{original_ident}");

    let mut meta = vec![
        META_VERSION.into(),
        "fn".into(),
        original_ident.to_string(),
        ty_string(&return_ty),
    ];
    for (n, t) in arg_names.iter().zip(&arg_types) {
        meta.push(format!("{n}{PAIR_SEP}{}", ty_string(t)));
    }
    let meta = meta_static(&original_ident.to_string(), &meta);

    // Scalar args are passed directly as wasm function parameters (no input
    // buffer) — the fast path. Any non-scalar arg falls back to an rkyv-encoded
    // input buffer.
    let all_scalar =
        !arg_types.is_empty() && arg_types.iter().all(is_direct_scalar);

    let unit_return = is_unit(&return_ty);
    let scalar_return = unit_return || is_direct_scalar(&return_ty);

    // Fully-scalar fast path: a scalar/unit return with no buffer-encoded args.
    // The shim is then a plain wasm function — args as params, value as the
    // return — with no output buffer, no rkyv, and no error code (it cannot
    // fail). This matches a bare wasm-bindgen export.
    if scalar_return && (arg_types.is_empty() || all_scalar) {
        let names: Vec<_> = (0..arg_types.len())
            .map(|i| format_ident!("a{i}"))
            .collect();
        let tys = &arg_types;
        let ret_arrow = if unit_return {
            quote!()
        } else {
            quote!(-> #return_ty)
        };
        let expanded = quote! {
            #original

            #meta

            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn #shim_ident(#(#names: #tys),*) #ret_arrow {
                #original_ident(#(#names),*)
            }
        };
        return expanded.into();
    }

    // Build the shim signature, the argument-decoding prelude, and the call.
    let (shim_inputs, decode_prelude, call_expr) = if all_scalar {
        let names: Vec<_> = (0..arg_types.len())
            .map(|i| format_ident!("a{i}"))
            .collect();
        let tys = &arg_types;
        (
            quote!( #(#names: #tys),*, out_ptr: u32 ),
            quote!(),
            quote!( #original_ident( #(#names),* ) ),
        )
    } else {
        match arg_types.len() {
        0 => (quote!(out_ptr: u32), quote!(), quote!(#original_ident())),
        1 => {
            let ty = &arg_types[0];
            (
                quote!(in_ptr: u32, out_ptr: u32),
                quote! {
                    let __arg: #ty = match unsafe {
                        ::wasm_zero::mem::from_buffer::<#ty>(in_ptr as *const u8)
                    } {
                        Ok(a) => a,
                        Err(e) => return e as u32,
                    };
                },
                quote!(#original_ident(__arg)),
            )
        }
        _ => {
            let tys = &arg_types;
            let idxs: Vec<Index> = (0..arg_types.len()).map(Index::from).collect();
            (
                quote!(in_ptr: u32, out_ptr: u32),
                quote! {
                    let __args: ( #(#tys),* ) = match unsafe {
                        ::wasm_zero::mem::from_buffer::<( #(#tys),* )>(in_ptr as *const u8)
                    } {
                        Ok(a) => a,
                        Err(e) => return e as u32,
                    };
                },
                quote!(#original_ident( #(__args.#idxs),* )),
            )
        }
        }
    };

    let expanded = quote! {
        // Original function, untouched.
        #original

        #meta

        // FFI shim.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #shim_ident(#shim_inputs) -> u32 {
            #decode_prelude

            let result: #return_ty = #call_expr;

            let out = out_ptr as *mut u8;

            // Serialize the archive directly into the output buffer (no
            // intermediate allocation, no copy). The region is 16-aligned
            // (malloc + HEADER) as rkyv requires.
            let dst = unsafe {
                ::core::slice::from_raw_parts_mut(
                    out.add(::wasm_zero::mem::HEADER),
                    ::wasm_zero::mem::MAX_BUFFER_SIZE,
                )
            };
            let written = match ::rkyv::api::high::to_bytes_in::<_, ::rkyv::rancor::Error>(
                &result,
                ::rkyv::ser::writer::Buffer::from(dst),
            ) {
                Ok(buf) => buf.len() as u32,
                Err(_) => return ::wasm_zero::ErrorCode::ArchivingError as u32,
            };

            unsafe {
                ::core::ptr::copy_nonoverlapping(
                    written.to_le_bytes().as_ptr(),
                    out,
                    4,
                );
            }

            ::wasm_zero::ErrorCode::Ok as u32
        }
    };

    expanded.into()
}

/// True if `ty` is the unit type `()`.
fn is_unit(ty: &Type) -> bool {
    matches!(ty, Type::Tuple(t) if t.elems.is_empty())
}

/// True if `ty` is a primitive that maps to a single wasm scalar, so it can be
/// passed as a direct FFI argument instead of through an rkyv input buffer.
fn is_direct_scalar(ty: &Type) -> bool {
    let Type::Path(p) = ty else { return false };
    let Some(seg) = p.path.segments.last() else {
        return false;
    };
    if !seg.arguments.is_empty() {
        return false;
    }
    matches!(
        seg.ident.to_string().as_str(),
        "i8" | "i16"
            | "i32"
            | "i64"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "f32"
            | "f64"
            | "usize"
            | "isize"
            | "bool"
    )
}
