// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use super::rsa_pub::{MAX_PUBLIC_MODULUS_BYTES, RsaPublicKey};
use crate::error::Error;
use crate::low;
use crate::mid::rng::RandomSource;

mod generate;
pub use generate::KeySize;

pub(crate) struct RsaPrivateKey {
    public: RsaPublicKey,

    p: SecretRsaPosIntModP,
    q: SecretRsaPosIntModP,
    d: SecretRsaPosIntD,
    dp: SecretRsaPosIntModP,
    dq: SecretRsaPosIntModP,
    iqmp: SecretRsaPosIntModP,

    iqmp_mont: SecretRsaPosIntModP,
    p_montifier: SecretRsaPosIntModP,
    q_montifier: SecretRsaPosIntModP,
    p0: u64,
    q0: u64,
}

impl RsaPrivateKey {
    pub(crate) fn new(
        p: RsaPosIntModP,
        q: RsaPosIntModP,
        d: RsaPosIntD,
        dp: RsaPosIntModP,
        dq: RsaPosIntModP,
        iqmp: RsaPosIntModP,
        n: RsaPosIntModN,
        e: u32,
    ) -> Result<Self, Error> {
        let p_len = p.len_bytes();
        if p.is_even()
            || q.is_even()
            || !(MIN_PRIVATE_MODULUS_BYTES..=MAX_PRIVATE_MODULUS_BYTES).contains(&p_len)
        {
            return Err(Error::OutOfRange);
        }

        let public = RsaPublicKey::new(n, e)?;
        let p_montifier: SecretRsaPosIntModP = p.montifier().into();
        let q_montifier = q.montifier().into();
        let iqmp_mont = iqmp.to_montgomery(&p_montifier, &p).into();
        let p0 = p.mont_neg_inverse();
        let q0 = q.mont_neg_inverse();

        let p = p.into();
        let q = q.into();
        let d = d.into();
        let dp = dp.into();
        let dq = dq.into();
        let iqmp = iqmp.into();

        Ok(Self {
            public,
            p,
            q,
            d,
            dp,
            dq,
            iqmp,
            iqmp_mont,
            p_montifier,
            q_montifier,
            p0,
            q0,
        })
    }

    pub(crate) fn generate(
        size: KeySize,
        candidate_source: &mut dyn RandomSource,
        witness_source: &mut dyn RandomSource,
    ) -> Result<Self, Error> {
        generate::generate_key(size, candidate_source, witness_source)
    }

    pub(crate) fn public_key(&self) -> RsaPublicKey {
        self.public.clone()
    }

    pub(crate) fn modulus_len_bytes(&self) -> usize {
        self.public.modulus_len_bytes()
    }

    pub(crate) fn encode_components<'a>(
        &self,
        buffer: &'a mut RsaComponentsBuffer,
    ) -> Result<RsaComponents<'a>, Error> {
        let (public_modulus, buffer) = buffer.0.split_at_mut(MAX_PUBLIC_MODULUS_BYTES + 1);
        let (public_exponent, buffer) = buffer.split_at_mut(4);
        let (p, buffer) = buffer.split_at_mut(MAX_PRIVATE_MODULUS_BYTES + 1);
        let (q, buffer) = buffer.split_at_mut(MAX_PRIVATE_MODULUS_BYTES + 1);
        let (d, buffer) = buffer.split_at_mut(MAX_PUBLIC_MODULUS_BYTES + 1);
        let (dp, buffer) = buffer.split_at_mut(MAX_PRIVATE_MODULUS_BYTES + 1);
        let (dq, buffer) = buffer.split_at_mut(MAX_PRIVATE_MODULUS_BYTES + 1);
        let (iqmp, _) = buffer.split_at_mut(MAX_PRIVATE_MODULUS_BYTES + 1);

        let public_modulus = self.public.n.to_bytes_asn1(public_modulus)?;
        public_exponent.copy_from_slice(&self.public.e.to_be_bytes());

        let p = self.p.to_bytes_asn1(p)?;
        let q = self.q.to_bytes_asn1(q)?;
        let d = self.d.to_bytes_asn1(d)?;
        let dp = self.dp.to_bytes_asn1(dp)?;
        let dq = self.dq.to_bytes_asn1(dq)?;
        let iqmp = self.iqmp.to_bytes_asn1(iqmp)?;

        // We are exporting this key; it is leaving the confines of this
        // library and becoming public data.
        low::ct::public_slice(p);
        low::ct::public_slice(q);
        low::ct::public_slice(d);
        low::ct::public_slice(dp);
        low::ct::public_slice(dq);
        low::ct::public_slice(iqmp);

        Ok(RsaComponents {
            public_modulus,
            public_exponent,
            p,
            q,
            d,
            dp,
            dq,
            iqmp,
        })
    }

    /// returns c ^ d mod n
    ///
    /// (albeit via CRT)
    pub(crate) fn private_op(&self, c: &RsaPosIntModN) -> Result<RsaPosIntModN, Error> {
        if !c.less_than(&self.public.n) {
            return Err(Error::OutOfRange);
        }

        // A note about blinding:
        //
        // In this library, we only perform RSA signatures, where `c` is
        // public information, there is little use for base blinding.
        //
        // Exponent and modulus blinding are _also_ relatively unnecessary,
        // since our `PosInt::mod_exp` is side-channel silent.
        // See the commentary there for why I think that is the case.

        // i.   Let m_1 = c^dP mod p and m_2 = c^dQ mod q.
        // (do reductions of c first, so the mod exp can be done at
        // width of p or q rather than pq.)
        let cmp = c.reduce(&self.p, &self.p_montifier);
        let m_1 = cmp.mod_exp(&self.dp, &self.p, &self.p_montifier, self.p0);
        let cmq = c.reduce(&self.q, &self.q_montifier);
        let m_2 = cmq.mod_exp(&self.dq, &self.q, &self.q_montifier, self.q0);

        // ii. If u > 2, let m_i = c^(d_i) mod r_i, i = 3, ..., u.
        // (we don't support multiprime rsa)

        // iii. Let h = (m_1 - m_2) * qInv mod p.
        let h = m_1
            .sub_mod(&m_2, &self.p)
            .mont_mul(&self.iqmp_mont, &self.p, self.p0);

        // iv.  Let m = m_2 + q * h.
        let m = m_2.widen().add(&low::PosInt::mul(&self.q, &h));
        let m = low::ct::into_public(m);

        // validate the result as a fault attack countermeasure,
        // at the same time it validates our working above, and
        // the key halves against each other
        let c2 = self.public.public_op(m.clone())?;
        if c2.equals(c) {
            Ok(m)
        } else {
            Err(Error::DecryptFailed)
        }
    }
}

impl Drop for RsaPrivateKey {
    fn drop(&mut self) {
        low::zeroise_value(&mut self.p0);
        low::zeroise_value(&mut self.q0);
    }
}

pub(crate) struct RsaComponents<'a> {
    pub(crate) public_modulus: &'a [u8],
    pub(crate) public_exponent: &'a [u8],
    pub(crate) p: &'a [u8],
    pub(crate) q: &'a [u8],
    pub(crate) d: &'a [u8],
    pub(crate) dp: &'a [u8],
    pub(crate) dq: &'a [u8],
    pub(crate) iqmp: &'a [u8],
}

pub(crate) struct RsaComponentsBuffer([u8; Self::LEN]);

impl RsaComponentsBuffer {
    pub(crate) const LEN: usize =
        // public modulus and private exponent
        (MAX_PUBLIC_MODULUS_BYTES + 1) * 2 +
            // public exponent
            4 +
            // private moduli and crt components
            (MAX_PRIVATE_MODULUS_BYTES + 1) * 5;
}

impl Drop for RsaComponentsBuffer {
    fn drop(&mut self) {
        low::zeroise(&mut self.0);
    }
}

impl Default for RsaComponentsBuffer {
    fn default() -> Self {
        Self([0u8; 4619])
    }
}

const MAX_PRIVATE_MODULUS_BITS: usize = 4096;
const MAX_PRIVATE_MODULUS_WORDS: usize = MAX_PRIVATE_MODULUS_BITS / 64;
pub(crate) const MAX_PRIVATE_MODULUS_BYTES: usize = MAX_PRIVATE_MODULUS_BITS / 8;

const MIN_PRIVATE_MODULUS_BITS: usize = 1024;
const MIN_PRIVATE_MODULUS_BYTES: usize = MIN_PRIVATE_MODULUS_BITS / 8;

type SecretRsaPosIntModP = low::SecretPosInt<MAX_PRIVATE_MODULUS_WORDS>;
type SecretRsaPosIntD = low::SecretPosInt<{ MAX_PRIVATE_MODULUS_WORDS * 2 }>;
type RsaPosIntModP = low::PosInt<MAX_PRIVATE_MODULUS_WORDS>;
type RsaPosIntD = low::PosInt<{ MAX_PRIVATE_MODULUS_WORDS * 2 }>;
type RsaPosIntModN = low::PosInt<{ MAX_PRIVATE_MODULUS_WORDS * 2 }>;
