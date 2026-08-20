#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

use rkyv::{Archive, Deserialize, Serialize};
use wasm_zero::wasm_zero;

#[wasm_zero]
#[derive(Archive, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
    pub age: u32,
    pub email: Option<String>,
    pub scores: Vec<u32>,
}

// --- call overhead ---------------------------------------------------------

/// Minimal round-trip: a function that returns the unit type. Measures the
/// per-call rkyv `[len][bytes]` tax (here `len == 0`).
#[wasm_zero]
pub fn thunk() -> () {}

/// Adds two numbers. The args are rkyv-decoded from the input buffer.
#[wasm_zero]
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

/// Computes Fib(n). Returns `i32` to match `bench_wasm_bindgen::fibonacci`
/// exactly (an `u64` return would box as a JS BigInt — a different, slower path).
#[wasm_zero]
pub fn fibonacci(n: i32) -> i32 {
    let mut a: u64 = 1;
    let mut b: u64 = 1;
    for _ in 0..n {
        let tmp = b;
        b += a;
        a = tmp;
    }
    a as i32
}

// --- data round-trip -------------------------------------------------------

/// Matches `bench_wasm_bindgen::get_person`.
#[wasm_zero]
pub fn get_person() -> Person {
    Person {
        name: "Susize".to_string(),
        age: 25,
        email: Some("susize@example.com".to_string()),
        scores: alloc::vec![95, 87, 92],
    }
}

/// Matches `bench_wasm_bindgen::get_scores` (1024 u32s).
#[wasm_zero]
pub fn get_scores() -> Vec<u32> {
    (0..1024u32).collect()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
