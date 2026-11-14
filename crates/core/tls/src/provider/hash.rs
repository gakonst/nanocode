use graviola::hashing::{self, HashContext, hmac::Hmac};
use rustls::crypto::{
    hash::{self, HashAlgorithm},
    hmac,
};
use std::marker::PhantomData;

const SHA256_OUTPUT_BYTES: usize = 32;
const SHA384_OUTPUT_BYTES: usize = 48;

pub(super) static SHA256: Hashing<hashing::Sha256> =
    Hashing::new(HashAlgorithm::SHA256, SHA256_OUTPUT_BYTES);
pub(super) static SHA384: Hashing<hashing::Sha384> =
    Hashing::new(HashAlgorithm::SHA384, SHA384_OUTPUT_BYTES);
pub(super) static HMAC_SHA256: Hmacing<hashing::Sha256> = Hmacing::new(SHA256_OUTPUT_BYTES);
pub(super) static HMAC_SHA384: Hmacing<hashing::Sha384> = Hmacing::new(SHA384_OUTPUT_BYTES);

pub(super) struct Hashing<H> {
    algorithm: HashAlgorithm,
    output_bytes: usize,
    hash: PhantomData<fn() -> H>,
}

impl<H> Hashing<H> {
    const fn new(algorithm: HashAlgorithm, output_bytes: usize) -> Self {
        Self {
            algorithm,
            output_bytes,
            hash: PhantomData,
        }
    }
}

impl<H: hashing::Hash + 'static> hash::Hash for Hashing<H>
where
    H::Context: Send + Sync + 'static,
{
    fn start(&self) -> Box<dyn hash::Context> {
        Box::new(Context::<H>(H::new()))
    }

    fn hash(&self, data: &[u8]) -> hash::Output {
        hash::Output::new(H::hash(data).as_ref())
    }

    fn algorithm(&self) -> HashAlgorithm {
        self.algorithm
    }

    fn output_len(&self) -> usize {
        self.output_bytes
    }
}

struct Context<H: hashing::Hash>(H::Context);

impl<H: hashing::Hash + 'static> hash::Context for Context<H>
where
    H::Context: Send + Sync + 'static,
{
    fn fork_finish(&self) -> hash::Output {
        hash::Output::new(self.0.clone().finish().as_ref())
    }

    fn fork(&self) -> Box<dyn hash::Context> {
        Box::new(Self(self.0.clone()))
    }

    fn finish(self: Box<Self>) -> hash::Output {
        hash::Output::new(self.0.finish().as_ref())
    }

    fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
}

pub(super) struct Hmacing<H> {
    output_bytes: usize,
    hash: PhantomData<fn() -> H>,
}

impl<H> Hmacing<H> {
    const fn new(output_bytes: usize) -> Self {
        Self {
            output_bytes,
            hash: PhantomData,
        }
    }
}

impl<H: hashing::Hash + Clone + Send + Sync + 'static> hmac::Hmac for Hmacing<H>
where
    H::Context: Send + Sync + 'static,
{
    fn with_key(&self, key: &[u8]) -> Box<dyn hmac::Key> {
        Box::new(Key {
            hmac: Hmac::<H>::new(key),
            output_bytes: self.output_bytes,
        })
    }

    fn hash_output_len(&self) -> usize {
        self.output_bytes
    }
}

struct Key<H: hashing::Hash> {
    hmac: Hmac<H>,
    output_bytes: usize,
}

impl<H: hashing::Hash + Clone + Send + Sync + 'static> hmac::Key for Key<H>
where
    H::Context: Send + Sync + 'static,
{
    fn sign_concat(&self, first: &[u8], middle: &[&[u8]], last: &[u8]) -> hmac::Tag {
        let mut hmac = self.hmac.clone();
        hmac.update(first);
        for part in middle {
            hmac.update(part);
        }
        hmac.update(last);
        hmac::Tag::new(hmac.finish().as_ref())
    }

    fn tag_len(&self) -> usize {
        self.output_bytes
    }
}
