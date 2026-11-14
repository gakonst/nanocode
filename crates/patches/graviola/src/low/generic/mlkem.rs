// Written for Graviola by Joe Birr-Pixton, 2026.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

/// Unpack ML-KEM polynomial coefficients as 12-bit numbers
///
/// Input a[384] (bytes); output r[256] (signed 16-bit words)
///
/// This accepts an array of 384 bytes and unpacks them into 256 16-bit numbers
/// in the range 0 <= a[i] < 2^12 (typically they will be < 3329, the ML-KEM prime).
pub(crate) fn mlkem_frombytes(r: &mut [i16; 256], a: &[u8; 384]) {
    for (r, a) in r.chunks_exact_mut(2).zip(a.chunks_exact(3)) {
        r[0] = (a[0] as i16) | (((a[1] & 0x0f) as i16) << 8);
        r[1] = ((a[1] >> 4) as i16) | ((a[2] as i16) << 4);
    }
}

/// Reorder ML-KEM polynomial coefficients for the current implementation
///
/// Input/output a[256] (signed 16-bit words)
///
/// This reorders coefficients from their natural order into the order
/// used by the rest of the implementation.  The x86_64 version of this
/// function produces the layout required by the AVX2 NTT; this
/// implementation (and the functions it works alongside, like
/// [`mlkem_frombytes`]) uses the natural order, so there is nothing to do.
pub(crate) fn mlkem_unpack(_a: &mut [i16; 256]) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpacks_known_values() {
        let mut a = [0u8; 384];
        // first pair of coefficients: 0xabc and 0xdef, packed little-endian
        // per FIPS 203 ByteEncode_12
        a[0] = 0xbc;
        a[1] = 0xfa;
        a[2] = 0xde;

        let mut r = [0i16; 256];
        mlkem_frombytes(&mut r, &a);
        assert_eq!(r[0], 0xabc);
        assert_eq!(r[1], 0xdef);
        assert_eq!(&r[2..], &[0i16; 254]);
    }

    #[test]
    fn output_is_at_most_12_bits() {
        let a = [0xffu8; 384];
        let mut r = [0i16; 256];
        mlkem_frombytes(&mut r, &a);
        assert_eq!(r, [0xfff; 256]);
    }
}
