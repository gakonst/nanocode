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

