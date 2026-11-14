// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use crate::error::Error;
use crate::low;
use crate::mid::sha2::Sha512Context;
use crate::mid::util;

/// The little-endian encoded order of the base-point `B`,
/// `L := 2^252 + 27742317777372353535851937790883648493`.
const ORDER: [u64; 4] = [
    0x5812631a5cf5d3ed,
    0x14def9dea2f79cd6,
    0x0000000000000000,
    0x1000000000000000,
];

pub(crate) struct SigningKey {
    seed: [u8; 32],
    s: UnreducedScalar,
    prefix: [u8; 32],
    verifying_key: VerifyingKey,
}

impl SigningKey {
    pub(crate) fn from_seed(seed: &[u8; 32]) -> Self {
        low::ct::secret_slice(seed);

        // Step: rfc8032 5.1.5.1, 5.1.5.2
        // `h := SHA512(seed)`
        // `s := ed25519-clamp(h[0..32])`
        // `prefix := h[32..64]`
        let (s, prefix): ([u8; 32], [u8; 32]) = {
            let mut ctx = Sha512Context::new();
            ctx.update(seed);
            let h = ctx.finish();

            (h[0..32].try_into().unwrap(), h[32..64].try_into().unwrap())
        };

        // Step: rfc8032 5.1.5.3, 5.1.5.4
        // Compute `[s]B` and compress to get the public key bytes
        let s = UnreducedScalar::clamp_from_le_bytes(&s);
        let point = s.base_mul();
        let bytes = point.compress().0;

        Self {
            seed: *seed,
            prefix,
            s,
            verifying_key: low::ct::into_public(VerifyingKey { point, bytes }),
        }
    }

    /// `PureEd25519` signing
    pub(crate) fn sign(&self, msg: &[u8]) -> [u8; 64] {
        // Step: rfc8032 5.1.6.2
        // Compute the deterministic nonce
        // `r := SHA-512(dom2(F, C) || prefix || msg) mod L`
        let r: Scalar = {
            let r = ed25519_digest(&self.prefix, msg, &[]);
            Scalar::reduce_from_le_bytes(&r)
        };

        // Step: rfc8032 5.1.6.3
        // Compute the commitment point `R := [r]B`.
        let rb = r.base_mul();

        // `sig := (R || S)`
        // Start by writing `R` into the first 32 bytes of `sig`.
        let sig_r = rb.compress();

        // Step: rfc8032 5.1.6.4
        // Compute the challenge `k := SHA512(dom2(F, C) || R || A || PH(msg)) mod L`
        let k: Scalar = {
            let k = ed25519_digest(&sig_r.0, &self.verifying_key().as_bytes(), msg);
            Scalar::reduce_from_le_bytes(&k)
        };

        // Step: rfc8032 5.1.6.5
        // Compute the proof `S := (k * s + r) mod L`
        let s = Scalar::madd_n25519(&k.0, &self.s.0, &r.0);

        let mut sig = [0u8; 64];
        sig[0..32].copy_from_slice(&sig_r.0);
        sig[32..64].copy_from_slice(&s.to_le_bytes());
        low::ct::into_public(sig)
    }

    pub(crate) fn verifying_key(&self) -> &VerifyingKey {
        &self.verifying_key
    }

    pub(crate) fn as_seed_bytes(&self) -> &[u8; 32] {
        &self.seed
    }
}

impl Drop for SigningKey {
    fn drop(&mut self) {
        low::zeroise(&mut self.seed);
        low::zeroise(&mut self.s.0);
        low::zeroise(&mut self.prefix);
        // clearly not necessary, but makes zeroisation easier to test
        low::zeroise(&mut self.verifying_key.point.0);
        low::zeroise(&mut self.verifying_key.bytes);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct VerifyingKey {
    bytes: [u8; 32],
    point: EdwardsPoint,
}

impl VerifyingKey {
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let Ok(bytes) = bytes.try_into() else {
            return Err(Error::WrongLength);
        };

        EdwardsPoint::decompress_from(bytes).map(|point| Self {
            bytes: *bytes,
            point,
        })
    }

    /// `PureEd25519` signature verification
    pub(crate) fn verify(&self, sig: &[u8; 64], msg: &[u8]) -> Result<(), Error> {
        // Step: rfc8032 5.1.7.1 (A' := A is an invariant for `self`)
        let (r_sig, s) = sig.split_at(32);

        // INVARIANT: this unwrap is unreachable: sig is [u8; 64]
        // so its split at 32 yields two 32-length slices.
        let s = s.try_into().unwrap();

        // S must be in the range [0, order) to prevent signature malleability.
        let s = Scalar::try_from_le_bytes(s).ok_or(Error::BadSignature)?;

        // Step: rfc8032 5.1.7.2
        // Compute the challenge `k := SHA512(dom2(F, C) || R || A || msg)`
        let k = {
            let k = ed25519_digest(r_sig, &self.bytes, msg);
            Scalar::reduce_from_le_bytes(&k)
        };

        // Step: rfc8032 5.1.7.3
        // Compute `R := [S]B - [k]A`.
        let r = EdwardsPoint::scalarmuldouble(&k, &self.point.negate(), &s).compress();

        if r_sig == r.0 {
            Ok(())
        } else {
            Err(Error::BadSignature)
        }
    }

    pub(crate) fn as_bytes(&self) -> [u8; 32] {
        self.bytes
    }
}

/// In ed25519 format, the curve point (x, y) is determined by the
/// y-coordinate and the sign of x.
///
/// The first 255 bits of a `CompressedEdwardsY` represent the y-coordinate.
/// The high bit of the 32nd byte gives the sign of x.
struct CompressedEdwardsY([u8; 32]);

/// Represents a point (x, y) on the edwards25519 curve.
#[derive(Clone, Debug)]
pub(crate) struct EdwardsPoint([u64; 8]);

impl EdwardsPoint {


    /// Compute `A := [scalar]Point + [bscalar]B`.
    fn scalarmuldouble(scalar: &Scalar, point: &Self, bscalar: &Scalar) -> Self {
        let mut out = Self([0u64; 8]);
        low::edwards25519_scalarmuldouble(&mut out.0, &scalar.0, &point.0, &bscalar.0);
        out
    }

    /// Compute `B := -A` for this curve point.
    ///
    /// In our affine representation, this is simply (x, y) := (-x, y).
    fn negate(&self) -> Self {
        let x = self.0[..4].try_into().unwrap();
        let mut x_neg = [0u64; 4];
        low::bignum_neg_p25519(&mut x_neg, x);

        let mut out = [0u64; 8];
        out[..4].copy_from_slice(&x_neg);
        out[4..].copy_from_slice(&self.0[4..]);

        Self(out)
    }

    /// Try to decompress a curve point from input bytes.
    ///
    /// Returns `Err(Error::NotOnCurve)` if the input is not reduced, not
    /// on the curve, or not canonically encoded.
    fn decompress_from(compressed: &[u8; 32]) -> Result<Self, Error> {
        let mut point = Self([0u64; 8]);
        if low::edwards25519_decode(&mut point.0, compressed) {
            Ok(point)
        } else {
            Err(Error::NotOnCurve)
        }
    }

    /// Encode edwards25519 point into compressed form as 256-bit number
    fn compress(&self) -> CompressedEdwardsY {
        // Do this in Rust to avoid the pessimistic endian handling in the
        // aarch64 s2n-bignum `edwards25519_encode` impl.
        //
        // godbolt: <https://godbolt.org/z/nKMWfe1hj>

        // Load lowest word of x coordinate
        let p = &self.0;
        let xb = p[0];
        // Load y coordinate as [y0, y1, y2, y3]
        let y0 = p[4];
        let y1 = p[5];
        let y2 = p[6];
        let y3 = p[7];

        // Compute the encoded form, making the LSB of x the MSB of the encoding
        let y3 = (y3 & 0x7fffffffffffffff) | (xb << 63);

        let mut out = CompressedEdwardsY([0u8; 32]);
        out.0[0..8].copy_from_slice(&y0.to_le_bytes());
        out.0[8..16].copy_from_slice(&y1.to_le_bytes());
        out.0[16..24].copy_from_slice(&y2.to_le_bytes());
        out.0[24..32].copy_from_slice(&y3.to_le_bytes());
        out
    }
}

/// An unreduced 256-bit little-endian scalar.
struct UnreducedScalar([u64; 4]);

impl UnreducedScalar {
    /// Read a uniformly random 32-byte little-endian string, and clamp it.
    fn clamp_from_le_bytes(x: &[u8; 32]) -> Self {
        let mut x = *x;

        // Clamp the scalar:
        // <https://mailarchive.ietf.org/arch/msg/cfrg/pt2bt3fGQbNF8qdEcorp-rJSJrc/>
        // <https://neilmadden.blog/2020/05/28/whats-the-curve25519-clamping-all-about/>
        x[0] &= 0b1111_1000;
        x[31] &= 0b0111_1111;
        x[31] |= 0b0100_0000;

        Self(util::little_endian_to_u64x4(&x))
    }

    /// Scalar multiply this scalar by the base-point: `[self]B`.
    fn base_mul(&self) -> EdwardsPoint {
        let mut point = EdwardsPoint([0u64; 8]);
        low::edwards25519_scalarmulbase(&mut point.0, &self.0);
        point
    }
}

/// A little-endian 256-bit scalar reduced modulo [`ORDER`].
struct Scalar([u64; 4]);

impl Scalar {
    /// Read a 256-bit little-endian number from the non-secret input bytes,
    /// additionally verifying that it's a valid `Scalar` reduced modulo
    /// [`ORDER`].
    fn try_from_le_bytes(x: &[u8; 32]) -> Option<Self> {
        let s = util::little_endian_to_u64x4(x);

        (low::bignum_cmp_lt(&s, &ORDER) != 0).then_some(Self(s))
    }

    /// Reduce a 512-bit little-endian scalar modulo [`ORDER`].
    fn reduce_from_le_bytes(x: &[u8; 64]) -> Self {
        let mut s = Self([0u64; 4]);
        low::bignum_mod_n25519(&mut s.0, &util::little_endian_to_u64x8(x));
        s
    }

    fn to_le_bytes(&self) -> [u8; 32] {
        util::u64x4_to_little_endian(&self.0)
    }

    /// Scalar multiply by the base-point: `[self]B`
    fn base_mul(&self) -> EdwardsPoint {
        let mut point = EdwardsPoint([0u64; 8]);
        low::edwards25519_scalarmulbase(&mut point.0, &self.0);
        point
    }

    /// Compute `z := (x * y + c)` modulo [`ORDER`].
    fn madd_n25519(x: &[u64; 4], y: &[u64; 4], c: &[u64; 4]) -> Self {
        let mut z = Self([0u64; 4]);
        low::bignum_madd_n25519(&mut z.0, x, y, c);
        z
    }
}

/// This is `H(..) := SHA-512(dom2(phflag, ctx) || ..)` from rfc8032 5.1, with
/// phflag=0 and ctx="".
///
/// `dom2(phflag, ctx)` is therefore empty, so this is simply the concatenation
/// of the arguments.
fn ed25519_digest(x1: &[u8], x2: &[u8], x3: &[u8]) -> [u8; 64] {
    let mut h = Sha512Context::new();
    h.update(x1);
    h.update(x2);
    h.update(x3);
    h.finish()
}

