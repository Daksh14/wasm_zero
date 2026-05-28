#![no_std]

extern crate alloc;

mod error;
pub mod mem;

pub use rkyv;
pub use wasm_zero_macro::wasm_zero;
