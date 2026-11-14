// Written for Graviola by Joe Birr-Pixton, 2026.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

/// Compute Keccak-f1600 on the first two states in `a`.
#[inline]
pub(crate) fn sha3_keccak2of4_f1600(a: &mut [[u64; 25]; 4], rc: &[u64; 24]) {
    let (a, _) = a.split_at_mut(2);
    let a = a.try_into().unwrap();

    match super::cpu::HaveSha3::check() {
        // SAFETY: This branch is only called if the CPU has the SHA3 feature.
        Some(_) => unsafe {
            super::sha3_keccak2_f1600::sha3_keccak2_f1600(a, rc);
        },
        None => {
            for a in a.iter_mut() {
                super::sha3_keccak_f1600::sha3_keccak_f1600(a, rc);
            }
        }
    }
}
