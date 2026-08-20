// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.
//
// Copyright (c) DUSK NETWORK. All rights reserved.

use alloc::alloc::{alloc, dealloc, Layout};
use core::slice;

use bytecheck::CheckBytes;
use rkyv::api::high::{HighDeserializer, HighValidator};
use rkyv::rancor;
use rkyv::util::AlignedVec;
use rkyv::{Archive, Deserialize};

use crate::error::ErrorCode;

// big enough to cover all rkyv archived types
const ALIGNMENT: usize = 16;

/// Byte offset of the archive payload within a `[len][archive]` buffer.
///
/// The length is a `u32` at offset 0; the archive starts at `HEADER`. We pad to
/// 16 (rather than 4) so that — because `malloc` returns 16-aligned pointers —
/// the archive base is 16-aligned. That lets JS build correctly-aligned
/// zero-copy typed-array views over archived `Vec<T>` data (a `Float64Array`
/// view needs an 8-aligned offset). Must match the `HEADER` constant in the
/// generated JS bindings.
pub const HEADER: usize = 16;

/// Capacity (bytes) the JS scratch output buffer reserves for the archive
/// payload. The shim serializes directly into this region, so it bounds the
/// largest value a `#[wasm_zero]` function may return. Must match
/// `MAX_BUFFER_SIZE` in the generated JS bindings.
///
/// 512 KiB is large enough to hold a 320×240 RGBA frame (≈300 KiB) for the
/// canvas demo with headroom; small returns are unaffected (the scratch buffer
/// is malloc'd once).
pub const MAX_BUFFER_SIZE: usize = 512 * 1024;

#[unsafe(no_mangle)]
pub fn malloc(len: u32) -> u32 {
    // SAFETY: We use the same ALIGNMENT big enough to cover rkyv types
    unsafe {
        let layout = Layout::from_size_align_unchecked(len as usize, ALIGNMENT);
        let ptr = alloc(layout);
        ptr as _
    }
}

#[unsafe(no_mangle)]
pub fn free(ptr: u32, len: u32) {
    // SAFETY: We use the same ALIGNMENT big enough to cover rkyv types
    unsafe {
        let layout = Layout::from_size_align_unchecked(len as usize, ALIGNMENT);
        dealloc(ptr as _, layout);
    }
}

/// Read a buffer from the given pointer.
/// # SAFETY
/// the pointer
pub unsafe fn read_buffer<'a>(ptr: *const u8) -> &'a [u8] {
    // SAFETY: We use the same ALIGNMENT big enough to cover rkyv types
    unsafe {
        let len = slice::from_raw_parts(ptr, 4);
        let len = u32::from_le_bytes(len.try_into().unwrap()) as usize;
        slice::from_raw_parts(ptr.add(HEADER), len)
    }
}

/// Parse the buffer
///
/// rkyv reads the archive **in place**, so `bytes` must satisfy
/// the archived type's alignment.
///
/// Buffers from [`read_buffer`] are always 16-aligned,
/// which covers every rkyv archived type, so the generated bindings unarchive with no copy at all.
///
/// Any other caller is made correct by copying into aligned scratch first.
///
/// # SAFETY
/// the pointer
pub unsafe fn parse_buffer<T>(bytes: &[u8]) -> Result<T, ErrorCode>
where
    T: Archive,
    T::Archived: for<'a> CheckBytes<HighValidator<'a, rancor::Error>>
        + Deserialize<T, HighDeserializer<rancor::Error>>,
{
    if (bytes.as_ptr() as usize).is_multiple_of(ALIGNMENT) {
        return rkyv::from_bytes::<T, rancor::Error>(bytes)
            .or(Err(ErrorCode::UnarchivingError));
    }

    // AlignedVec guarantees 16-byte alignment — enough for all rkyv types
    let mut aligned = AlignedVec::<16>::new();
    aligned.extend_from_slice(bytes);

    rkyv::from_bytes::<T, rancor::Error>(&aligned).or(Err(ErrorCode::UnarchivingError))
}

/// Checks and deserializes a value from the given po
/// # SAFETY
/// the pointer
pub unsafe fn from_buffer<T>(ptr: *const u8) -> Result<T, ErrorCode>
where
    T: Archive,
    T::Archived: for<'a> CheckBytes<HighValidator<'a, rancor::Error>>
        + Deserialize<T, HighDeserializer<rancor::Error>>,
{
    let bytes = unsafe { read_buffer(ptr) };

    unsafe { parse_buffer::<T>(bytes) }
}
