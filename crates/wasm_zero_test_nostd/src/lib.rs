#![no_std]

use crate::alloc::string::ToString;
extern crate alloc;

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

use alloc::format;
use alloc::string::String;
use wasm_bindgen::prelude::*;
use wasm_zero::wasm_zero;

use rkyv::{Archive, Deserialize, Serialize};

#[derive(Archive, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
    pub age: u32,
}

#[wasm_zero]
pub fn get_adult_person() -> Person {
    Person {
        name: "Susize".to_string(),
        age: 25,
    }
}

#[wasm_zero]
pub fn greet(name: &str) -> String {
    format!("Hello, {name}! wasm_zero (no_std) is loaded.")
}

// #[wasm_bindgen]
// pub fn greet(name: &str) -> String {
//     format!("Hello, {name}! wasm_zero (no_std) is loaded.")
// }
