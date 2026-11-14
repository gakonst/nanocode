// Written for Graviola by Joe Birr-Pixton, 2025.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

cfg_if::cfg_if! {
    if #[cfg(all(test, feature = "__ctgrind"))] {
        use crabgrind::{memcheck, valgrind};
        use core::mem::size_of_val;

        #[inline]
        pub(crate) fn secret_slice<T>(v: &[T]) {
            if let valgrind::RunningMode::Valgrind = valgrind::running_mode() {
                memcheck::mark_memory(
                    v.as_ptr().cast(),
                    size_of_val(v),
                    memcheck::MemState::Undefined,
                )
                .unwrap();
            }
        }

        #[inline]
        pub(crate) fn public_slice<T>(v: &[T]) {
            if let valgrind::RunningMode::Valgrind = valgrind::running_mode() {
                memcheck::mark_memory(
                    v.as_ptr().cast(),
                    size_of_val(v),
                    memcheck::MemState::Defined,
                )
                .unwrap();
            }
        }

        #[inline]
        #[must_use]
        pub(crate) fn into_secret<T>(v: T) -> T {
            if let valgrind::RunningMode::Valgrind = valgrind::running_mode() {
                memcheck::mark_memory(
                    (&v as *const T).cast(),
                    size_of_val(&v),
                    memcheck::MemState::Undefined,
                )
                .unwrap();
            }
            v
        }

        #[inline]
        #[must_use]
        pub(crate) fn into_public<T>(v: T) -> T {
            if let valgrind::RunningMode::Valgrind = valgrind::running_mode() {
                memcheck::mark_memory(
                    (&v as *const T).cast(),
                    size_of_val(&v),
                    memcheck::MemState::Defined,
                )
                .unwrap();
            }
            v
        }
    } else {
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
    }
}
