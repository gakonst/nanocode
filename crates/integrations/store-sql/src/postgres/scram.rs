use crate::postgres::PgError;
use nanocode_sys::base64::STANDARD;
use nanocode_digest::Sha256;
use nanocode_sync::SwapOption;
use std::sync::Arc;

pub(super) const MECHANISM: &str = "SCRAM-SHA-256";
const NONCE_BYTES: usize = 18;
const GS2_HEADER: &str = "n,,";
const BLOCK_BYTES: usize = 64;
const INNER_PAD: u8 = 0x36;
const OUTER_PAD: u8 = 0x5c;
const FIRST_BLOCK: u32 = 1;
const OUTPUT_BYTES: usize = Sha256::OUTPUT_BYTES;

type Digest = [u8; OUTPUT_BYTES];

#[derive(Clone)]
struct Hmac {
    inner: Sha256,
    outer: Sha256,
}

impl Hmac {
    fn new(key: &[u8]) -> Self {
        let mut block = [0_u8; BLOCK_BYTES];
        if key.len() > BLOCK_BYTES {
            block[..OUTPUT_BYTES].copy_from_slice(&Sha256::digest(key));
        } else {
            block[..key.len()].copy_from_slice(key);
        }
        let mut inner = Sha256::new();
        inner.update(&block.map(|byte| byte ^ INNER_PAD));
        let mut outer = Sha256::new();
        outer.update(&block.map(|byte| byte ^ OUTER_PAD));
        Self { inner, outer }
    }

    fn sign(&self, parts: &[&[u8]]) -> Digest {
        let mut inner = self.inner.clone();
        for part in parts {
            inner.update(part);
        }
        let mut outer = self.outer.clone();
        outer.update(&inner.finalize());
        outer.finalize()
    }
}

fn salted(password: &[u8], salt: &[u8], iterations: u32) -> Digest {
    let keyed = Hmac::new(password);
    let mut block = keyed.sign(&[salt, &FIRST_BLOCK.to_be_bytes()]);
    let mut salted = block;
    for _ in 1..iterations {
        block = keyed.sign(&[&block]);
        for (byte, next) in salted.iter_mut().zip(block) {
            *byte ^= next;
        }
    }
    salted
}

pub(super) struct Salted {
    salt: Vec<u8>,
    iterations: u32,
    digest: Digest,
}

pub(super) struct Scram {
    nonce: String,
    first_bare: String,
}

pub(super) struct Proof {
    pub(super) message: String,
    server_signature: Digest,
}

impl Scram {
    pub(super) fn start() -> Result<(Self, String), PgError> {
        let mut random = [0_u8; NONCE_BYTES];
        nanocode_platform::fill_random(&mut random)?;
        let nonce = STANDARD.encode(random);
        let first_bare = format!("n=,r={nonce}");
        let first = format!("{GS2_HEADER}{first_bare}");
        Ok((Self { nonce, first_bare }, first))
    }

    pub(super) fn prove(
        &self,
        password: &str,
        server_first: &[u8],
        reused: &SwapOption<Salted>,
    ) -> Result<Proof, PgError> {
        let refused =
            |what: &str| PgError::Authentication(format!("the server's SCRAM challenge {what}"));
        let server_first =
            std::str::from_utf8(server_first).map_err(|_| refused("is not UTF-8"))?;
        let mut nonce = None;
        let mut salt = None;
        let mut iterations = None;
        for attribute in server_first.split(',') {
            match attribute.split_at_checked(2) {
                Some(("r=", value)) => nonce = Some(value),
                Some(("s=", value)) => {
                    salt = Some(
                        STANDARD
                            .decode(value)
                            .map_err(|_| refused("has a salt that is not base64"))?,
                    )
                }
                Some(("i=", value)) => {
                    iterations = Some(
                        value
                            .parse::<u32>()
                            .map_err(|_| refused("has an iteration count that is not a number"))?,
                    )
                }
                _ => {}
            }
        }
        let nonce = nonce.ok_or_else(|| refused("has no nonce"))?;
        let salt = salt.ok_or_else(|| refused("has no salt"))?;
        let iterations = iterations
            .filter(|&count| count > 0)
            .ok_or_else(|| refused("has no iteration count"))?;
        if nonce.len() <= self.nonce.len() || !nonce.starts_with(&self.nonce) {
            return Err(refused("does not extend the client's nonce"));
        }
        if !password.is_ascii() {
            return Err(PgError::Authentication(
                "a password with non-ASCII characters needs SASLprep, which this driver does not apply".to_owned(),
            ));
        }
        let digest = match &*reused.load() {
            Some(cached) if cached.salt == salt && cached.iterations == iterations => cached.digest,
            _ => {
                let digest = salted(password.as_bytes(), &salt, iterations);
                reused.store(Some(Arc::new(Salted {
                    salt,
                    iterations,
                    digest,
                })));
                digest
            }
        };
        let salted = Hmac::new(&digest);
        let client_key = salted.sign(&[b"Client Key"]);
        let stored_key = Sha256::digest(&client_key);
        let without_proof = format!("c={},r={nonce}", STANDARD.encode(GS2_HEADER));
        let auth = [
            self.first_bare.as_bytes(),
            b",",
            server_first.as_bytes(),
            b",",
            without_proof.as_bytes(),
        ];
        let signature = Hmac::new(&stored_key).sign(&auth);
        let mut proof = client_key;
        for (byte, mask) in proof.iter_mut().zip(signature) {
            *byte ^= mask;
        }
        let server_key = salted.sign(&[b"Server Key"]);
        Ok(Proof {
            message: format!("{without_proof},p={}", STANDARD.encode(proof)),
            server_signature: Hmac::new(&server_key).sign(&auth),
        })
    }
}

impl Proof {
    pub(super) fn verify(&self, server_final: &[u8]) -> Result<(), PgError> {
        let refused =
            |what: String| PgError::Authentication(format!("the server's SCRAM answer {what}"));
        let server_final =
            std::str::from_utf8(server_final).map_err(|_| refused("is not UTF-8".to_owned()))?;
        if let Some(error) = server_final.strip_prefix("e=") {
            return Err(refused(format!("is an error: {error}")));
        }
        let signature = server_final
            .split(',')
            .find_map(|attribute| attribute.strip_prefix("v="))
            .ok_or_else(|| refused("has no signature".to_owned()))?;
        let signature = STANDARD
            .decode(signature)
            .map_err(|_| refused("has a signature that is not base64".to_owned()))?;
        if signature != self.server_signature {
            return Err(refused(
                "does not prove the server knows the password".to_owned(),
            ));
        }
        Ok(())
    }
}
