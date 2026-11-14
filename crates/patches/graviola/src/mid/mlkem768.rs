// Written for Graviola by Joe Birr-Pixton, 2026.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! ML-KEM-768 as standardised by FIPS-203.
//!
//! ```
//! # fn main() -> Result<(), graviola::Error> {
//! use graviola::key_agreement::mlkem768::{DecapKey, EncapKey};
//!
//! // The recipient generates a key pair, and sends the encoded
//! // encapsulation key to the sender.
//! let decap_key = DecapKey::generate()?;
//! let encap_key_bytes = decap_key.encapsulation_key().as_bytes();
//!
//! // The sender decodes the encapsulation key, encapsulates a fresh
//! // shared secret, and sends the ciphertext back to the recipient.
//! let encap_key = EncapKey::from_bytes(&encap_key_bytes)?;
//! let (sender_secret, ciphertext) = encap_key.encaps()?;
//!
//! // The recipient decapsulates the ciphertext, recovering the same
//! // shared secret.
//! let recipient_secret = decap_key.decaps(&ciphertext);
//!
//! assert_eq!(sender_secret.as_ref(), recipient_secret.as_ref());
//! # Ok(())
//! # }
//! ```

use core::fmt;
use core::marker::PhantomData;

use crate::{Error, low};
use crate::{
    low::{ct_copy, ct_equal, ct_select_i16},
    mid::{
        rng::{RandomSource, SystemRandom},
        sha3,
    },
};

/// An ML-KEM-768 decapsulation key.
pub struct DecapKey {
    ek_pke: EncapKey,
    dk_pke: [u8; K * 384],
    h_ek: [u8; 32],
    z: [u8; 32],
}

impl DecapKey {
    /// Generate a random [`DecapKey`] (which contains the corresponding [`EncapKey`]).
    ///
    /// This fails only if random material generation fails.
    pub fn generate() -> Result<Self, Error> {
        let _entry = low::Entry::new_secret();
        let mut seed = [0u8; 64];
        SystemRandom.fill(&mut seed)?;
        Ok(Self::keygen_internal(&seed))
    }

    /// Decapsulate ciphertext `c`, yielding a [`SharedSecret`].
    pub fn decaps(self, c: &Ciphertext) -> SharedSecret {
        let _entry = low::Entry::new_secret();
        self.decaps_internal(c)
    }

    /// Return the corresponding [`EncapKey`].
    pub fn encapsulation_key(&self) -> EncapKey {
        let _entry = low::Entry::new_public();
        self.ek_pke.clone()
    }

    /// Encode the key as bytes.
    ///
    /// This is used for testing.
    #[doc(hidden)]
    pub fn as_bytes(&self) -> [u8; K * 768 + 96] {
        let _entry = low::Entry::new_secret();
        let mut out = [0u8; K * 768 + 96];
        out[..K * 384].copy_from_slice(&self.dk_pke);
        out[K * 384..K * 768].copy_from_slice(&self.ek_pke.t_hat);
        out[K * 768..K * 768 + 32].copy_from_slice(&self.ek_pke.rho);
        out[K * 768 + 32..K * 768 + 64].copy_from_slice(&self.h_ek);
        out[K * 768 + 64..].copy_from_slice(&self.z);
        out
    }

    #[doc(hidden)]
    pub fn keygen_internal(seed: &[u8; 64]) -> Self {
        // Preliminaries: split seed into d and z.
        let (d, z) = seed.split_at(32);
        let d = d.try_into().unwrap();
        let z = z.try_into().unwrap();

        // 1. (ekPKE, dkPKE) <- K-PKE.KeyGen(d)
        // 2. ek <- ekPKE
        let (ek_pke, dk_pke) = Self::kpke_keygen(&d);

        // 3. dk <- (dkPKE || ek || H(ek) || z)
        // 4. return (ek, dk)
        let mut h = sha3::Sha3_256Context::new();
        h.update(&ek_pke.t_hat);
        h.update(&ek_pke.rho);
        let h_ek = h.finish();

        Self {
            dk_pke,
            ek_pke,
            h_ek,
            z,
        }
    }

    #[doc(hidden)]
    pub fn decaps_internal(&self, c: &Ciphertext) -> SharedSecret {
        // Steps 1. - 4. are not relevant for us.
        // 5. m' <- K-PKE.Decrypt(dkPKE, c)
        let m_prime = self.kpke_decrypt(c);

        // 6. (K', r') <- G(m' || h)
        let mut g = sha3::Sha3_512Context::new();
        g.update(&m_prime.0);
        g.update(&self.h_ek);
        let kr_prime = g.finish();
        let (k_prime, r_prime) = kr_prime.split_at(32);
        let r_prime = KpkeRandomness(r_prime.try_into().unwrap());

        // 7. K‾ <- J(z || c)
        let mut k_bar = SharedSecret([0u8; 32]);
        sha3::Shake256::new(&[&self.z, &c.0]).read(&mut k_bar.0);

        // 8. c' <- K-PKE.Encrypt(ekPKE, m', r')
        let c_prime = self.ek_pke.kpke_encrypt(m_prime, r_prime);

        // 9. - 11. if c != c' then K' <- K‾
        // 12. return K'
        let mut k_prime = k_prime.try_into().unwrap();
        ct_copy(ct_equal(&c.0, &c_prime.0), &mut k_prime, &k_bar.0);
        SharedSecret(k_prime)
    }

    /// This is Algorithm 13: K-PKE.KeyGen(d) -> (ek_PKE, dk_PKE)
    fn kpke_keygen(d: &[u8; 32]) -> (EncapKey, [u8; K * 384]) {
        // 1. (𝜌, 𝜎) <- G(d || k)
        let mut g = sha3::Sha3_512Context::new();
        g.update(d);
        g.update(&[K_BYTE]);
        let g = g.finish();
        let (rho, sigma) = g.split_at(32);
        let rho: [u8; 32] = rho.try_into().unwrap();
        let sigma = sigma.try_into().unwrap();

        // 3. - 7.
        let a_hat = Coeffs::sample_poly(&rho);

        // 8. - 15.
        let (s, e) = Coeffs::sample_poly_cbd_dual(sigma);

        // 16. ŝ <- NTT(s)
        let mut s_hat = s.ntt();

        // 17. ê <- NTT(e)
        let e_hat = e.ntt();

        // 18. t^ = A^ ∘ ŝ + ê
        let t_hat = a_hat.mul_add(&s_hat, &e_hat);

        let encap = EncapKey {
            t_hat: t_hat.to_bytes(),
            rho,
            transpose_a_hat: a_hat.transpose(),
        };

        s_hat.reduce_in_place();
        let decap = s_hat.to_bytes();

        (encap, decap)
    }

    /// This is Algorithm 15: K-PKE.Decrypt(dk_PKE, c)
    fn kpke_decrypt(&self, c: &Ciphertext) -> Message {
        // 1. c_1 <- c[0 ∶ 32 * du * k]
        // 2. c_2 <- c[32 * du * k ∶ 32 * (du * k + dv)]
        let (c_1, c_2) = c.0.split_at(32 * DU * K);

        // 3. u' <- Decompress_du(ByteDecode_du(c1))
        let u_prime = Coeffs::decompress_from_bytes_du_10(c_1.try_into().unwrap());
        // 4. v' <- Decompress_dv(ByteDecode_dv(c2))
        let v_prime = Coeffs::decompress_from_bytes_dv_4(c_2.try_into().unwrap());

        // 5. s^ <- ByteDecode_12(dk_PKE)
        let s_hat = Coeffs::from_bytes(&self.dk_pke);

        // 6. w <- v' - NTT-1(transpose(s^) * NTT(u'))
        let w = v_prime.sub(&s_hat.mul_and_inverse_ntt(&u_prime.ntt()));

        // 7. m <- ByteEncode_1(Compress_1(w))
        w.to_message_bits()
    }
}

impl Drop for DecapKey {
    fn drop(&mut self) {
        low::zeroise(&mut self.dk_pke);
        low::zeroise(&mut self.z);
    }
}

/// An ML-KEM-768 encapsulation key.
#[derive(Clone)]
pub struct EncapKey {
    t_hat: [u8; K * 384],
    rho: [u8; 32],
    transpose_a_hat: Coeffs<{ K * K * N }, Ntt>,
}

impl EncapKey {
    /// Create a new [`EncapKey`] from bytes.
    pub fn from_bytes(input: &[u8; K * 384 + 32]) -> Result<Self, Error> {
        let _entry = low::Entry::new_public();
        let (t_hat, rho) = input.split_at(K * 384);
        let t_hat = t_hat.try_into().unwrap();
        let rho = rho.try_into().unwrap();

        // 2. (Modulus check) Perform the computation
        //
        //   test <- ByteEncode_12(ByteDecode_12(ek[0 ∶ 384k]))
        //
        // If test != ek[0 ∶ 384k], then input checking failed.
        if t_hat != Coeffs::from_bytes(&t_hat).to_bytes() {
            return Err(Error::OutOfRange);
        }

        let transpose_a_hat = Coeffs::sample_poly_transposed(&rho);

        Ok(Self {
            t_hat,
            rho,
            transpose_a_hat,
        })
    }

    /// Encapsulate a random shared secret, returning the shared secret and a ciphertext.
    ///
    /// The ciphertext can be returned to the holder of the [`DecapKey`] who can then
    /// derive the same [`SharedSecret`].
    pub fn encaps(self) -> Result<(SharedSecret, Ciphertext), Error> {
        let _entry = low::Entry::new_secret();
        let mut m = Message([0; 32]);
        SystemRandom.fill(&mut m.0)?;
        Ok(self.encaps_internal(m))
    }

    /// Encode the [`EncapKey`] as bytes.
    pub fn as_bytes(&self) -> [u8; K * 384 + 32] {
        let _entry = low::Entry::new_public();
        let mut out = [0u8; K * 384 + 32];
        out[..K * 384].copy_from_slice(&self.t_hat);
        out[K * 384..].copy_from_slice(&self.rho);
        out
    }

    #[doc(hidden)]
    pub fn encaps_internal(self, m: Message) -> (SharedSecret, Ciphertext) {
        // 1. (K, r) <- G(m || H(ek))
        let mut h = sha3::Sha3_256Context::new();
        h.update(&self.t_hat);
        h.update(&self.rho);
        let h_ek = h.finish();

        let mut g = sha3::Sha3_512Context::new();
        g.update(&m.0);
        g.update(&h_ek);
        let kr = g.finish();
        let (k, r) = kr.split_at(32);
        let k = k.try_into().unwrap();
        let r = KpkeRandomness(r.try_into().unwrap());

        // 2. c <- K-PKE.Encrypt(ek, m, r)
        let c = self.kpke_encrypt(m, r);
        (SharedSecret(k), c)
    }

    fn kpke_encrypt(&self, m: Message, r: KpkeRandomness) -> Ciphertext {
        // 2. t^ <- ByteDecode12(ekPKE[0 ∶ 384k])
        let t_hat = Coeffs::from_bytes(&self.t_hat);

        // 3. 𝜌 <- ekPKE[384k ∶ 384k + 32]
        //  - only required for a_hat expansion
        // 4. - 8. rolled into self.transpose_a_hat.

        // 9. - 12.
        // 13. - 16.
        let (y, e_1) = Coeffs::sample_poly_cbd_dual(&r.0);

        // 17. e_2 <- SamplePolyCBD(PRF (r, N))
        let mut buf = [0u8; 128];
        sha3::Shake256::new(&[&r.0, &[K_BYTE * 2]]).read(&mut buf);
        let mut e_2 = Coeffs::zero();
        sample_cbd2(&buf, &mut e_2.0);

        // 18. y^ <- NTT(y)
        let y_hat = y.ntt();

        // 19. u <- NTT^-1(transpose(A^) ∘ y^) + e_1
        let u = self.transpose_a_hat.mul_and_inverse_ntt(&y_hat).add(&e_1);

        // 20. µ <- Decompress_1(ByteDecode_1(m))
        let mu = Coeffs::from_message_bits(m);

        // 21. v <- NTT^-1(transpose(t^) ∘ y^) + e_2 + µ
        let v = t_hat.mul_and_inverse_ntt(&y_hat).add(&e_2).add(&mu);

        let mut c = Ciphertext([0; _]);
        let (c_1, c_2) = c.0.split_at_mut(32 * DU * K);

        // 22. c_1 <- ByteEncode_du(Compress_du(u))
        u.compress_into_bytes_du_10(c_1.try_into().unwrap());
        // 23. c_2 <- ByteEncode_dv(Compress_dv(v))
        v.compress_into_bytes_dv_4(c_2.try_into().unwrap());

        // 24. return c <- (c_1 || c_2)
        c
    }
}

/// An ML-KEM-768 ciphertext.
#[derive(Debug, Clone)]
pub struct Ciphertext([u8; 32 * (DU * K + DV)]);

impl From<[u8; 1088]> for Ciphertext {
    fn from(value: [u8; 1088]) -> Self {
        Self(value)
    }
}

impl AsRef<[u8; 1088]> for Ciphertext {
    fn as_ref(&self) -> &[u8; 1088] {
        &self.0
    }
}

/// A K-PKE message.
#[doc(hidden)]
pub struct Message(pub [u8; 32]);

impl Drop for Message {
    fn drop(&mut self) {
        low::zeroise(&mut self.0);
    }
}

/// K-PKE randomness input.
struct KpkeRandomness([u8; 32]);

/// An ML-KEM shared secret.
pub struct SharedSecret([u8; 32]);

impl AsRef<[u8; 32]> for SharedSecret {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for SharedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SharedSecret").finish_non_exhaustive()
    }
}

impl Drop for SharedSecret {
    fn drop(&mut self) {
        low::zeroise(&mut self.0);
    }
}

/// Tracks what the values in a `Coeffs` mean.
trait Domain: Clone {}

/// Coefficients in normal domain.
#[derive(Clone)]
struct Normal;
impl Domain for Normal {}

/// Coefficients in NTT domain.
#[derive(Clone)]
struct Ntt;
impl Domain for Ntt {}

/// Multiplication cache values.
#[derive(Clone)]
struct MulCache;
impl Domain for MulCache {}

#[derive(Clone, Debug)]
#[repr(align(32))]
struct Coeffs<const C: usize, D: Domain>([i16; C], PhantomData<D>);

impl<const C: usize, D: Domain> Coeffs<C, D> {
    const fn zero() -> Self {
        Self([0; C], PhantomData)
    }

    /// Reduce coefficients into [0, Q)
    fn reduce_in_place(&mut self) {
        for k in self.0.as_chunks_mut().0 {
            low::mlkem_reduce(k);
        }
    }
}

impl Coeffs<{ K * K * N }, Ntt> {
    fn sample_poly(rho: &[u8; 32]) -> Self {
        Self::_sample_poly_ntt::<false>(rho)
    }

    fn sample_poly_transposed(rho: &[u8; 32]) -> Self {
        Self::_sample_poly_ntt::<true>(rho)
    }

    fn _sample_poly_ntt<const TRANSPOSED: bool>(rho: &[u8; 32]) -> Self {
        let mut r = Coeffs::zero();

        // We have K * K polynomials to generate.  In this case, K := 3, so 9.
        // We have a by-4 keccak, so we can attack the problem in two
        // sets of four, followed by one straggler.

        let mut work_iter = SAMPLE_POLY_WORK.chunks_exact(4);

        for c4 in work_iter.by_ref() {
            let inputs = match TRANSPOSED {
                false => &[
                    &[c4[0].1, c4[0].0],
                    &[c4[1].1, c4[1].0],
                    &[c4[2].1, c4[2].0],
                    &[c4[3].1, c4[3].0],
                ],
                true => &[
                    &[c4[0].0, c4[0].1],
                    &[c4[1].0, c4[1].1],
                    &[c4[2].0, c4[2].1],
                    &[c4[3].0, c4[3].1],
                ],
            };

            Self::_sample_poly_ntt_quad(
                rho,
                inputs,
                (&mut r.0[c4[0].2..c4[3].2 + N]).try_into().unwrap(),
            );
        }

        for (i, j, offs) in work_iter.remainder() {
            let input = match TRANSPOSED {
                false => &[*j, *i],
                true => &[*i, *j],
            };
            Shake128ForMlKem::new(&[rho, input])
                .sample_into((&mut r.0[*offs..*offs + N]).try_into().unwrap());
        }

        r.reorder();

        r
    }

    fn _sample_poly_ntt_quad(rho: &[u8; 32], inputs: &[&[u8; 2]; 4], outputs: &mut [i16; N * 4]) {
        let mut buf = [0; 40];
        buf[..32].copy_from_slice(rho);
        buf[34] = sha3::SHAKE_PAD_BYTE;

        let mut buf0 = buf;
        buf0[32..34].clone_from_slice(inputs[0]);
        let mut buf1 = buf;
        buf1[32..34].clone_from_slice(inputs[1]);
        let mut buf2 = buf;
        buf2[32..34].clone_from_slice(inputs[2]);
        let mut buf3 = buf;
        buf3[32..34].clone_from_slice(inputs[3]);

        let sponge_4x = sha3::SqueezingSponge4xShake128::new(&[&buf0, &buf1, &buf2, &buf3]);
        let (output0, outputs) = outputs.split_at_mut(N);
        let (output1, outputs) = outputs.split_at_mut(N);
        let (output2, output3) = outputs.split_at_mut(N);

        let mut samples = [[0; sha3::SHAKE_128_R_BYTES * 3]; 4];
        let [tsponge0, tsponge1, tsponge2, tsponge3] = sponge_4x.squeeze(&mut samples);

        let tail0 = Shake128ForMlKem::sample(&samples[0], output0.try_into().unwrap());
        let tail1 = Shake128ForMlKem::sample(&samples[1], output1.try_into().unwrap());
        let tail2 = Shake128ForMlKem::sample(&samples[2], output2.try_into().unwrap());
        let tail3 = Shake128ForMlKem::sample(&samples[3], output3.try_into().unwrap());

        if !tail0.is_empty() {
            Shake128ForMlKem {
                sponge: tsponge0.restitute(),
            }
            .tail_case(tail0);
        }

        if !tail1.is_empty() {
            Shake128ForMlKem {
                sponge: tsponge1.restitute(),
            }
            .tail_case(tail1);
        }

        if !tail2.is_empty() {
            Shake128ForMlKem {
                sponge: tsponge2.restitute(),
            }
            .tail_case(tail2);
        }

        if !tail3.is_empty() {
            Shake128ForMlKem {
                sponge: tsponge3.restitute(),
            }
            .tail_case(tail3);
        }
    }

    // a * s + e
    fn mul_add(
        &self,
        s: &Coeffs<{ K * N }, Ntt>,
        e: &Coeffs<{ K * N }, Ntt>,
    ) -> Coeffs<{ K * N }, Ntt> {
        let mut r = e.clone();

        let s_precomp = s.mul_cache();

        let mut term = Coeffs::<_, Ntt>::zero();

        for (aa, rr) in self
            .0
            .as_chunks()
            .0
            .iter()
            .zip(r.0.as_chunks_mut::<N>().0.iter_mut())
        {
            low::mlkem_basemul_k3(&mut term.0, aa, &s.0, &s_precomp.0);
            low::mlkem_tomont(&mut term.0);

            for (r, t) in rr.iter_mut().zip(term.0) {
                *r += t;
            }
        }

        r.reduce_in_place();
        r
    }

    fn reorder(&mut self) {
        for ch in self.0.as_chunks_mut().0 {
            low::mlkem_unpack(ch);
        }
    }

    fn transpose(mut self) -> Self {
        let (chunks, _) = self.0.as_chunks_mut::<N>();

        for i in 0..K {
            for j in (i + 1)..K {
                chunks.swap(i * K + j, j * K + i);
            }
        }

        self
    }

    fn mul_and_inverse_ntt(&self, y: &Coeffs<{ K * N }, Ntt>) -> Coeffs<{ K * N }, Normal> {
        let mut r = Coeffs::<_, Ntt>::zero();

        let y_precomp = y.mul_cache();

        let mut term = Coeffs::<_, Ntt>::zero();

        for (aa, rr) in self
            .0
            .as_chunks()
            .0
            .iter()
            .zip(r.0.as_chunks_mut::<N>().0.iter_mut())
        {
            low::mlkem_basemul_k3(&mut term.0, aa, &y.0, &y_precomp.0);

            for (r, t) in rr.iter_mut().zip(term.0) {
                *r += t;
            }
        }

        r.reduce_in_place();
        r.inverse_ntt()
    }
}

/// Values for i and j (pre-transpose) plus start offset of N coefficients within
/// a K * K * N matrix.
const SAMPLE_POLY_WORK: &[(u8, u8, usize)] = &[
    (0, 0, 0), //
    (0, 1, N),
    (0, 2, N * 2),
    (1, 0, K * N),
    (1, 1, N + K * N),
    (1, 2, N * 2 + K * N),
    (2, 0, K * N * 2),
    (2, 1, N + K * N * 2),
    (2, 2, N * 2 + K * N * 2),
];

impl Coeffs<{ K * N }, Ntt> {
    /// This is `ByteDecode_12(bytes)`.
    fn from_bytes(bytes: &[u8; 1152]) -> Self {
        let mut r = Coeffs::zero();

        let (r_chunks, _) = r.0.as_chunks_mut();
        let (b_chunks, _) = bytes.as_chunks();

        for (rr, bb) in r_chunks.iter_mut().zip(b_chunks.iter()) {
            low::mlkem_frombytes(rr, bb);
            low::mlkem_reduce(rr);
        }

        r
    }

    /// This is `ByteEncode_12(self)`
    fn to_bytes(&self) -> [u8; 1152] {
        let mut r = [0u8; 1152];

        let (c_chunks, _) = self.0.as_chunks();
        let (r_chunks, _) = r.as_chunks_mut();

        for (rr, cc) in r_chunks.iter_mut().zip(c_chunks.iter()) {
            low::mlkem_tobytes(rr, cc);
        }

        r
    }

    /// This is `NTT-1(self * s)`
    fn mul_and_inverse_ntt(&self, s: &Coeffs<{ K * N }, Ntt>) -> Coeffs<{ N }, Normal> {
        let mut r = Coeffs::zero();

        let s_precomp = s.mul_cache();

        low::mlkem_basemul_k3(&mut r.0, &self.0, &s.0, &s_precomp.0);
        // nb. no need for a tomont here, as `inverse_ntt` incorporates that

        r.inverse_ntt()
    }

    fn mul_cache(&self) -> Coeffs<{ K * 128 }, MulCache> {
        let mut r = Coeffs::zero();

        let (m, _) = self.0.as_chunks();
        let (rr, _) = r.0.as_chunks_mut();

        low::mlkem_mulcache_compute(&mut rr[0], &m[0]);
        low::mlkem_mulcache_compute(&mut rr[1], &m[1]);
        low::mlkem_mulcache_compute(&mut rr[2], &m[2]);

        r
    }
}

impl<const C: usize> Coeffs<C, Ntt> {
    fn inverse_ntt(mut self) -> Coeffs<C, Normal> {
        for ch in self.0.as_chunks_mut().0 {
            low::mlkem_intt(ch);
        }
        Coeffs(self.0, PhantomData)
    }
}

/// `Compress_4(x) = round((2^4 / q) * x) mod 2^4`, for x in [0, Q)
///
/// Division-free so no variable-time division instruction is emitted
/// (cf. KyberSlash); the constant is exact over the whole input range.
fn compress_4(x: i16) -> u8 {
    debug_assert!((0..Q).contains(&x));
    let c = x as u64;
    let c = c << 4;
    let c = c + Q_HALF as u64;
    let c = c * 80635;
    let c = c >> 28;
    let c = c & 0xf;
    c as u8
}

/// `Decompress_4(y) = round((q / 2^4) * y)`, for y in [0, 2^4)
fn decompress_4(y: u16) -> i16 {
    debug_assert!(y < 16);
    let d = y as u32;
    let d = d * (Q as u32);
    let d = d + 8;
    let d = d >> 4;
    d as i16
}

/// `Compress_10(x) = round((2^10 / q) * x) mod 2^10`, for x in [0, Q)
fn compress_10(x: i16) -> u16 {
    debug_assert!((0..Q).contains(&x));
    let c = x as u64;
    let c = c << 10;
    let c = c + Q_HALF as u64;
    let c = c * 1290167;
    let c = c >> 32;
    let c = c & 0x3ff;
    c as u16
}

/// `Decompress_10(y) = round((q / 2^10) * y)`, for y in [0, 2^10)
fn decompress_10(y: u16) -> i16 {
    debug_assert!(y < 1024);
    let d = y as u32;
    let d = d * (Q as u32);
    let d = d + 512;
    let d = d >> 10;
    d as i16
}

/// `Compress_1(x)` for eight coefficients in `[0, Q)`
fn compress_1_x8(coeffs: &[i16; 8]) -> u8 {
    let mut r = 0;

    for (i, x) in coeffs.iter().enumerate() {
        debug_assert!((0..Q).contains(x));
        let x = (*x as u64) << 1;
        let x = x + Q_HALF as u64;
        let x = x * 80635;
        let x = x >> 28;
        r |= ((x & 1) as u8) << i;
    }

    r
}

impl Coeffs<{ K * N }, Normal> {
    /// This is dual, K-wise `SamplePolyCBD`.
    ///
    /// In other words, it produces two noise vectors, diversifying the seed `sigma`
    /// with indices from 0..2K.
    fn sample_poly_cbd_dual(sigma: &[u8; 32]) -> (Self, Self) {
        // We need 2 * K samples.
        let mut samples = [[0; 128]; 6];

        // Prepare 6 SHAKE256 inputs.
        let mut buf = [0; 40];
        buf[..32].copy_from_slice(sigma);
        buf[33] = sha3::SHAKE_PAD_BYTE;

        let mut buf0 = buf;
        let mut buf1 = buf;
        let mut buf2 = buf;
        let mut buf3 = buf;
        let mut buf4 = buf;
        let mut buf5 = buf;
        buf0[32] = 0;
        buf1[32] = 1;
        buf2[32] = 2;
        buf3[32] = 3;
        buf4[32] = 4;
        buf5[32] = 5;

        sha3::Shake256::one_shot_sextet(&[&buf0, &buf1, &buf2, &buf3, &buf4, &buf5], &mut samples);

        let mut a = Coeffs::zero();
        let mut b = Coeffs::zero();

        for (sample, poly) in samples.iter().zip(
            a.0.as_chunks_mut()
                .0
                .iter_mut()
                .chain(b.0.as_chunks_mut().0.iter_mut()),
        ) {
            sample_cbd2(sample, poly);
        }

        (a, b)
    }

    /// K-wise NTT
    fn ntt(mut self) -> Coeffs<{ K * N }, Ntt> {
        for ch in self.0.as_chunks_mut().0 {
            low::mlkem_ntt(ch);
        }
        Coeffs(self.0, PhantomData)
    }

    /// This is `ByteEncode_10(Compress_10(u))`
    ///
    /// Each group of four coefficients becomes five bytes,
    /// little-endian bit order per `BitsToBytes`.
    fn compress_into_bytes_du_10(&self, c_1: &mut [u8; 960]) {
        let (quads, _) = self.0.as_chunks::<4>();
        let (out_chunks, _) = c_1.as_chunks_mut::<5>();

        for (quad, out) in quads.iter().zip(out_chunks.iter_mut()) {
            let t0 = compress_10(quad[0]);
            let t1 = compress_10(quad[1]);
            let t2 = compress_10(quad[2]);
            let t3 = compress_10(quad[3]);

            out[0] = t0 as u8;
            out[1] = ((t0 >> 8) | (t1 << 2)) as u8;
            out[2] = ((t1 >> 6) | (t2 << 4)) as u8;
            out[3] = ((t2 >> 4) | (t3 << 6)) as u8;
            out[4] = (t3 >> 2) as u8;
        }
    }

    /// This is `Decompress_10(ByteDecode_10(c_1))`
    ///
    /// The inverse of [`Self::compress_into_bytes_du_10`]: each group of
    /// five bytes yields four ten-bit values, little-endian bit order.
    fn decompress_from_bytes_du_10(c_1: &[u8; 960]) -> Self {
        let mut r = Self::zero();

        let (in_chunks, _) = c_1.as_chunks::<5>();
        let (quads, _) = r.0.as_chunks_mut::<4>();

        for (inp, quad) in in_chunks.iter().zip(quads.iter_mut()) {
            let t0 = (inp[0] as u16) | ((inp[1] as u16 & 0x3) << 8);
            let t1 = ((inp[1] as u16) >> 2) | ((inp[2] as u16 & 0xf) << 6);
            let t2 = ((inp[2] as u16) >> 4) | ((inp[3] as u16 & 0x3f) << 4);
            let t3 = ((inp[3] as u16) >> 6) | ((inp[4] as u16) << 2);

            quad[0] = decompress_10(t0);
            quad[1] = decompress_10(t1);
            quad[2] = decompress_10(t2);
            quad[3] = decompress_10(t3);
        }

        r
    }
}

impl Coeffs<N, Normal> {
    /// This is `Decompress_1(ByteDecode_1(m))`
    ///
    /// `ByteDecode_1` is just `BytesToBits`.
    /// `Decompress_1` maps set bits to Q/2.
    fn from_message_bits(m: Message) -> Self {
        let mut r = Self::zero();

        for (i, byte) in m.0.iter().enumerate() {
            for bit in 0..8 {
                r.0[i * 8 + bit] = ct_select_i16(byte >> bit & 1, Q_HALF, 0);
            }
        }

        r
    }

    /// This is `ByteEncode_1(Compress_1(m))`
    fn to_message_bits(&self) -> Message {
        let mut r = Message([0; 32]);

        for (i, r) in r.0.iter_mut().enumerate() {
            *r = compress_1_x8(&self.0[i * 8..(i + 1) * 8].try_into().unwrap());
        }

        r
    }

    /// This is `ByteEncode_4(Compress_4(v))`
    ///
    /// Each pair of coefficients becomes one byte, low nibble first.
    fn compress_into_bytes_dv_4(&self, c_2: &mut [u8; 128]) {
        let (pairs, _) = self.0.as_chunks::<2>();
        for (pair, byte) in pairs.iter().zip(c_2.iter_mut()) {
            *byte = compress_4(pair[0]) | (compress_4(pair[1]) << 4);
        }
    }

    /// This is `Decompress_4(ByteDecode_4(c_2))`
    ///
    /// The inverse of [`Self::compress_into_bytes_dv_4`]: each byte yields
    /// two four-bit values, low nibble first.
    fn decompress_from_bytes_dv_4(c_2: &[u8; 128]) -> Self {
        let mut r = Self::zero();

        let (pairs, _) = r.0.as_chunks_mut::<2>();
        for (byte, pair) in c_2.iter().zip(pairs.iter_mut()) {
            pair[0] = decompress_4((byte & 0xf) as u16);
            pair[1] = decompress_4((byte >> 4) as u16);
        }

        r
    }
}

impl<const C: usize> Coeffs<C, Normal> {
    /// Simple addition
    fn add(mut self, term: &Self) -> Self {
        for (ss, tt) in self.0.iter_mut().zip(term.0.iter()) {
            *ss += *tt;
        }
        self.reduce_in_place();
        self
    }

    /// Simple subtraction
    fn sub(mut self, term: &Self) -> Self {
        for (ss, tt) in self.0.iter_mut().zip(term.0.iter()) {
            *ss -= *tt;
        }
        self.reduce_in_place();
        self
    }
}

fn sample_cbd2(buf: &[u8; 128], out: &mut [i16; 256]) {
    for (in_bytes, out_coeffs) in buf.chunks_exact(4).zip(out.chunks_exact_mut(8)) {
        let t = u32::from_le_bytes(in_bytes.try_into().unwrap());
        let d = (t & 0x5555_5555) + ((t >> 1) & 0x5555_5555);

        for (j, coeff) in out_coeffs.iter_mut().enumerate() {
            let a = ((d >> (4 * j)) & 0x3) as i16;
            let b = ((d >> (4 * j + 2)) & 0x3) as i16;
            *coeff = a - b;
        }
    }
}

/// SHAKE128, but oriented at use in ML-KEM's `SampleNTT()`
struct Shake128ForMlKem {
    sponge: sha3::Shake128SqueezingSponge,
}

impl Shake128ForMlKem {
    fn new(message: &[&[u8]]) -> Self {
        Self {
            sponge: sha3::Shake128Sponge::new_for_message(message),
        }
    }

    /// Extract 256 coefficients that are < Q by rejection sampling.
    ///
    /// Refer to FIPS-203 `SampleNTT()`.  This function is the inner rejection loop.
    fn sample_into(mut self, output: &mut [i16; 256]) {
        // First, we squeeze three blocks.  Each block contributes up to 112 coefficients,
        // so we get 336 candidate coefficients.
        let mut initial_bytes = [0; sha3::SHAKE_128_R_BYTES * 3];
        self.sponge.squeeze(&mut initial_bytes);

        let tail = Self::sample(&initial_bytes, output);

        // If we were unlucky, 336 candidate coeffients weren't enough.  That
        // happens with low but not negligible probability (1 in ~120).
        if !tail.is_empty() {
            self.tail_case(tail);
        }
    }

    /// Sample into `output` using the bytes `samples`
    ///
    /// Returns the _unwritten_ items in output.  Call `tail_case()` with this value if
    /// non-empty.
    #[must_use]
    fn sample<'a>(
        samples: &'_ [u8; sha3::SHAKE_128_R_BYTES * 3],
        output: &'a mut [i16; 256],
    ) -> &'a mut [i16] {
        let used = low::mlkem_rej_uniform_vartime(output, samples) as usize;
        output.split_at_mut(used).1
    }

    fn tail_case(self, output: &mut [i16]) {
        let tail_iterator = Shake128TwelveBitIterator::new(self.sponge).filter(|f| *f < Q);

        for (out, coeff) in output.iter_mut().zip(tail_iterator) {
            *out = coeff;
        }
    }
}

/// Iterates over 12-bit samples drawn from `sponge`.
struct Shake128TwelveBitIterator {
    sponge: sha3::Shake128SqueezingSponge,
    samples: [i16; Self::SAMPLE_COUNT],
    used: usize,
}

impl Shake128TwelveBitIterator {
    fn new(sponge: sha3::Shake128SqueezingSponge) -> Self {
        Self {
            sponge,
            samples: [0; Self::SAMPLE_COUNT],
            used: Self::SAMPLE_COUNT,
        }
    }

    /// Each three bytes of SHAKE output yields two coefficients.
    const SAMPLE_COUNT: usize = sha3::SHAKE_128_R_BYTES / 3 * 2;
}

impl Iterator for Shake128TwelveBitIterator {
    type Item = i16;

    #[cold]
    fn next(&mut self) -> Option<Self::Item> {
        if self.used == self.samples.len() {
            let mut bytes = [0u8; sha3::SHAKE_128_R_BYTES];
            self.sponge.squeeze(&mut bytes);

            for (buf, d) in bytes.chunks_exact(3).zip(self.samples.chunks_exact_mut(2)) {
                d[0] = (u16::from_le_bytes([buf[0], buf[1]]) & 0xfff) as i16;
                d[1] = (u16::from_le_bytes([buf[1], buf[2]]) >> 4) as i16;
            }
            self.used = 0;
        }

        let item = self.samples[self.used];
        self.used += 1;
        Some(item)
    }
}

const K: usize = 3;
const K_BYTE: u8 = K as u8;
const DU: usize = 10;
const DV: usize = 4;

const Q: i16 = 3329;
const Q_HALF: i16 = (Q + 1) / 2;
const N: usize = 256;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_secret_debug() {
        assert_eq!(format!("{:?}", SharedSecret([0u8; 32])), "SharedSecret(..)");
    }

    #[test]
    fn pairwise() {
        let d = DecapKey::generate().unwrap();
        let (ess, ct) = d.encapsulation_key().encaps().unwrap();
        let dss = d.decaps(&ct);
        assert_eq!(ess.0, dss.0);
    }

    #[test]
    fn encaps_modulus_test() {
        assert_eq!(
            EncapKey::from_bytes(&[0xffu8; 1184]).err(),
            Some(Error::OutOfRange)
        );
    }
}
