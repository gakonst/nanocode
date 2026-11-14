// Written for Graviola by Joe Birr-Pixton, 2026.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

/// On my aarch64 computer, sha3_keccak2_f1600 is way more than 2x faster than
/// sha3_keccak4_f1600.  So this function simply calls it twice.
#[inline]
pub(crate) fn sha3_keccak4_f1600(a: &mut [[u64; 25]; 4], rc: &[u64; 24]) {
    match super::cpu::HaveSha3::check() {
        // SAFETY: This branch is only called if the CPU has the SHA3 feature.
        Some(_) => unsafe {
            for a in a.as_chunks_mut().0.iter_mut() {
                super::sha3_keccak2_f1600::sha3_keccak2_f1600(a, rc);
            }
        },
        None => {
            for a in a.iter_mut() {
                super::sha3_keccak_f1600::sha3_keccak_f1600(a, rc);
            }
        }
    }
}
