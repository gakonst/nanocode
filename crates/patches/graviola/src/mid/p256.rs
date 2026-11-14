// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use core::fmt;
use core::ops::Range;

use super::util;
use crate::Error;
use crate::low;
use crate::mid::rng::{RandomSource, SystemRandom};

mod precomp;

/// A P-256 public key.
#[derive(Clone, Debug)]
pub struct PublicKey {
    point: AffineMontPoint,
    precomp_w5: JacobianMontPointTableW5,
}

impl PublicKey {
    pub(crate) const BYTES: usize = 1 + FieldElement::BYTES + FieldElement::BYTES;

    /// Create an P-256 [`PublicKey`] from a byte slice.
    ///
    /// This must be exactly 65 bytes in length, using the X9.62
    /// uncompressed encoding.  An error is returned if the point is
    /// not on the curve.
    pub fn from_x962_uncompressed(bytes: &[u8]) -> Result<Self, Error> {
        let _entry = low::Entry::new_public();
        let point = AffineMontPoint::from_x962_uncompressed(bytes)?;
        Ok(Self::from_affine(point))
    }

    /// Encodes this public key using the X9.62 uncompressed encoding.
    pub fn as_bytes_uncompressed(&self) -> [u8; Self::BYTES] {
        let _entry = low::Entry::new_public();
        self.point.as_bytes_uncompressed()
    }

    fn from_affine(point: AffineMontPoint) -> Self {
        Self {
            precomp_w5: point.public_precomp_w5(),
            point,
        }
    }

    pub(crate) fn raw_ecdsa_verify(&self, r: &Scalar, s: &Scalar, e: &Scalar) -> Result<(), Error> {
        // 4. Compute: u1 = e s^-1 mod n and u2 = r s^−1 mod n
        let s_inv = s.inv().as_mont();
        let u1 = s_inv.mont_mul(&e.as_mont()).demont();
        let u2 = s_inv.mont_mul(&r.as_mont()).demont();

        // 5. Compute: R = (xR, yR) = u1 G + u2 QU
        //  If R = O, output "invalid" and stop.
        let lhs = JacobianMontPoint::public_base_multiply(&u1);
        let rhs = JacobianMontPoint::public_multiply_w5(&u2, &self.precomp_w5);

        // nb. if lhs == rhs, then we need a doubling rather than addition
        // (because `p256_montjadd` doesn't handle P + P.)
        let point = if lhs.public_eq(&rhs) {
            lhs.double()
        } else {
            JacobianMontPoint::add(&lhs, &rhs)
        };

        if point.public_is_infinity() {
            return Err(Error::BadSignature);
        }

        // 6. Convert the field element xR to an integer xR using the conversion routine specified in Section 2.3.9.
        // 7. Set v = xR mod n.
        let v = point.x_scalar();

        // 8. Compare v and r — if v = r, output "valid", and if v != r, output "invalid".
        match v.public_eq(r) {
            true => Ok(()),
            false => Err(Error::BadSignature),
        }
    }
}

/// A P-256 ephemeral private key.
///
/// This can be used at most once for the Diffie-Hellman operation,
/// and that is the most common usage pattern for key exchange.
pub struct PrivateKey {
    scalar: Scalar,
}

impl PrivateKey {
    /// Generate a new random private key using the system RNG.
    pub fn new_random() -> Result<Self, Error> {
        Self::generate(&mut SystemRandom)
    }

    /// Derive the corresponding public key, and return it in
    /// X9.62 uncompressed encoding.
    pub fn public_key_uncompressed(&self) -> [u8; PublicKey::BYTES] {
        let _entry = low::Entry::new_secret();
        self.public_point().as_bytes_uncompressed()
    }

    /// Do the Diffie-Hellman operation.
    ///
    /// `peer` is the peer's public key (and this type means it was
    /// already checked to be on the curve.)
    ///
    /// Returns a [`SharedSecret`].  May return an error in fault conditions.
    pub fn diffie_hellman(self, peer: &PublicKey) -> Result<SharedSecret, Error> {
        let _entry = low::Entry::new_secret();
        let result = JacobianMontPoint::multiply_w5(&self.scalar, &peer.precomp_w5).as_affine();
        match result.on_curve() {
            true => Ok(SharedSecret(util::u64x4_to_big_endian(
                &result.x().demont().0,
            ))),
            false => Err(Error::NotOnCurve),
        }
    }

    fn public_point(&self) -> AffineMontPoint {
        let point = JacobianMontPoint::base_multiply(&self.scalar).as_affine();
        let point = low::ct::into_public(point);
        match point.on_curve() {
            true => point,
            false => panic!("internal fault"),
        }
    }

    pub(crate) fn generate(rng: &mut dyn RandomSource) -> Result<Self, Error> {
        let _entry = low::Entry::new_secret();
        for _ in 0..64 {
            let mut r = [0u8; Scalar::BYTES];
            rng.fill(&mut r)?;
            if let Ok(p) = Self::from_bytes(&r) {
                return Ok(p);
            }
        }

        Err(Error::RngFailed)
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        Scalar::from_bytes_checked(bytes).map(|scalar| Self {
            scalar: low::ct::into_secret(scalar),
        })
    }
}

impl fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrivateKey").finish_non_exhaustive()
    }
}

/// A P-256 static private key.
///
/// This can be used many times for the Diffie-Hellman operation.
/// It can also be serialized and deserialized.
pub struct StaticPrivateKey(PrivateKey);

impl StaticPrivateKey {
    /// Generate a new random private key using the system RNG.
    pub fn new_random() -> Result<Self, Error> {
        PrivateKey::generate(&mut SystemRandom).map(Self)
    }

    /// Decode a private key from `bytes`.
    ///
    /// `bytes` may be larger or smaller than the size of `n`: excess bytes
    /// must be zero.  If given a variable-sized input, this is deemed a
    /// non-secret property.  Prefer to use fixed-sized inputs.
    ///
    /// An error is returned if the magnitude of the value is larger than
    /// `n` (ie, the input is never reduced mod n),  or the value is zero.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let _entry = low::Entry::new_secret();
        PrivateKey::from_bytes(bytes).map(Self)
    }

    /// Return a fixed-length encoding of this private key's value.
    pub fn as_bytes(&self) -> [u8; Scalar::BYTES] {
        let _entry = low::Entry::new_secret();
        low::ct::into_public(self.0.scalar.as_bytes())
    }

    /// Derive the corresponding public key, and return it in
    /// X9.62 uncompressed encoding.
    pub fn public_key_uncompressed(&self) -> [u8; PublicKey::BYTES] {
        let _entry = low::Entry::new_secret();
        self.0.public_point().as_bytes_uncompressed()
    }

    /// Do the Diffie-Hellman operation.
    ///
    /// `peer` is the peer's public key (and this type means it was
    /// already checked to be on the curve.)
    ///
    /// Returns a [`SharedSecret`].  May return an error in fault conditions.
    pub fn diffie_hellman(&self, peer: &PublicKey) -> Result<SharedSecret, Error> {
        let _entry = low::Entry::new_secret();
        PrivateKey {
            scalar: Scalar(self.0.scalar.0),
        }
        .diffie_hellman(peer)
    }

    pub(crate) fn generate(rng: &mut dyn RandomSource) -> Result<Self, Error> {
        let _entry = low::Entry::new_secret();
        PrivateKey::generate(rng).map(Self)
    }

    pub(crate) fn public_key_x_scalar(&self) -> Scalar {
        let _entry = low::Entry::new_secret();
        self.0.public_point().x_scalar()
    }

    pub(crate) fn raw_ecdsa_sign(&self, k: &Self, e: &Scalar, r: &Scalar) -> Scalar {
        // this is (e + r * d) / k
        let lhs_mont = self
            .0
            .scalar
            .as_mont()
            .mont_mul(&r.as_mont())
            .demont()
            .add(e)
            .as_mont();
        low::ct::into_public(k.0.scalar.inv().mont_mul(&lhs_mont))
    }
}

/// A shared secret output from a P-256 Diffie-Hellman operation.
pub struct SharedSecret(pub [u8; FieldElement::BYTES]);

impl Drop for SharedSecret {
    fn drop(&mut self) {
        low::zeroise(&mut self.0);
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AffineMontPoint {
    xy: [u64; 8],
}

impl AffineMontPoint {
    const X: Range<usize> = 0..4;
    const Y: Range<usize> = 4..8;

    fn from_x962_uncompressed(bytes: &[u8]) -> Result<Self, Error> {
        match bytes.first() {
            Some(&0x04) => (),
            Some(_) => return Err(Error::NotUncompressed),
            None => return Err(Error::WrongLength),
        }

        if bytes.len() != PublicKey::BYTES {
            return Err(Error::WrongLength);
        }

        let (_, xy) = bytes.split_at(1);
        let (x, y) = xy.split_at(FieldElement::BYTES);

        let point = Self::from_xy(
            FieldElement(util::big_endian_slice_to_u64x4(x).unwrap()).as_mont(),
            FieldElement(util::big_endian_slice_to_u64x4(y).unwrap()).as_mont(),
        );

        if !point.on_curve() {
            return Err(Error::NotOnCurve);
        }

        Ok(point)
    }

    fn x_scalar(&self) -> Scalar {
        let bytes = self.as_bytes_uncompressed();
        Scalar::from_bytes_reduced(&bytes[1..1 + Scalar::BYTES]).unwrap()
    }

    fn from_xy(x: FieldElement, y: FieldElement) -> Self {
        let mut r = Self::default();
        r.xy[Self::X].copy_from_slice(&x.0[..]);
        r.xy[Self::Y].copy_from_slice(&y.0[..]);
        r
    }

    fn x(&self) -> FieldElement {
        FieldElement(self.xy[Self::X].try_into().unwrap())
    }

    fn y(&self) -> FieldElement {
        FieldElement(self.xy[Self::Y].try_into().unwrap())
    }

    fn on_curve(&self) -> bool {
        // Compute the curve equation:
        //
        // y ^ 2 === x ^ 3 + ax + b
        //
        // all in GF(p)
        //

        let x = self.x();
        let rhs = x.mont_sqr(); // x ^ 2
        let rhs = rhs.add(&CURVE_A_MONT); // x ^ 2 + a
        let rhs = rhs.mont_mul(&x); // (x ^ 2 + a) * x   equiv  x ^ 3 + ax
        let rhs = rhs.add(&CURVE_B_MONT);

        let lhs = self.y().mont_sqr();

        lhs.public_eq(&rhs)
    }

    fn as_bytes_uncompressed(&self) -> [u8; PublicKey::BYTES] {
        let mut r = [0u8; PublicKey::BYTES];
        let (indicator, xy) = r.split_at_mut(1);
        let (x, y) = xy.split_at_mut(FieldElement::BYTES);

        indicator[0] = 0x04;
        x.copy_from_slice(&util::u64x4_to_big_endian(&self.x().demont().0));
        y.copy_from_slice(&util::u64x4_to_big_endian(&self.y().demont().0));
        r
    }

    #[cfg(test)]
    fn slow_multiply(&self, scalar: &Scalar) -> Self {
        let mut result = JacobianMontPoint::infinity();

        let mut j = JacobianMontPoint::from_affine(self);
        let zero = JacobianMontPoint::zero();

        for bit in scalar.bits() {
            result.add_inplace(&JacobianMontPoint::select(&zero, &j, bit));
            j = j.double();
        }

        result.as_affine()
    }

    fn negate_y(&mut self) {
        let neg_y = self.y().negate_mod_p();
        self.xy[Self::Y].copy_from_slice(&neg_y.0);
    }

    fn maybe_negate_y(&mut self, sign: u8) {
        let y = self.y();
        let neg_y = y.negate_mod_p();
        let result = FieldElement::select(&y, &neg_y, sign);
        self.xy[Self::Y].copy_from_slice(&result.0);
    }

    /// Precomputes a table (with 𝑤=6) for the point `self`
    ///
    /// 64 is the row size, 2**6.
    /// 37 is the table height, ceil(256/7) (Booth encoding gives us
    /// one bit extra free, in exchange for a negation to compute a
    /// negative point from the precomputed positive point -- this is
    /// ~free).
    ///
    /// This should not be used at runtime, since (for brevity) it
    /// does excessive point representation conversions, and recomputes
    /// items in a given row several times (compare `public_precomp_w5`).
    #[cfg(test)]
    fn public_precomp_w7_slow(&self) -> [[Self; 64]; 37] {
        let mut r = [[Self::default(); 64]; 37];

        for window in 0..(256usize.div_ceil(7)) {
            let row = &mut r[window];

            // indices into rows are reduced by 1 since 0G does not require
            // storage.  therefore row[0] is 1G << shift.
            let mut first = JacobianMontPoint::from_affine(self);
            first.double_inplace_n(window * 7);
            row[0] = first.as_affine();

            for r in 1..64 {
                row[r] = row[0].slow_multiply(&Scalar([(r + 1) as u64, 0, 0, 0]));
            }
        }

        r
    }

    fn public_precomp_w5(&self) -> JacobianMontPointTableW5 {
        let mut r = [JacobianMontPoint::zero(); 16];

        // indices into r are intuitively 1-based; index i contains i * G,
        // and 0 * G is not useful to store.
        macro_rules! index {
            ($i:literal) => {
                $i - 1
            };
        }

        r[index!(1)] = JacobianMontPoint::from_affine(self);
        r[index!(2)] = r[index!(1)].double();
        r[index!(3)] = r[index!(1)].add(&r[index!(2)]);
        r[index!(4)] = r[index!(2)].double();
        r[index!(5)] = r[index!(1)].add(&r[index!(4)]);
        r[index!(6)] = r[index!(3)].double();
        r[index!(7)] = r[index!(1)].add(&r[index!(6)]);
        r[index!(8)] = r[index!(4)].double();
        r[index!(9)] = r[index!(1)].add(&r[index!(8)]);
        r[index!(10)] = r[index!(5)].double();
        r[index!(11)] = r[index!(1)].add(&r[index!(10)]);
        r[index!(12)] = r[index!(6)].double();
        r[index!(13)] = r[index!(1)].add(&r[index!(12)]);
        r[index!(14)] = r[index!(7)].double();
        r[index!(15)] = r[index!(1)].add(&r[index!(14)]);
        r[index!(16)] = r[index!(8)].double();

        // now linearise, so table lookup can be type safe
        let mut t = [0; 192];

        for (out, rr) in t.chunks_exact_mut(12).zip(r) {
            out.copy_from_slice(&rr.xyz);
        }

        t
    }

    /// Returns table[(i - 1) * 8] if index > 0, or else AffineMontPoint::default()
    fn lookup_w7(table: &[u64; 512], index: u8) -> Self {
        let mut r = Self::default();
        low::bignum_aff_point_select_p256(&mut r.xy, table, index);
        r
    }

    /// Returns table[(i - 1) * 8] if index > 0, or else AffineMontPoint::default()
    ///
    /// `index` is a public quantity.
    fn public_lookup_w7(table: &[u64; 512], index: u8) -> Self {
        let mut r = Self::default();
        if index > 0 {
            let offs = (index - 1) as usize * 8;
            r.xy.copy_from_slice(&table[offs..offs + 8]);
        }
        r
    }

    fn private_eq(&self, other: &Self) -> bool {
        low::bignum_eq(&self.xy, &other.xy)
    }

    fn is_infinity(&self) -> u8 {
        let zero = Self::default();
        self.private_eq(&zero) as u8
    }
}

#[derive(Clone, Copy, Debug)]
struct JacobianMontPoint {
    xyz: [u64; 12],
}

impl JacobianMontPoint {
    const X: Range<usize> = 0..4;
    const Y: Range<usize> = 4..8;
    const XY: Range<usize> = 0..8;
    const Z: Range<usize> = 8..12;

    fn infinity() -> Self {
        Self {
            xyz: [1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
        }
    }

    fn public_is_infinity(&self) -> bool {
        self.z().public_eq(&FieldElement::default())
    }

    fn zero() -> Self {
        Self { xyz: [0; 12] }
    }

    fn x(&self) -> FieldElement {
        FieldElement(self.xyz[Self::X].try_into().unwrap())
    }

    fn y(&self) -> FieldElement {
        FieldElement(self.xyz[Self::Y].try_into().unwrap())
    }

    fn z(&self) -> FieldElement {
        FieldElement(self.xyz[Self::Z].try_into().unwrap())
    }

    fn set_z(&mut self, fe: &FieldElement) {
        self.xyz[Self::Z].copy_from_slice(&fe.0);
    }

    fn base_multiply(scalar: &Scalar) -> Self {
        Self::multiply_w7::<true>(scalar, &precomp::CURVE_GENERATOR_PRECOMP_W7)
    }

    fn public_base_multiply(scalar: &Scalar) -> Self {
        Self::multiply_w7::<false>(scalar, &precomp::CURVE_GENERATOR_PRECOMP_W7)
    }

    fn multiply_w7<const SECRET: bool>(scalar: &Scalar, precomp: &AffineMontPointTableW7) -> Self {
        let mut terms = scalar.booth_recoded_w7();
        // unwrap: number of terms is constant
        let (digit, sign) = terms.next().unwrap();
        let tmp = if SECRET {
            let mut tmp = AffineMontPoint::lookup_w7(&precomp[0], digit);
            tmp.maybe_negate_y(sign);
            tmp
        } else {
            let mut tmp = AffineMontPoint::public_lookup_w7(&precomp[0], digit);
            if sign > 0 {
                tmp.negate_y();
            }
            tmp
        };

        let mut result = Self::from_affine(&tmp);
        result.set_z(&FieldElement::select(
            &FieldElement::default(),
            &CURVE_ONE_MONT,
            digit,
        ));

        for (index, (digit, sign)) in (1..).zip(terms) {
            let tmp = if SECRET {
                let mut tmp = AffineMontPoint::lookup_w7(&precomp[index], digit);
                tmp.maybe_negate_y(sign);
                tmp
            } else {
                let mut tmp = AffineMontPoint::public_lookup_w7(&precomp[index], digit);
                if sign > 0 {
                    tmp.negate_y();
                }
                tmp
            };

            result.add_inplace_affine(&tmp);
        }

        result
    }

    fn multiply_w5(scalar: &Scalar, precomp: &JacobianMontPointTableW5) -> Self {
        Self::_multiply_w5::<true>(scalar, precomp)
    }

    fn public_multiply_w5(scalar: &Scalar, precomp: &JacobianMontPointTableW5) -> Self {
        Self::_multiply_w5::<false>(scalar, precomp)
    }

    fn _multiply_w5<const SECRET: bool>(
        scalar: &Scalar,
        precomp: &JacobianMontPointTableW5,
    ) -> Self {
        let mut terms = scalar.reversed_booth_recoded_w5();

        let (digit, _, _) = terms.next().unwrap();
        let mut result = if SECRET {
            Self::lookup_w5(precomp, digit)
        } else {
            Self::public_lookup_w5(precomp, digit)
        };
        result.double_inplace_n(5);

        for (digit, sign, last) in terms {
            let tmp = if SECRET {
                let mut tmp = Self::lookup_w5(precomp, digit);
                tmp.maybe_negate_y(sign);
                tmp
            } else {
                let mut tmp = Self::public_lookup_w5(precomp, digit);
                if sign > 0 {
                    tmp.negate_y();
                }
                tmp
            };
            result.add_inplace(&tmp);

            if !last {
                result.double_inplace_n(5);
            }
        }

        result
    }

    fn from_affine(p: &AffineMontPoint) -> Self {
        let mut xyz: [u64; 12] = Default::default();
        xyz[Self::XY].copy_from_slice(&p.xy);
        xyz[Self::Z].copy_from_slice(&CURVE_ONE_MONT.0);
        Self { xyz }
    }

    fn as_affine(&self) -> AffineMontPoint {
        // recover (x, y) from (x / z ^ 2, x / z ^ 3, z)
        let z2 = self.z().mont_sqr();
        let z3 = self.z().mont_mul(&z2);

        // inversion calculated outside montgomery domain
        // (benchmarked vs addition chain in montgomery)
        let z2_inv = z2.demont().inv().as_mont();
        let z3_inv = z3.demont().inv().as_mont();

        let x = self.x().mont_mul(&z2_inv);
        let y = self.y().mont_mul(&z3_inv);

        AffineMontPoint::from_xy(x, y)
    }

    fn public_eq(&self, other: &Self) -> bool {
        // we can't compare these directly, because they could be
        // the same point but with different z factors.  instead,
        // convert one to affine, and then bring it back into
        // jacobian using other's z factors.
        let a = self.as_affine();

        let b_z2 = other.z().mont_sqr();
        let b_z3 = other.z().mont_mul(&b_z2);

        let a_x_b_z2 = a.x().mont_mul(&b_z2);
        let a_y_b_z3 = a.y().mont_mul(&b_z3);

        a_x_b_z2.public_eq(&other.x()) && a_y_b_z3.public_eq(&other.y())
    }

    fn x_scalar(&self) -> Scalar {
        // this is a faster version of `as_affine().x_scalar()`
        let z2 = self.z().mont_sqr();
        let z2_inv = z2.demont().inv().as_mont();
        let x = self.x().mont_mul(&z2_inv).demont();
        Scalar::from_bytes_reduced(&util::u64x4_to_big_endian(&x.0)).unwrap()
    }

    #[must_use]
    fn double(&self) -> Self {
        let mut tmp = Self::zero();
        low::p256_montjdouble(&mut tmp.xyz, &self.xyz);
        tmp
    }

    fn double_inplace_n(&mut self, n: usize) {
        for _ in 0..n {
            let tmp = *self;
            low::p256_montjdouble(&mut self.xyz, &tmp.xyz);
        }
    }

    fn add_inplace(&mut self, p: &Self) {
        let mut r = Self::infinity();
        low::p256_montjadd(&mut r.xyz, &self.xyz, &p.xyz);
        *self = r;
    }

    fn add_inplace_affine(&mut self, p: &AffineMontPoint) {
        let mut r = Self::infinity();
        low::p256_montjmixadd(&mut r.xyz, &self.xyz, &p.xy);
        // XXX: annoyingly, p256_montjmixadd does not handle point at infinity correctly,
        // so do it here
        *self = Self::select(&r, self, p.is_infinity());
    }

    #[must_use]
    fn add(&self, p: &Self) -> Self {
        let mut r = Self::infinity();
        low::p256_montjadd(&mut r.xyz, &self.xyz, &p.xyz);
        r
    }

    /// Return p0 if select == 0, p1 otherwise
    #[must_use]
    fn select(p0: &Self, p1: &Self, select: u8) -> Self {
        let mut r = Self::zero();
        let select = select as u64;
        low::bignum_mux(select, &mut r.xyz[..], &p1.xyz[..], &p0.xyz[..]);
        r
    }

    /// Returns table[i - 1] if index > 0, or else an infinity
    fn lookup_w5(table: &JacobianMontPointTableW5, index: u8) -> Self {
        let mut r = Self::infinity();
        low::bignum_jac_point_select_p256(&mut r.xyz, table, index);
        r
    }

    /// Returns table[i - 1] if index > 0, or else an infinity
    fn public_lookup_w5(table: &JacobianMontPointTableW5, index: u8) -> Self {
        let mut r = Self::infinity();
        if index > 0 {
            let offs = (index as usize - 1) * 12;
            r.xyz.copy_from_slice(&table[offs..offs + 12]);
        }
        r
    }

    fn negate_y(&mut self) {
        let neg_y = self.y().negate_mod_p();
        self.xyz[Self::Y].copy_from_slice(&neg_y.0);
    }

    fn maybe_negate_y(&mut self, sign: u8) {
        let y = self.y();
        let neg_y = y.negate_mod_p();
        let result = FieldElement::select(&y, &neg_y, sign);
        self.xyz[Self::Y].copy_from_slice(&result.0);
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct FieldElement([u64; 4]);

impl FieldElement {
    const BYTES: usize = 32;

    fn inv(&self) -> Self {
        let mut r = Self::default();
        low::bignum_inv_p256(&mut r.0, &self.0);
        r
    }

    /// Montgomery squaring mod p256
    fn mont_sqr(&self) -> Self {
        let mut r = Self::default();
        low::bignum_montsqr_p256(&mut r.0, &self.0);
        r
    }

    /// Addition mod p256
    fn add(&self, other: &Self) -> Self {
        let mut r = Self::default();
        low::bignum_add_p256(&mut r.0, &self.0, &other.0);
        r
    }

    /// Montgomery multiplication mod p256
    fn mont_mul(&self, other: &Self) -> Self {
        let mut r = Self::default();
        low::bignum_montmul_p256(&mut r.0, &self.0, &other.0);
        r
    }

    /// Remove one montgomery factor
    fn demont(&self) -> Self {
        let mut r = Self::default();
        low::bignum_demont_p256(&mut r.0, &self.0);
        r
    }

    /// Add a montgomery factor
    fn as_mont(&self) -> Self {
        let mut r = Self::default();
        low::bignum_tomont_p256(&mut r.0, &self.0);
        r
    }

    fn negate_mod_p(&self) -> Self {
        let mut r = Self::default();
        low::bignum_neg_p256(&mut r.0, &self.0);
        r
    }

    /// Public equality
    fn public_eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }

    /// Return p0 if select == 0, p1 otherwise
    fn select(p0: &Self, p1: &Self, select: u8) -> Self {
        let mut r = Self::default();
        let select = select as u64;
        low::bignum_mux(select, &mut r.0[..], &p1.0[..], &p0.0[..]);
        r
    }
}

#[derive(Default)]
pub struct Scalar([u64; 4]);

impl Scalar {
    pub(crate) const BYTES: usize = 32;

    /// Create a scalar from the given slice, which can be any size.
    ///
    /// If it is larger than 32 bytes, the leading bytes must be
    /// zero (this is deemed a non-secret property).
    ///
    /// This returns an error if the scalar is zero or larger than
    /// the curve order.
    pub(crate) fn from_bytes_checked(bytes: &[u8]) -> Result<Self, Error> {
        let full = Self(util::big_endian_slice_any_size_to_u64x4(bytes).ok_or(Error::WrongLength)?);

        full.into_range_check()
    }

    pub(crate) fn from_bytes_reduced(bytes: &[u8]) -> Result<Self, Error> {
        Ok(
            Self(util::big_endian_slice_any_size_to_u64x4(bytes).ok_or(Error::WrongLength)?)
                .reduce_mod_n(),
        )
    }

    fn into_range_check(self) -> Result<Self, Error> {
        let reduced = self.reduce_mod_n();

        if !reduced.private_eq(&self) || self.is_zero() {
            Err(Error::OutOfRange)
        } else {
            Ok(self)
        }
    }

    pub(crate) fn as_bytes(&self) -> [u8; Self::BYTES] {
        util::u64x4_to_big_endian(&self.0)
    }

    #[cfg(test)]
    fn small_u64(v: u64) -> Self {
        Self([v, 0, 0, 0])
    }

    /// Private test for zero
    pub(crate) fn is_zero(&self) -> bool {
        self.private_eq(&Self::default())
    }

    /// Private equality
    fn private_eq(&self, other: &Self) -> bool {
        low::bignum_eq(&self.0, &other.0)
    }

    /// Public equality
    fn public_eq(&self, other: &Self) -> bool {
        low::bignum_eq(&self.0, &other.0)
    }

    /// Reduce mod n (curve order)
    fn reduce_mod_n(&self) -> Self {
        let mut r = Self::default();
        low::bignum_mod_n256(&mut r.0, &self.0);
        r
    }

    /// Remove one montgomery factor mod n
    fn demont(&self) -> Self {
        let mut r = Self::default();
        low::bignum_demont(&mut r.0, &self.0, &CURVE_ORDER);
        r
    }

    /// Add a montgomery factor mod n
    fn as_mont(&self) -> Self {
        let mut r = Self::default();
        low::bignum_montmul(&mut r.0, &self.0, &CURVE_ORDER_MM, &CURVE_ORDER);
        r
    }

    /// Return 2^512 mod n, ie MM mod n
    #[cfg(test)]
    fn montifier() -> Self {
        let mut r = Self::default();
        let mut tmp = Self::default();
        low::bignum_montifier(&mut r.0, &CURVE_ORDER, &mut tmp.0);
        r
    }

    /// Montgomery multiplication mod n
    ///
    /// Assumes `self` and `other` are in montgomery domain.
    /// Result is in montgomery domain.
    fn mont_mul(&self, other: &Self) -> Self {
        let mut r = Self::default();
        low::bignum_montmul(&mut r.0, &self.0, &other.0, &CURVE_ORDER);
        r
    }

    /// Find the multiplicative inverse of `self` mod n
    fn inv(&self) -> Self {
        let mut r = Self::default();
        let mut temp = [0u64; 4 * 3];
        low::bignum_modinv(&mut r.0, &self.0, &CURVE_ORDER, &mut temp);
        r
    }

    /// Add `self` + `other` mod n
    fn add(&self, other: &Self) -> Self {
        let mut r = Self::default();
        low::bignum_modadd(&mut r.0, &self.0, &other.0, &CURVE_ORDER);
        r
    }

    /// Iterator of the bits of the element, lowest first
    #[cfg(test)]
    fn bits(&self) -> Bits<'_> {
        Bits {
            scalar: self,
            word: 0,
            bit: 0,
        }
    }

    /// Iterator of 37 * 7-bit elements, MSB first, sign bit is separate
    fn booth_recoded_w7(&self) -> BoothRecodeW7 {
        BoothRecodeW7::new(self)
    }

    /// Iterator of 51 * 5-bit elements, LSB first, sign bit and final flag is separate
    fn reversed_booth_recoded_w5(&self) -> BoothRecodeW5 {
        BoothRecodeW5::new(self)
    }
}

impl Drop for Scalar {
    fn drop(&mut self) {
        low::zeroise(&mut self.0);
    }
}

#[cfg(test)]
struct Bits<'a> {
    scalar: &'a Scalar,
    word: usize,
    bit: usize,
}

#[cfg(test)]
impl Iterator for Bits<'_> {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        if self.word > 3 {
            return None;
        }

        let v = (self.scalar.0[self.word] >> self.bit) & 1;

        self.bit += 1;
        if self.bit == 64 {
            self.word += 1;
            self.bit = 0;
        }

        Some(v as u8)
    }
}

struct BoothRecodeW7 {
    // little endian
    bytes: [u8; 33],
    index: usize,
}

impl BoothRecodeW7 {
    fn new(scalar: &Scalar) -> Self {
        let mut bytes = [0u8; 33];

        bytes[0..8].copy_from_slice(&scalar.0[0].to_le_bytes());
        bytes[8..16].copy_from_slice(&scalar.0[1].to_le_bytes());
        bytes[16..24].copy_from_slice(&scalar.0[2].to_le_bytes());
        bytes[24..32].copy_from_slice(&scalar.0[3].to_le_bytes());

        Self { bytes, index: 0 }
    }

    fn recode(v: u8) -> (u8, u8) {
        // see the comment above boringssl's `ec_GFp_nistp_recode_scalar_bits` for
        // references
        let sign = !((v >> 7).wrapping_sub(1));

        let d = (1u16 << 8).wrapping_sub(v as u16).wrapping_sub(1) as u8;
        let d = (d & sign) | (v & !sign);
        let d = (d >> 1).wrapping_add(d & 1);

        (d, sign & 1)
    }
}

impl Iterator for BoothRecodeW7 {
    type Item = (u8, u8);

    fn next(&mut self) -> Option<Self::Item> {
        match self.index {
            0 => {
                self.index += 7;
                Some(Self::recode(self.bytes[0] << 1))
            }

            7..=252 => {
                let offset = (self.index - 1) / 8;
                let shift = (self.index - 1) % 8;
                let value = (((self.bytes[offset] as u16) | ((self.bytes[offset + 1] as u16) << 8))
                    >> shift) as u8;
                self.index += 7;
                Some(Self::recode(value))
            }

            _ => None,
        }
    }
}

struct BoothRecodeW5 {
    // little endian
    bytes: [u8; 33],
    index: usize,
}

impl BoothRecodeW5 {
    fn new(scalar: &Scalar) -> Self {
        let mut bytes = [0u8; 33];

        bytes[0..8].copy_from_slice(&scalar.0[0].to_le_bytes());
        bytes[8..16].copy_from_slice(&scalar.0[1].to_le_bytes());
        bytes[16..24].copy_from_slice(&scalar.0[2].to_le_bytes());
        bytes[24..32].copy_from_slice(&scalar.0[3].to_le_bytes());

        Self { bytes, index: 255 }
    }

    fn recode(v: u8) -> (u8, u8) {
        // see the comment above boringssl's `ec_GFp_nistp_recode_scalar_bits` for
        // references
        let sign = !((v >> 5).wrapping_sub(1));

        let d = (1u16 << 6).wrapping_sub(v as u16).wrapping_sub(1) as u8;
        let d = (d & sign) | (v & !sign);
        let d = (d >> 1).wrapping_add(d & 1);

        (d, sign & 1)
    }
}

impl Iterator for BoothRecodeW5 {
    type Item = (u8, u8, bool);

    fn next(&mut self) -> Option<Self::Item> {
        const MASK: u8 = (1 << (5 + 1)) - 1;

        match self.index {
            5..=255 => {
                let offset = (self.index - 1) / 8;
                let shift = (self.index - 1) % 8;
                let value = (((self.bytes[offset] as u16) | ((self.bytes[offset + 1] as u16) << 8))
                    >> shift) as u8
                    & MASK;
                let (digit, sign) = Self::recode(value);
                self.index -= 5;
                Some((digit, sign, false))
            }

            0 => {
                let value = (self.bytes[0] << 1) & MASK;
                let (digit, sign) = Self::recode(value);
                self.index = usize::MAX;
                Some((digit, sign, true))
            }

            _ => None,
        }
    }
}

/// 37 tables of 6 bits of exponent each.
///
/// 37 := ceil(256 / 7), ie taking 7 bits of exponent at a time.
/// Each 'row' builds in 7 successive doublings.
type AffineMontPointTableW7 = [[u64; 512]; 37];

type JacobianMontPointTableW5 = [u64; 192];

const CURVE_A_MONT: FieldElement = FieldElement([
    0xffff_ffff_ffff_fffc,
    0x0000_0003_ffff_ffff,
    0x0000_0000_0000_0000,
    0xffff_fffc_0000_0004,
]);

const CURVE_B_MONT: FieldElement = FieldElement([
    0xd89c_df62_29c4_bddf,
    0xacf0_05cd_7884_3090,
    0xe5a2_20ab_f721_2ed6,
    0xdc30_061d_0487_4834,
]);

const CURVE_ONE_MONT: FieldElement = FieldElement([
    0x0000_0000_0000_0001,
    0xffff_ffff_0000_0000,
    0xffff_ffff_ffff_ffff,
    0x0000_0000_ffff_fffe,
]);

const CURVE_ORDER: [u64; 4] = [
    0xf3b9_cac2_fc63_2551,
    0xbce6_faad_a717_9e84,
    0xffff_ffff_ffff_ffff,
    0xffff_ffff_0000_0000,
];

const CURVE_ORDER_MM: [u64; 4] = [
    0x83244c95be79eea2,
    0x4699799c49bd6fa6,
    0x2845b2392b6bec59,
    0x66e12d94f3d95620,
];

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use core::mem::size_of_val;

    use super::*;

    const CURVE_GENERATOR: AffineMontPoint = AffineMontPoint {
        xy: [
            0x79e7_30d4_18a9_143c,
            0x75ba_95fc_5fed_b601,
            0x79fb_732b_7762_2510,
            0x1890_5f76_a537_55c6,
            0xddf2_5357_ce95_560a,
            0x8b4a_b8e4_ba19_e45c,
            0xd2e8_8688_dd21_f325,
            0x8571_ff18_2588_5d85,
        ],
    };

    #[test]
    fn generator_on_curve() {
        println!("{CURVE_GENERATOR:x?}");
        assert!(CURVE_GENERATOR.on_curve());
        println!("demont {:x?}", CURVE_GENERATOR.as_bytes_uncompressed());
    }

    #[test]
    fn generate_key_1() {
        let scalar = Scalar::small_u64(1);
        let r = CURVE_GENERATOR.slow_multiply(&scalar);
        assert_eq!(
            r.as_bytes_uncompressed(),
            CURVE_GENERATOR.as_bytes_uncompressed()
        );
        println!("raw {r:x?}");
        println!("fmt {:x?}", r.as_bytes_uncompressed());
    }

    #[test]
    fn point_double() {
        let mut p = JacobianMontPoint::from_affine(&CURVE_GENERATOR);
        println!("p = {p:x?}");
        p = p.double();
        println!("p2 = {p:x?}");
        println!("p2 aff = {:x?}", p.as_affine());
        println!("enc = {:x?}", p.as_affine().as_bytes_uncompressed());
    }

    #[test]
    fn generate_key_2() {
        let scalar = Scalar::small_u64(2);
        let r = CURVE_GENERATOR.slow_multiply(&scalar);
        println!("raw {r:x?}");
        println!("fmt {:x?}", r.as_bytes_uncompressed());
    }

    #[test]
    fn generate_key_3() {
        let scalar = Scalar::small_u64(3);
        let r = JacobianMontPoint::base_multiply(&scalar).as_affine();
        println!("raw {r:x?}");
        println!("fmt {:x?}", r.as_bytes_uncompressed());

        let u = CURVE_GENERATOR.slow_multiply(&scalar);
        println!("raw {u:x?}");
        println!("fmt {:x?}", u.as_bytes_uncompressed());
    }

    #[test]
    fn generate_key_99999999() {
        let scalar = Scalar::small_u64(99999999);
        let r = JacobianMontPoint::base_multiply(&scalar).as_affine();
        println!("raw {r:x?}");
        println!("fmt {:x?}", r.as_bytes_uncompressed());

        let u = CURVE_GENERATOR.slow_multiply(&scalar);
        println!("raw {u:x?}");
        println!("fmt {:x?}", u.as_bytes_uncompressed());
    }

    #[test]
    fn generate_key_known_answer() {
        let bytes = b"\x1F\x55\x45\x23\x08\x50\x8C\x6B\x24\x37\x0F\x22\x1E\xF1\xB3\xF9\x54\x46\xBE\x4F\x8A\x4B\x42\x8A\x5B\x51\xB7\x10\xC2\x68\x4C\x03";
        let private = PrivateKey::from_bytes(bytes).unwrap();
        println!("priv = {private:x?}");
        let public = private.public_key_uncompressed();
        println!("pub = {public:x?}");
        const EXPECTED: &[u8; 65] = b"\x04\xcb\x8a\x14\x1c\xd7\xe4\x07\xaf\x69\xa5\x01\x88\xe9\x1c\xe5\x5d\xcc\xfd\x33\x48\xda\xba\x4a\x9c\x46\x64\x33\x2e\x95\x59\xb6\x81\x44\xfc\x1a\x61\xd8\x41\xe4\xdb\x80\x1b\x33\x51\x20\x12\x1d\x0b\xa4\x84\xb3\xc9\x53\xb3\x1d\x35\x1d\x7f\xa2\x13\x97\xd1\x25\x47";
        assert_eq!(&public, EXPECTED);
        let public_key = PublicKey::from_x962_uncompressed(&public).unwrap();
        assert_eq!(&public_key.as_bytes_uncompressed(), EXPECTED);
    }

    #[test]
    fn test_raw_ecdsa_sign() {
        let private = StaticPrivateKey::from_bytes(b"\xd1\xf6\xbc\xcc\x3e\x5a\x40\x1b\xcc\x2c\x21\xbe\x34\x90\xed\x38\xde\xf4\x93\x7f\x78\x06\x03\xf5\x2b\x23\xb9\xa6\xfa\x9c\xf6\x0e").unwrap();
        let k = StaticPrivateKey::from_bytes(b"\xe7\x95\x37\xa1\xd7\x55\x45\x1f\x8c\x3c\xbf\xf7\x84\xea\x5c\x1c\xdf\xe1\x6b\x1d\x13\xe7\xbf\xbb\x04\xd7\xfd\x90\x57\xee\xee\xf7").unwrap();
        let e = Scalar::from_bytes_checked(b"\x2c\xf2\x4d\xba\x5f\xb0\xa3\x0e\x26\xe8\x3b\x2a\xc5\xb9\xe2\x9e\x1b\x16\x1e\x5c\x1f\xa7\x42\x5e\x73\x04\x33\x62\x93\x8b\x98\x24").unwrap();
        let r = Scalar::from_bytes_checked(b"\x78\xee\x34\xc8\xb6\x52\x29\x54\x96\x19\x79\x45\x2e\x6e\x0c\xe0\x68\x5d\x40\x42\x38\x41\xef\xeb\x06\xfe\x3e\x3f\xf7\xb6\x5d\xf5").unwrap();
        let s = private.raw_ecdsa_sign(&k, &e, &r);
        assert_eq!(
            format!("{:02x?}", s.as_bytes()),
            "[5b, c9, 8e, 7d, 28, 74, d0, 9d, e8, fb, da, 22, 34, 83, 24, 4f, a1, ba, a2, 6a, 71, 91, 4e, 8e, d4, 8f, 88, b8, 2d, 17, 0b, 4b]"
        );
    }

    #[test]
    fn test_booth_recoded_w7() {
        let s = Scalar::small_u64(0x3bce_3c3e_27d2_604b);
        for (d, sign) in s.booth_recoded_w7() {
            println!("{d:x} {sign:x}");
        }
    }

    #[test]
    fn private_key_in_range() {
        assert_eq!(
            PrivateKey::from_bytes(&[0u8; 32]).unwrap_err(),
            Error::OutOfRange
        );

        // order rejected
        assert_eq!(PrivateKey::from_bytes(b"\xff\xff\xff\xff\x00\x00\x00\x00\xff\xff\xff\xff\xff\xff\xff\xff\xbc\xe6\xfa\xad\xa7\x17\x9e\x84\xf3\xb9\xca\xc2\xfc\x63\x25\x51").unwrap_err(), Error::OutOfRange);

        // order + 1 rejected
        assert_eq!(PrivateKey::from_bytes(b"\xff\xff\xff\xff\x00\x00\x00\x00\xff\xff\xff\xff\xff\xff\xff\xff\xbc\xe6\xfa\xad\xa7\x17\x9e\x84\xf3\xb9\xca\xc2\xfc\x63\x25\x52").unwrap_err(), Error::OutOfRange);

        // order - 1 is ok
        PrivateKey::from_bytes(b"\xff\xff\xff\xff\x00\x00\x00\x00\xff\xff\xff\xff\xff\xff\xff\xff\xbc\xe6\xfa\xad\xa7\x17\x9e\x84\xf3\xb9\xca\xc2\xfc\x63\x25\x50").unwrap();
    }

    #[test]
    fn curve_field_elements_as_mont() {
        const CURVE_A: FieldElement = FieldElement([
            0xffff_ffff_ffff_fffc,
            0x0000_0000_ffff_ffff,
            0x0000_0000_0000_0000,
            0xffff_ffff_0000_0001,
        ]);
        const CURVE_B: FieldElement = FieldElement([
            0x3bce_3c3e_27d2_604b,
            0x651d_06b0_cc53_b0f6,
            0xb3eb_bd55_7698_86bc,
            0x5ac6_35d8_aa3a_93e7,
        ]);

        println!("G.x = {:x?}", CURVE_GENERATOR.x().as_mont());
        println!("G.y = {:x?}", CURVE_GENERATOR.y().as_mont());
        println!("a = {:x?}", CURVE_A.as_mont());
        println!("b = {:x?}", CURVE_B.as_mont());
        let one = FieldElement([1, 0, 0, 0]);
        println!("R = {:x?}", one.as_mont());
        println!("R * R = {:x?}", one.as_mont().as_mont());

        println!("montify n = {:016x?}", Scalar::montifier().0);
    }

    #[test]
    fn base_point_precomp_w7() {
        let precomp = CURVE_GENERATOR.public_precomp_w7_slow();

        println!("pub(super) static CURVE_GENERATOR_PRECOMP_W7: super::AffineMontPointTableW7 = [");
        for (power, row) in precomp.iter().enumerate() {
            println!("    // 1G..64G << {}", power * 7);
            println!("    [");
            for column in row {
                for w in column.xy {
                    println!("            0x{w:016x}, ");
                }
            }
            println!("    ],");
        }
        println!("];");

        println!();
        println!("table size is {} bytes", size_of_val(&precomp));
    }
}
