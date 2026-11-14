// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use crate::low::generic;
use crate::low::x86_64;

pub(crate) fn sha256_compress_blocks(state: &mut [u32; 8], blocks: &[u8]) {
    if let Some(token) = x86_64::cpu::HaveSha256::check() {
        x86_64::sha256::sha256_compress_blocks_shaext(state, blocks, token)
    } else {
        generic::sha256::sha256_compress_blocks(state, blocks)
    }
}
