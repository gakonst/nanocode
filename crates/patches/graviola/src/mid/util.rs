// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// Once const generics is completed this should be able to be
// done better that way.

macro_rules! little_endian {
    ([u64; $N:literal], $fn_array_to:ident, $fn_slice_to:ident, $fn_to_bytes:ident) => {
        pub(crate) fn $fn_array_to(b: &[u8; $N * 8]) -> [u64; $N] {
            let mut r = [0; $N];
            for (ir, b) in (0..$N).zip(b.chunks_exact(8)) {
                r[ir] = u64::from_le_bytes(b.try_into().unwrap());
            }
            r
        }

        #[allow(dead_code)]
        pub(crate) fn $fn_slice_to(bytes: &[u8]) -> Option<[u64; $N]> {
            let as_array: [u8; $N * 8] = bytes.try_into().ok()?;
            Some($fn_array_to(&as_array))
        }

        #[allow(dead_code)]
        pub(crate) fn $fn_to_bytes(v: &[u64; $N]) -> [u8; $N * 8] {
            let mut r = [0u8; $N * 8];
            for (iv, rb) in (0..$N).zip(r.chunks_exact_mut(8)) {
                rb.copy_from_slice(&v[iv].to_le_bytes());
            }
            r
        }
    };
}

little_endian!(
    [u64; 4],
    little_endian_to_u64x4,
    little_endian_slice_to_u64x4,
    u64x4_to_little_endian
);
little_endian!(
    [u64; 8],
    little_endian_to_u64x8,
    little_endian_slice_to_u64x8,
    u64x8_to_little_endian
);

macro_rules! big_endian {
    ([u64; $N:literal], $fn_array_to:ident, $fn_slice_to:ident, $fn_slice_any_size_to:ident, $fn_to_bytes:ident) => {
        pub(crate) fn $fn_array_to(b: &[u8; $N * 8]) -> [u64; $N] {
            let mut r = [0; $N];
            for (ir, b) in (0..$N).zip(b.chunks_exact(8).rev()) {
                r[ir] = u64::from_be_bytes(b.try_into().unwrap());
            }
            r
        }

        pub(crate) fn $fn_slice_to(bytes: &[u8]) -> Option<[u64; $N]> {
            let as_array: [u8; $N * 8] = bytes.try_into().ok()?;
            Some($fn_array_to(&as_array))
        }

        pub(crate) fn $fn_slice_any_size_to(bytes: &[u8]) -> Option<[u64; $N]> {
            // short circuit for correct lengths
            if let Ok(array) = bytes.try_into() {
                return Some($fn_array_to(array));
            }

            fn add_leading_zeroes(bytes: &[u8]) -> [u64; $N] {
                let mut tmp = [0u8; $N * 8];
                tmp[$N * 8 - bytes.len()..].copy_from_slice(bytes);
                $fn_array_to(&tmp)
            }

            fn remove_leading_zeroes(mut bytes: &[u8]) -> Option<[u64; $N]> {
                loop {
                    // remove the next leading zero
                    match bytes.split_first() {
                        Some((first, remain)) if *first == 0x00 => {
                            if let Ok(array) = remain.try_into() {
                                return Some($fn_array_to(array));
                            }
                            bytes = remain;
                        }
                        _ => return None,
                    }
                }
            }

            if bytes.len() < $N * 8 {
                Some(add_leading_zeroes(bytes))
            } else {
                remove_leading_zeroes(bytes)
            }
        }

        pub(crate) fn $fn_to_bytes(v: &[u64; $N]) -> [u8; $N * 8] {
            let mut r = [0u8; $N * 8];
            for (iv, rb) in (0..$N).zip(r.chunks_exact_mut(8).rev()) {
                rb.copy_from_slice(&v[iv].to_be_bytes());
            }
            r
        }
    };
}

big_endian!(
    [u64; 4],
    big_endian_to_u64x4,
    big_endian_slice_to_u64x4,
    big_endian_slice_any_size_to_u64x4,
    u64x4_to_big_endian
);
big_endian!(
    [u64; 6],
    big_endian_to_u64x6,
    big_endian_slice_to_u64x6,
    big_endian_slice_any_size_to_u64x6,
    u64x6_to_big_endian
);
