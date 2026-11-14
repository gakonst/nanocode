// Written for Graviola by Joe Birr-Pixton, 2026.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

#[inline]
pub(crate) fn sha3_keccak4_f1600(a: &mut [[u64; 25]; 4], rc: &[u64; 24]) {
    super::sha3_keccak4_f1600_alt::sha3_keccak4_f1600(a, rc, &RHO8, &RHO56);
}

static RHO8: [u64; 4] = [
    0x0605040302010007,
    0x0E0D0C0B0A09080F,
    0x1615141312111017,
    0x1E1D1C1B1A19181F,
];
static RHO56: [u64; 4] = [
    0x0007060504030201,
    0x080F0E0D0C0B0A09,
    0x1017161514131211,
    0x181F1E1D1C1B1A19,
];
