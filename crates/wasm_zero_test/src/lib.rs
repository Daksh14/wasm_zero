use wasm_bindgen::prelude::*;

pub struct Person {
    pub name: String,
    pub age: u32,
}

#[wasm_bindgen]
pub fn greet(name: &str) -> String {
    format!("Hello, {name}! wasm_zero (std) is loaded.")
}
