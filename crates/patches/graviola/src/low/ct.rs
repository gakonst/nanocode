// Written for Graviola by Joe Birr-Pixton, 2025.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

#[inline]
pub(crate) fn secret_slice<T>(_v: &[T]) {}

#[inline]
pub(crate) fn public_slice<T>(_v: &[T]) {}

#[inline]
#[must_use]
pub(crate) fn into_secret<T>(v: T) -> T { v }

#[inline]
#[must_use]
pub(crate) fn into_public<T>(v: T) -> T { v }

