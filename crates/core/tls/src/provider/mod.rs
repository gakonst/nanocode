mod aead;
mod hash;
mod kx;
mod sign;
mod suites;
mod verify;

use rustls::crypto::{CryptoProvider, GetRandomFailed, SecureRandom};

#[must_use]
pub fn provider() -> CryptoProvider {
    CryptoProvider {
        cipher_suites: suites::ALL.to_vec(),
        kx_groups: kx::ALL.to_vec(),
        signature_verification_algorithms: verify::ALGORITHMS,
        secure_random: &Random,
        key_provider: &sign::P256Keys,
    }
}

#[derive(Debug)]
struct Random;

impl SecureRandom for Random {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), GetRandomFailed> {
        graviola::random::fill(bytes).map_err(|_| GetRandomFailed)
    }
}
