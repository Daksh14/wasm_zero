use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, FnArg, Index, ItemFn, ReturnType, Type};

/// Turns `fn foo(args...) -> T` into an FFI shim that rkyv-decodes its arguments
/// from an input buffer and rkyv-encodes its return value into an output
/// buffer.
///
/// Generated export (arguments present):
/// ```ignore
/// __wasm_zero_foo(in_ptr: u32, out_ptr: u32) -> u32
/// ```
/// Nullary functions omit `in_ptr`. Both buffers use the `[len: u32][bytes]`
/// protocol from `wasm_zero::mem`. The returned `u32` is a `wasm_zero::ErrorCode`
/// (`0 == Ok`).
#[proc_macro_attribute]
pub fn wasm_zero(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let original = parse_macro_input!(item as ItemFn);

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

    // Collect argument types (by value — references aren't rkyv-decodable here).
    let mut arg_types: Vec<Type> = Vec::new();
    for input in &original.sig.inputs {
        match input {
            FnArg::Typed(pt) => arg_types.push((*pt.ty).clone()),
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
