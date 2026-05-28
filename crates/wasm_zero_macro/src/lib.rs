use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, parse_quote, ItemFn, ReturnType};

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
        ReturnType::Type(_, ty) => ty.clone(),
    };

    let mut shim = original.clone();
    let shim_ident = format_ident!("__wasm_zero_{}", original.sig.ident);
    let original_ident = &original.sig.ident;

    set_safety(&mut shim);
    set_abi(&mut shim);
    shim.sig.ident = shim_ident;

    // Replace the body with the FFI shim logic
    shim.sig.inputs = parse_quote!(out_ptr: u32);
    shim.sig.output = parse_quote!(-> u32);
    shim.block = parse_quote!({
        let result: #return_ty = #original_ident();

        let bytes = match ::rkyv::to_bytes::<::rkyv::rancor::Error>(&result) {
            Ok(b) => b,
            Err(_) => return ErrorCode::SerializationError as u32,
        };

        let len = bytes.len() as u32;

        unsafe {
            let out = out_ptr as *mut u8;
            ::core::ptr::copy_nonoverlapping(
                len.to_le_bytes().as_ptr(),
                out,
                4,
            );
            ::core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                out.add(4),
                bytes.len(),
            );
        }

        ErrorCode::Ok as u32
    });

    let expanded = quote! {
        // Original function untouched
        #original

        // FFI shim
        #[unsafe(no_mangle)]
        #shim
    };

    expanded.into()
}

fn set_safety(item: &mut ItemFn) -> &mut ItemFn {
    item.sig.unsafety = Some(parse_quote!(unsafe));
    item
}

fn set_abi(item: &mut ItemFn) -> &mut ItemFn {
    item.sig.abi = Some(parse_quote!(extern "C"));
    item
}
