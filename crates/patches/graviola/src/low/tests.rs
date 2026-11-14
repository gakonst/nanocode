// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

fn bignum_mux_equiv(p: u64, x_if_p: &[u64], y_if_not_p: &[u64]) {
    let mut model_z = vec![0; x_if_p.len()];
    let mut real_z = vec![0; x_if_p.len()];
    model::bignum_mux(p, &mut model_z, x_if_p, y_if_not_p);
    super::bignum_mux(p, &mut real_z, x_if_p, y_if_not_p);
    assert_eq!(model_z, real_z);
}

#[test]
fn bignum_mux() {
    bignum_mux_equiv(0, &[0u64; 4], &[1u64; 4]);
    bignum_mux_equiv(1, &[1u64; 4], &[0xff; 4]);
    bignum_mux_equiv(u64::MAX, &[1; 1], &[0; 1]);
}

#[test]
fn zeroise() {
    for n in 0..1024 {
        zeroise_equiv(n);
    }
}

fn zeroise_equiv(len: usize) {
    let expect = vec![0x00u8; len];
    let mut bytes = vec![0xffu8; len];
    super::zeroise(&mut bytes);
    assert_eq!(expect, bytes);
}

#[test]
fn ct_equal() {
    ct_equal_equiv(&[], &[]);

    for n in 1..1024 {
        let a = vec![0u8; n];

        for i in 0..n {
            let mut b = vec![0u8; n];
            b[i] = i as u8;
            ct_equal_equiv(&a, &b);
        }
    }
}

fn ct_equal_equiv(a: &[u8], b: &[u8]) {
    assert_eq!(a == b, super::ct_equal(a, b));
}

mod model {
    pub(super) fn bignum_mux(p: u64, z: &mut [u64], x_if_p: &[u64], y_if_not_p: &[u64]) {
        if p > 0 {
            z.copy_from_slice(x_if_p);
        } else {
            z.copy_from_slice(y_if_not_p);
        }
    }
}

#[test]
fn test_edwards25519_decode() {
    use hex::FromHex;

    // valid pubkey from RFC 8032
    let pk_hex = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
    let pk = <[u8; 32]>::from_hex(pk_hex).unwrap();
    let mut point = [0u64; 8];
    assert!(super::edwards25519_decode(&mut point, &pk));

    // invalid pubkey
    assert!(!super::edwards25519_decode(&mut point, &[0xffu8; 32]));
}
