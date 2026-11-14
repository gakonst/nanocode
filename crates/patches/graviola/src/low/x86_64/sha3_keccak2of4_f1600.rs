// Written for Graviola by Joe Birr-Pixton, 2026.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

/// Compute Keccak-f1600 on the first two states in `a`.
#[inline]
pub(crate) fn sha3_keccak2of4_f1600(a: &mut [[u64; 25]; 4], rc: &[u64; 24]) {
    super::sha3_keccak4_f1600_shim::sha3_keccak4_f1600(a, rc);
}
