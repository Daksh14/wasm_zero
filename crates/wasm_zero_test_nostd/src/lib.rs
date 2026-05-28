#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

use rkyv::{Archive, Deserialize, Serialize};
use wasm_zero::wasm_zero;

#[derive(Archive, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
    pub age: u32,
    pub email: Option<String>,
    pub scores: Vec<u32>,
}

#[wasm_zero]
pub fn get_adult_person() -> Person {
    Person {
        name: "Susize".to_string(),
        age: 25,
        email: Some("susize@example.com".to_string()),
        scores: vec![95, 87, 92],
    }
}

#[wasm_zero]
pub fn greet() -> String {
    "Hello! wasm_zero (no_std) is loaded.".to_string()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
