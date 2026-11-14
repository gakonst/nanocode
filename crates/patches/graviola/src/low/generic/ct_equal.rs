// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use crate::low::ct_compare_bytes;

pub(crate) fn ct_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    // SAFETY: prior code guarantees a.len() == b.len()
    let diff = unsafe { ct_compare_bytes(a.as_ptr(), b.as_ptr(), a.len()) };
    diff == 0
}
