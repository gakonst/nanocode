// Written for Graviola by Joe Birr-Pixton, 2026.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

#[inline]
pub(crate) fn sha3_keccak_f1600(a: &mut [u64; 25], rc: &[u64; 24]) {
    match super::cpu::HaveSha3::check() {
        // SAFETY: This branch is only called if the CPU has the SHA3 feature.
        Some(_) => unsafe { super::sha3_keccak_f1600_alt::sha3_keccak_f1600(a, rc) },
        None => super::sha3_keccak_f1600::sha3_keccak_f1600(a, rc),
    }
}
