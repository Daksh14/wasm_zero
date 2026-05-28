#![no_std]

extern crate alloc;

pub mod error;
pub mod mem;

pub use error::ErrorCode;
pub use rkyv;
pub use wasm_zero_macro::wasm_zero;
