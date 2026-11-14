// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use super::hash::{Hash, HashContext};
use crate::Error;
use crate::mid::rng::RandomSource;

/// This is EMSA-PKCS1-v1_5-ENCODE
///
/// `out` is the modulus-size output buffer.
/// `digest_info` is a constant `DIGESTINFO_*` from this module.
/// `hash` is the hash of the message.
///
/// panics if the encoding cannot fit.  this is not reachable
/// for the combinations of hash functions / key sizes that
/// we support.
pub(crate) fn encode_pkcs1_sig(out: &mut [u8], digest_info: &[u8], hash: &[u8]) {
    // 1.  Apply the hash function to the message M to produce a hash
    //     value H:
    // (done by caller)

    // 2.  Encode the algorithm ID for the hash function (...)
    // (in fact, we do the common cheat of concatenating the fixed encoding
    // in `digest_info` and the hash.  see `digestinfos_are_correct` below
    // for cross-checking with a full DER encoder.)

    // 3.  If emLen < tLen + 11, output "intended encoded message length
    //     too short" and stop.
    let t_len = digest_info.len() + hash.len();
    let em_len = out.len();
    assert!(em_len >= t_len + 11);

    // 4. Generate an octet string PS consisting of emLen - tLen - 3
    //    octets with hexadecimal value 0xff.  The length of PS will be
    //    at least 8 octets.
    //
    //  5.  Concatenate PS, the DER encoding T, and other padding to form
    //      the encoded message EM as
    //
    //         EM = 0x00 || 0x01 || PS || 0x00 || T.
    //
    // (we do this in a slightly more obvious way, and minimising
    // bounds checks...)

    let (padding, digest_info_out) = out.split_at_mut(em_len - t_len);
    let (prefix_out, hash_out) = digest_info_out.split_at_mut(digest_info.len());
    let (leader, rest) = padding.split_first_mut().unwrap();
    let (top_sep, rest) = rest.split_first_mut().unwrap();
    let (bot_sep, ps) = rest.split_last_mut().unwrap();

    *leader = 0x00;
    *top_sep = 0x01;
    ps.fill(0xff);
    *bot_sep = 0x00;
    prefix_out.copy_from_slice(digest_info);
    hash_out.copy_from_slice(hash);
}

pub(crate) static DIGESTINFO_SHA256: &[u8] = &[
    0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05,
    0x00, 0x04, 0x20,
];

pub(crate) static DIGESTINFO_SHA384: &[u8] = &[
    0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02, 0x05,
    0x00, 0x04, 0x30,
];

pub(crate) static DIGESTINFO_SHA512: &[u8] = &[
    0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05,
    0x00, 0x04, 0x40,
];

/// This is EMSA-PSS-ENCODE.
///
/// `sLen` is fixed as `hLen`.
/// `MGF` is `MGF1` with hash `H`.
/// `out` is the modulus-length output buffer.
/// `hash` is the message hash, made by the caller using `H`.
/// `rng` is used to generate the salt.
pub(crate) fn encode_pss_sig<H: Hash>(
    out: &mut [u8],
    rng: &mut dyn RandomSource,
    hash: &[u8],
) -> Result<(), Error> {
    // 1.  If the length of M is greater than the input limitation for the
    //     hash function (2^61 - 1 octets for SHA-1), output "message too
    //     long" and stop.
    // 2.  Let mHash = Hash(M), an octet string of length hLen.
    // (by caller)

    // 3.  If emLen < hLen + sLen + 2, output "encoding error" and stop.
    let s_len = hash.len();
    let h_len = hash.len();
    let em_len = out.len();
    assert!(em_len >= h_len + s_len + 2);

    // 4.  Generate a random octet string salt of length sLen; if sLen = 0,
    //     then salt is the empty string.
    let mut salt = H::zeroed_output();
    assert_eq!(salt.as_ref().len(), s_len);
    rng.fill(salt.as_mut())?;

    // 5.  Let
    //       M' = (0x)00 00 00 00 00 00 00 00 || mHash || salt;
    //     M' is an octet string of length 8 + hLen + sLen with eight
    //     initial zero octets.
    let m_prime = [&[0u8; 8], hash, salt.as_ref()];

    // 6.  Let H = Hash(M'), an octet string of length hLen.
    let mut ctx = H::new();
    for d in m_prime {
        ctx.update(d);
    }
    let h = ctx.finish();
    assert_eq!(h.as_ref().len(), h_len);

    // 7.  Generate an octet string PS consisting of emLen - sLen - hLen - 2
    //     zero octets.  The length of PS may be 0.
    // 8.  Let DB = PS || 0x01 || salt; DB is an octet string of length
    //     emLen - hLen - 1.
    let (db, h_bc) = out.split_at_mut(em_len - h_len - 1);
    let (ps, ps_sep_salt) = db.split_at_mut(em_len - s_len - h_len - 2);
    let (ps_sep, salt_out) = ps_sep_salt.split_first_mut().unwrap();
    ps.fill(0x00);
    *ps_sep = 0x01;
    salt_out.copy_from_slice(salt.as_ref());

    // 9.  Let dbMask = MGF(H, emLen - hLen - 1).
    // 10. Let maskedDB = DB \xor dbMask.
    mgf1_xor::<H>(h.as_ref(), h_len, db);

    // 11. Set the leftmost 8emLen - emBits bits of the leftmost octet in
    //     maskedDB to zero.
    db[0] &= 0x7f;

    // 12. Let EM = maskedDB || H || 0xbc.
    let (bc, h_out) = h_bc.split_last_mut().unwrap();
    h_out.copy_from_slice(h.as_ref());
    *bc = 0xbc;

    // 13. Output EM.
    Ok(())
}

/// This is EMSA-PSS-VERIFY.
///
/// `sLen` is fixed as `hLen`, like `encode_pss_sig`.
///
/// `MGF` is `MGF1` with hash `H`.
///
/// `em` is the modulus-length input and temporary buffer.
///
/// `m_hash` is the message hash, made by the caller using `H`.
pub(crate) fn verify_pss_sig<H: Hash>(em: &mut [u8], m_hash: &[u8]) -> Result<(), Error> {
    // 1.   If the length of M is greater than the input limitation for
    //      the hash function (2^61 - 1 octets for SHA-1), output
    //      "inconsistent" and stop.
    // 2.   Let mHash = Hash(M), an octet string of length hLen.
    // (done by caller)

    // 3.   If emLen < hLen + sLen + 2, output "inconsistent" and stop.
    let em_len = em.len();
    let h_len = m_hash.len();
    let s_len = h_len;
    if em_len < h_len + s_len + 2 {
        return Err(Error::BadSignature);
    }

    // 4.   If the rightmost octet of EM does not have hexadecimal value
    //      0xbc, output "inconsistent" and stop.
    let (bc, em) = em.split_last_mut().unwrap();
    if *bc != 0xbc {
        return Err(Error::BadSignature);
    }

    // 5.   Let maskedDB be the leftmost emLen - hLen - 1 octets of EM,
    //      and let H be the next hLen octets.
    let (masked_db, h) = em.split_at_mut(em_len - h_len - 1);

    // 6.   If the leftmost 8emLen - emBits bits of the leftmost octet in
    //      maskedDB are not all equal to zero, output "inconsistent" and
    //      stop.
    if masked_db[0] & 0x80 != 0x00 {
        return Err(Error::BadSignature);
    }

    // 7.   Let dbMask = MGF(H, emLen - hLen - 1).
    // 8.   Let DB = maskedDB \xor dbMask.
    mgf1_xor::<H>(h, h_len, masked_db);
    let db = masked_db;

    // 9.   Set the leftmost 8emLen - emBits bits of the leftmost octet
    //      in DB to zero.
    db[0] &= 0x7f;

    // 10.  If the emLen - hLen - sLen - 2 leftmost octets of DB are not
    //      zero or if the octet at position emLen - hLen - sLen - 1 (the
    //      leftmost position is "position 1") does not have hexadecimal
    //      value 0x01, output "inconsistent" and stop.
    // 11.  Let salt be the last sLen octets of DB.
    let (zeroes, one_salt) = db.split_at(em_len - h_len - s_len - 2);
    let (one, salt) = one_salt.split_first().unwrap();
    if zeroes.iter().any(|z| *z != 0x00) || *one != 0x01 {
        return Err(Error::BadSignature);
    }

    // 12.  Let

    //         M' = (0x)00 00 00 00 00 00 00 00 || mHash || salt ;

    //      M' is an octet string of length 8 + hLen + sLen with eight
    //      initial zero octets.
    let m_prime = [&[0u8; 8], m_hash, salt];

    // 13.  Let H' = Hash(M'), an octet string of length hLen.
    let mut ctx = H::new();
    for d in m_prime {
        ctx.update(d);
    }
    let h_prime = ctx.finish();

    // 14.  If H = H', output "consistent".  Otherwise, output
    //      "inconsistent".
    if h_prime.ct_equal(h) {
        Ok(())
    } else {
        Err(Error::BadSignature)
    }
}

/// Compute MGF1-H, and XOR the result into `out`.
fn mgf1_xor<H: Hash>(seed: &[u8], h_len: usize, out: &mut [u8]) {
    for (chunk, counter) in out.chunks_mut(h_len).zip(0u32..) {
        let mut ctx = H::new();
        ctx.update(seed);
        ctx.update(&counter.to_be_bytes());
        let term = ctx.finish();

        for (out, t) in chunk.iter_mut().zip(term.as_ref().iter()) {
            *out ^= *t;
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::high::asn1::{self, Type, oid, pkix};
    use crate::high::hash;
    use crate::mid::rng::SliceRandomSource;

    #[test]
    fn digestinfo_sha256_is_correct() {
        let hash = [0xaa; 32];

        let di = pkix::DigestInfo {
            digestAlgorithm: pkix::AlgorithmIdentifier {
                algorithm: oid::id_sha256.clone(),
                parameters: Some(asn1::Any::Null(asn1::Null)),
            },
            digest: asn1::OctetString::new(&hash),
        };

        let mut correct = vec![0; di.encoded_len()];
        di.encode(&mut asn1::Encoder::new(&mut correct)).unwrap();

        let actual = {
            let mut v = Vec::new();
            v.extend_from_slice(DIGESTINFO_SHA256);
            v.extend_from_slice(&hash);
            v
        };

        println!("correct: {correct:#04x?}");
        assert_eq!(actual, correct);
    }

    #[test]
    fn digestinfo_sha384_is_correct() {
        let hash = [0xaa; 48];

        let di = pkix::DigestInfo {
            digestAlgorithm: pkix::AlgorithmIdentifier {
                algorithm: oid::id_sha384.clone(),
                parameters: Some(asn1::Any::Null(asn1::Null)),
            },
            digest: asn1::OctetString::new(&hash),
        };

        let mut correct = vec![0; di.encoded_len()];
        di.encode(&mut asn1::Encoder::new(&mut correct)).unwrap();

        let actual = {
            let mut v = Vec::new();
            v.extend_from_slice(DIGESTINFO_SHA384);
            v.extend_from_slice(&hash);
            v
        };

        println!("correct: {correct:#04x?}");
        assert_eq!(actual, correct);
    }

    #[test]
    fn digestinfo_sha512_is_correct() {
        let hash = [0xaa; 64];

        let di = pkix::DigestInfo {
            digestAlgorithm: pkix::AlgorithmIdentifier {
                algorithm: oid::id_sha512.clone(),
                parameters: Some(asn1::Any::Null(asn1::Null)),
            },
            digest: asn1::OctetString::new(&hash),
        };

        let mut correct = vec![0; di.encoded_len()];
        di.encode(&mut asn1::Encoder::new(&mut correct)).unwrap();

        let actual = {
            let mut v = Vec::new();
            v.extend_from_slice(DIGESTINFO_SHA512);
            v.extend_from_slice(&hash);
            v
        };

        println!("correct: {correct:#04x?}");
        assert_eq!(actual, correct);
    }

    #[test]
    fn pss_encode_test() {
        // from first wycheproof test in rsa_pss_2048_sha256_mgf1_32_test.json
        let mut buf = [0u8; 256];
        let mut seed = SliceRandomSource(
            b"\xc0\x72\x47\xf0\x8b\xfe\xf7\xe3\xb7\xe8\x8a\x75\x4a\x15\xf1\x85\
              \x7f\x93\x5d\x8b\xe6\x40\xe5\x23\x7c\xb1\x6c\x4d\xa9\x6b\xe0\x6d",
        );
        let hash = b"\xe3\xb0\xc4\x42\x98\xfc\x1c\x14\x9a\xfb\xf4\xc8\x99\x6f\xb9\x24\
                     \x27\xae\x41\xe4\x64\x9b\x93\x4c\xa4\x95\x99\x1b\x78\x52\xb8\x55";
        encode_pss_sig::<hash::Sha256>(&mut buf, &mut seed, hash).unwrap();

        assert_eq!(
            buf,
            [
                0x35, 0x18, 0x38, 0x0b, 0xba, 0x9e, 0xbb, 0x93, 0xd0, 0xc4, 0x11, 0x14, 0x22, 0x3e,
                0x35, 0x26, 0xe7, 0xb9, 0xa1, 0xa5, 0x81, 0xa1, 0xc7, 0x71, 0x36, 0x58, 0xaf, 0xcb,
                0x58, 0x3d, 0x2f, 0x0b, 0x0c, 0x99, 0x50, 0x03, 0x61, 0x1a, 0xba, 0x84, 0x1d, 0xe3,
                0x3a, 0x0c, 0x12, 0xac, 0x9c, 0xaa, 0xb8, 0x23, 0xe8, 0x57, 0xb5, 0x07, 0x4e, 0xe8,
                0x69, 0x13, 0x7d, 0xd6, 0x3b, 0xca, 0xdd, 0x5b, 0x1c, 0xb6, 0xce, 0x0e, 0xa1, 0xe0,
                0xca, 0xf0, 0xfa, 0x8d, 0xf5, 0x41, 0xe9, 0x64, 0x6c, 0x24, 0x82, 0xe3, 0xfd, 0x23,
                0xde, 0x18, 0xe6, 0x39, 0xd0, 0x87, 0x75, 0xc5, 0x58, 0x2f, 0x6b, 0x6f, 0xb1, 0xbf,
                0xa7, 0xf6, 0x1e, 0xaf, 0x04, 0xb5, 0x52, 0xbd, 0x2b, 0x0c, 0x5b, 0x05, 0xb8, 0x47,
                0x9f, 0x28, 0x04, 0x7f, 0x88, 0x61, 0x43, 0x22, 0x51, 0xa7, 0x8b, 0x41, 0x12, 0x61,
                0xa1, 0x7f, 0x5d, 0x8f, 0xd0, 0xc2, 0xdc, 0x6b, 0x17, 0x57, 0xa1, 0x84, 0x9a, 0x19,
                0x95, 0x1a, 0x86, 0x2b, 0x39, 0x79, 0x46, 0x89, 0xf0, 0xb2, 0x62, 0x96, 0xc4, 0x1e,
                0xc0, 0x0f, 0xea, 0x83, 0xe3, 0x90, 0x7a, 0x97, 0xca, 0x7c, 0xc7, 0xae, 0x20, 0xa4,
                0x78, 0x16, 0x22, 0x8f, 0x52, 0x4a, 0x75, 0x7a, 0xc1, 0x6a, 0x7b, 0x30, 0x01, 0xb5,
                0xc3, 0xf2, 0x92, 0x2c, 0xdf, 0x5e, 0x6b, 0xba, 0x52, 0x69, 0xf4, 0x08, 0x1e, 0xe0,
                0xd4, 0x36, 0x4a, 0xcc, 0x9d, 0xef, 0x4f, 0xca, 0x94, 0xec, 0x45, 0x57, 0x74, 0xb3,
                0xbc, 0x6d, 0x2c, 0xc0, 0xaf, 0xad, 0x83, 0x50, 0x38, 0x33, 0xfa, 0xeb, 0x01, 0x08,
                0x96, 0x98, 0x55, 0xa2, 0x15, 0x13, 0x89, 0x73, 0x4e, 0xa9, 0x57, 0x2e, 0xd1, 0x3c,
                0xf4, 0x94, 0xda, 0xd9, 0xc1, 0x63, 0x25, 0x37, 0x4f, 0x2a, 0x1a, 0x35, 0x5e, 0x1e,
                0xf4, 0x22, 0xd7, 0xbc
            ],
        );

        verify_pss_sig::<hash::Sha256>(&mut buf, hash).unwrap();
    }
}
