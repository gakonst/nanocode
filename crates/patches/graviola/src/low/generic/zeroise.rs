// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use core::mem::{size_of, size_of_val};

use crate::low::zero_bytes;

/// Writes zeroes over the whole of the `v` slice.
pub(crate) fn zeroise<T: Zeroable>(v: &mut [T]) {
    zero_bytes(v.as_mut_ptr().cast(), size_of_val(v));
}

/// Writes zeroes over the whole of the `v` value.
pub(crate) fn zeroise_value<T: Zeroable>(v: &mut T) {
    zero_bytes((v as *mut T).cast(), size_of::<T>());
}

/// Marker trait for types who have valid all-bits-zero values.
pub(crate) trait Zeroable {}

impl Zeroable for u8 {}
impl Zeroable for u64 {}
impl Zeroable for usize {}

#[cfg(target_arch = "x86_64")]
impl Zeroable for core::arch::x86_64::__m128i {}
#[cfg(target_arch = "x86_64")]
impl Zeroable for core::arch::x86_64::__m256i {}
#[cfg(target_arch = "x86_64")]
impl Zeroable for core::arch::x86_64::__m512i {}

#[cfg(target_arch = "aarch64")]
impl Zeroable for core::arch::aarch64::uint8x16_t {}
#[cfg(target_arch = "aarch64")]
impl Zeroable for core::arch::aarch64::uint64x2_t {}
