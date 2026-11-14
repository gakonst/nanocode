// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use crate::low::generic;
use crate::low::x86_64;

pub(crate) fn sha512_compress_blocks(state: &mut [u64; 8], blocks: &[u8]) {
    // nb. avx2 is in our required set.
    if let Some(token) = x86_64::cpu::HaveBmi2::check() {
        x86_64::sha512::sha512_compress_blocks(state, blocks, token)
    } else {
        generic::sha512::sha512_compress_blocks(state, blocks)
    }
}
