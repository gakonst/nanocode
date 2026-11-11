//! Container growth out of line (D2521).
//!
//! `Vec::push` inlines its capacity check and a call to the grow path at every site,
//! and the call takes the `Vec` by reference, so the vector is spilled to the shadow
//! stack around it even where it could live in locals. At ~370 sites that was ~13 KB of
//! wasm. `push_out` is one small function per element type instead: use it wherever a
//! push is not in a loop that the benchmarks show, and of values up to 16 words (D2521
//! lists the measurements).

use alloc::string::String;
use alloc::vec::Vec;

/// `Vec::with_capacity`, out of line: the allocation call and its error path are one
/// function per element type instead of being inlined at each site (D2521).
#[inline(never)]
pub(crate) fn vec_with_capacity<T>(n: usize) -> Vec<T> {
    Vec::with_capacity(n)
}

pub(crate) trait PushOut<T> {
    /// `push`, out of line.
    fn push_out(&mut self, x: T);
}

impl<T> PushOut<T> for Vec<T> {
    #[inline(never)]
    fn push_out(&mut self, x: T) {
        // Large values are better built in place by an inline `push`: passed to a call
        // they are first built in the caller's frame and then copied, and the frame
        // grows by their size (a 832-byte `FuncState` pushed this way overflowed the
        // parser's stack budget in the debug wasm build, D1941).
        const {
            assert!(
                size_of::<T>() <= 16 * size_of::<usize>(),
                "push_out is for small values"
            )
        };
        self.push(x)
    }
}

impl PushOut<char> for String {
    #[inline(never)]
    fn push_out(&mut self, c: char) {
        self.push(c)
    }
}
