use graviola::key_agreement::{mlkem768, p256, p384, x25519};
use rustls::{
    Error, NamedGroup, PeerMisbehaved, ProtocolVersion,
    crypto::{
        ActiveKeyExchange, CompletedKeyExchange, GetRandomFailed, SharedSecret, SupportedKxGroup,
    },
    ffdhe_groups::FfdheGroup,
};

const X25519_SHARE_BYTES: usize = 32;
const P256_SHARE_BYTES: usize = 65;
const P384_SHARE_BYTES: usize = 97;
const MLKEM768_ENCAPSULATION_KEY_BYTES: usize = 1184;
const MLKEM768_CIPHERTEXT_BYTES: usize = 1088;
const INVALID_KEY_SHARE: Error = Error::PeerMisbehaved(PeerMisbehaved::InvalidKeyShare);

pub(super) static ALL: &[&dyn SupportedKxGroup] = &[&X25519MLKEM768, &X25519, &P256, &P384];

fn random_failed<E>(_: E) -> Error {
    Error::from(GetRandomFailed)
}

fn invalid_share<E>(_: E) -> Error {
    INVALID_KEY_SHARE
}

#[derive(Debug)]
struct X25519;

impl SupportedKxGroup for X25519 {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let private = x25519::PrivateKey::new_random().map_err(random_failed)?;
        let public = private.public_key().as_bytes();
        Ok(Box::new(ActiveX25519 { private, public }))
    }

    fn ffdhe_group(&self) -> Option<FfdheGroup<'static>> {
        None
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::X25519
    }
}

struct ActiveX25519 {
    private: x25519::PrivateKey,
    public: [u8; X25519_SHARE_BYTES],
}

impl ActiveKeyExchange for ActiveX25519 {
    fn complete(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, Error> {
        let shared = x25519::PublicKey::try_from_slice(peer)
            .and_then(|peer| self.private.diffie_hellman(&peer))
            .map_err(invalid_share)?;
        Ok(SharedSecret::from(&shared.0[..]))
    }

    fn pub_key(&self) -> &[u8] {
        &self.public
    }

    fn ffdhe_group(&self) -> Option<FfdheGroup<'static>> {
        None
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::X25519
    }
}

#[derive(Debug)]
struct P256;

impl SupportedKxGroup for P256 {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let private = p256::PrivateKey::new_random().map_err(random_failed)?;
        let public = private.public_key_uncompressed();
        Ok(Box::new(ActiveP256 { private, public }))
    }

    fn ffdhe_group(&self) -> Option<FfdheGroup<'static>> {
        None
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::secp256r1
    }
}

struct ActiveP256 {
    private: p256::PrivateKey,
    public: [u8; P256_SHARE_BYTES],
}

impl ActiveKeyExchange for ActiveP256 {
    fn complete(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, Error> {
        let peer = p256::PublicKey::from_x962_uncompressed(peer).map_err(invalid_share)?;
        let shared = self.private.diffie_hellman(&peer).map_err(invalid_share)?;
        Ok(SharedSecret::from(&shared.0[..]))
    }

    fn pub_key(&self) -> &[u8] {
        &self.public
    }

    fn ffdhe_group(&self) -> Option<FfdheGroup<'static>> {
        None
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::secp256r1
    }
}

#[derive(Debug)]
struct P384;

impl SupportedKxGroup for P384 {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let private = p384::PrivateKey::new_random().map_err(random_failed)?;
        let public = private.public_key_uncompressed();
        Ok(Box::new(ActiveP384 { private, public }))
    }

    fn ffdhe_group(&self) -> Option<FfdheGroup<'static>> {
        None
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::secp384r1
    }
}

struct ActiveP384 {
    private: p384::PrivateKey,
    public: [u8; P384_SHARE_BYTES],
}

impl ActiveKeyExchange for ActiveP384 {
    fn complete(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, Error> {
        let peer = p384::PublicKey::from_x962_uncompressed(peer).map_err(invalid_share)?;
        let shared = self.private.diffie_hellman(&peer).map_err(invalid_share)?;
        Ok(SharedSecret::from(&shared.0[..]))
    }

    fn pub_key(&self) -> &[u8] {
        &self.public
    }

    fn ffdhe_group(&self) -> Option<FfdheGroup<'static>> {
        None
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::secp384r1
    }
}

#[derive(Debug)]
struct X25519MLKEM768;

impl X25519MLKEM768 {
    const fn split(share: &[u8], post_quantum_bytes: usize) -> Result<(&[u8], &[u8]), Error> {
        if share.len() != post_quantum_bytes + X25519_SHARE_BYTES {
            return Err(INVALID_KEY_SHARE);
        }
        Ok(share.split_at(post_quantum_bytes))
    }
}

impl SupportedKxGroup for X25519MLKEM768 {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let classical = X25519.start()?;
        let decapsulation =
            mlkem768::DecapKey::generate().map_err(|_| Error::FailedToGetRandomBytes)?;
        let encapsulation: [u8; MLKEM768_ENCAPSULATION_KEY_BYTES] =
            decapsulation.encapsulation_key().as_bytes();
        let public = [&encapsulation[..], classical.pub_key()].concat();
        Ok(Box::new(ActiveHybrid {
            classical,
            decapsulation,
            public,
        }))
    }

    fn start_and_complete(&self, client_share: &[u8]) -> Result<CompletedKeyExchange, Error> {
        let (post_quantum, classical) =
            Self::split(client_share, MLKEM768_ENCAPSULATION_KEY_BYTES)?;
        let classical = X25519.start_and_complete(classical)?;
        let encapsulation =
            mlkem768::EncapKey::from_bytes(post_quantum.try_into().map_err(invalid_share)?)
                .map_err(invalid_share)?;
        let (shared, ciphertext) = encapsulation
            .encaps()
            .map_err(|_| Error::FailedToGetRandomBytes)?;
        Ok(CompletedKeyExchange {
            group: NamedGroup::X25519MLKEM768,
            pub_key: [ciphertext.as_ref(), &classical.pub_key[..]].concat(),
            secret: SharedSecret::from(
                [shared.as_ref().as_slice(), classical.secret.secret_bytes()].concat(),
            ),
        })
    }

    fn ffdhe_group(&self) -> Option<FfdheGroup<'static>> {
        None
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::X25519MLKEM768
    }

    fn fips(&self) -> bool {
        false
    }

    fn usable_for_version(&self, version: ProtocolVersion) -> bool {
        version == ProtocolVersion::TLSv1_3
    }
}

struct ActiveHybrid {
    classical: Box<dyn ActiveKeyExchange>,
    decapsulation: mlkem768::DecapKey,
    public: Vec<u8>,
}

impl ActiveKeyExchange for ActiveHybrid {
    fn complete(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, Error> {
        let (post_quantum, classical) = X25519MLKEM768::split(peer, MLKEM768_CIPHERTEXT_BYTES)?;
        let classical = self.classical.complete(classical)?;
        let ciphertext: [u8; MLKEM768_CIPHERTEXT_BYTES] =
            post_quantum.try_into().map_err(invalid_share)?;
        let shared = self
            .decapsulation
            .decaps(&mlkem768::Ciphertext::from(ciphertext));
        Ok(SharedSecret::from(
            [shared.as_ref().as_slice(), classical.secret_bytes()].concat(),
        ))
    }

    fn hybrid_component(&self) -> Option<(NamedGroup, &[u8])> {
        Some((self.classical.group(), self.classical.pub_key()))
    }

    fn complete_hybrid_component(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, Error> {
        self.classical.complete(peer)
    }

    fn pub_key(&self) -> &[u8] {
        &self.public
    }

    fn ffdhe_group(&self) -> Option<FfdheGroup<'static>> {
        None
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::X25519MLKEM768
    }
}
