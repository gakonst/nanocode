use graviola::aead::{AesGcm as Gcm, ChaCha20Poly1305};
use rustls::{
    ConnectionTrafficSecrets, ContentType, Error, ProtocolVersion,
    crypto::cipher::{
        AeadKey, InboundOpaqueMessage, InboundPlainMessage, Iv, KeyBlockShape, MessageDecrypter,
        MessageEncrypter, NONCE_LEN, Nonce, OutboundOpaqueMessage, OutboundPlainMessage,
        PrefixedPayload, Tls12AeadAlgorithm, Tls13AeadAlgorithm, UnsupportedOperationError,
        make_tls12_aad, make_tls13_aad,
    },
};

pub(super) const AES_128_KEY_BYTES: usize = 16;
pub(super) const AES_256_KEY_BYTES: usize = 32;
const CHACHA20_KEY_BYTES: usize = 32;
const TAG_BYTES: usize = 16;
const CONTENT_TYPE_BYTES: usize = 1;
const GCM_FIXED_IV_BYTES: usize = 4;
const GCM_EXPLICIT_NONCE_BYTES: usize = 8;
const GCM_TLS12_OVERHEAD: usize = GCM_EXPLICIT_NONCE_BYTES + TAG_BYTES;
const MAX_FRAGMENT_BYTES: usize = 16_384;

pub(super) struct Chacha20Poly1305;

fn chacha(key: &AeadKey) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(
        key.as_ref()
            .try_into()
            .expect("rustls derives a ChaCha20-Poly1305 key of key_len bytes"),
    )
}

impl Tls13AeadAlgorithm for Chacha20Poly1305 {
    fn encrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageEncrypter> {
        Box::new(Tls13Cipher(Cipher::Chacha(chacha(&key)), iv))
    }

    fn decrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageDecrypter> {
        Box::new(Tls13Cipher(Cipher::Chacha(chacha(&key)), iv))
    }

    fn key_len(&self) -> usize {
        CHACHA20_KEY_BYTES
    }

    fn extract_keys(
        &self,
        key: AeadKey,
        iv: Iv,
    ) -> Result<ConnectionTrafficSecrets, UnsupportedOperationError> {
        Ok(ConnectionTrafficSecrets::Chacha20Poly1305 { key, iv })
    }
}

impl Tls12AeadAlgorithm for Chacha20Poly1305 {
    fn encrypter(&self, key: AeadKey, iv: &[u8], _: &[u8]) -> Box<dyn MessageEncrypter> {
        Box::new(ChachaTls12(chacha(&key), Iv::copy(iv)))
    }

    fn decrypter(&self, key: AeadKey, iv: &[u8]) -> Box<dyn MessageDecrypter> {
        Box::new(ChachaTls12(chacha(&key), Iv::copy(iv)))
    }

    fn key_block_shape(&self) -> KeyBlockShape {
        KeyBlockShape {
            enc_key_len: CHACHA20_KEY_BYTES,
            fixed_iv_len: NONCE_LEN,
            explicit_nonce_len: 0,
        }
    }

    fn extract_keys(
        &self,
        key: AeadKey,
        iv: &[u8],
        _explicit: &[u8],
    ) -> Result<ConnectionTrafficSecrets, UnsupportedOperationError> {
        Ok(ConnectionTrafficSecrets::Chacha20Poly1305 {
            key,
            iv: Iv::copy(iv),
        })
    }
}

pub(super) struct AesGcm(pub(super) usize);

impl AesGcm {
    fn secrets(&self, key: AeadKey, iv: Iv) -> ConnectionTrafficSecrets {
        match self.0 {
            AES_128_KEY_BYTES => ConnectionTrafficSecrets::Aes128Gcm { key, iv },
            AES_256_KEY_BYTES => ConnectionTrafficSecrets::Aes256Gcm { key, iv },
            other => unreachable!("an AES-GCM suite has a 16 or 32 byte key, not {other}"),
        }
    }
}

impl Tls13AeadAlgorithm for AesGcm {
    fn encrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageEncrypter> {
        Box::new(Tls13Cipher(Cipher::Gcm(Gcm::new(key.as_ref())), iv))
    }

    fn decrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageDecrypter> {
        Box::new(Tls13Cipher(Cipher::Gcm(Gcm::new(key.as_ref())), iv))
    }

    fn key_len(&self) -> usize {
        self.0
    }

    fn extract_keys(
        &self,
        key: AeadKey,
        iv: Iv,
    ) -> Result<ConnectionTrafficSecrets, UnsupportedOperationError> {
        Ok(self.secrets(key, iv))
    }
}

impl Tls12AeadAlgorithm for AesGcm {
    fn encrypter(
        &self,
        key: AeadKey,
        write_iv: &[u8],
        explicit: &[u8],
    ) -> Box<dyn MessageEncrypter> {
        Box::new(GcmTls12Encrypter(
            Gcm::new(key.as_ref()),
            gcm_iv(write_iv, explicit),
        ))
    }

    fn decrypter(&self, key: AeadKey, iv: &[u8]) -> Box<dyn MessageDecrypter> {
        Box::new(GcmTls12Decrypter(
            Gcm::new(key.as_ref()),
            iv.try_into()
                .expect("rustls derives a TLS 1.2 AES-GCM fixed IV of fixed_iv_len bytes"),
        ))
    }

    fn key_block_shape(&self) -> KeyBlockShape {
        KeyBlockShape {
            enc_key_len: self.0,
            fixed_iv_len: GCM_FIXED_IV_BYTES,
            explicit_nonce_len: GCM_EXPLICIT_NONCE_BYTES,
        }
    }

    fn extract_keys(
        &self,
        key: AeadKey,
        iv: &[u8],
        explicit: &[u8],
    ) -> Result<ConnectionTrafficSecrets, UnsupportedOperationError> {
        Ok(self.secrets(key, gcm_iv(iv, explicit)))
    }
}

enum Cipher {
    Chacha(ChaCha20Poly1305),
    Gcm(Gcm),
}

impl Cipher {
    fn encrypt(
        &self,
        nonce: &[u8; NONCE_LEN],
        aad: &[u8],
        data: &mut [u8],
        tag: &mut [u8; TAG_BYTES],
    ) {
        match self {
            Self::Chacha(cipher) => cipher.encrypt(nonce, aad, data, tag),
            Self::Gcm(cipher) => cipher.encrypt(nonce, aad, data, tag),
        }
    }

    fn decrypt(
        &self,
        nonce: &[u8; NONCE_LEN],
        aad: &[u8],
        data: &mut [u8],
        tag: &[u8],
    ) -> Result<(), Error> {
        match self {
            Self::Chacha(cipher) => cipher.decrypt(nonce, aad, data, tag),
            Self::Gcm(cipher) => cipher.decrypt(nonce, aad, data, tag),
        }
        .map_err(|_| Error::DecryptError)
    }
}

struct Tls13Cipher(Cipher, Iv);

impl MessageEncrypter for Tls13Cipher {
    fn encrypt(
        &mut self,
        message: OutboundPlainMessage<'_>,
        seq: u64,
    ) -> Result<OutboundOpaqueMessage, Error> {
        let total = self.encrypted_payload_len(message.payload.len());
        let mut payload = PrefixedPayload::with_capacity(total);
        payload.extend_from_chunks(&message.payload);
        payload.extend_from_slice(&message.typ.to_array());
        let nonce = Nonce::new(&self.1, seq);
        let mut tag = [0; TAG_BYTES];
        self.0
            .encrypt(&nonce.0, &make_tls13_aad(total), payload.as_mut(), &mut tag);
        payload.extend_from_slice(&tag);
        Ok(OutboundOpaqueMessage::new(
            ContentType::ApplicationData,
            ProtocolVersion::TLSv1_2,
            payload,
        ))
    }

    fn encrypted_payload_len(&self, payload_len: usize) -> usize {
        payload_len + CONTENT_TYPE_BYTES + TAG_BYTES
    }
}

impl MessageDecrypter for Tls13Cipher {
    fn decrypt<'a>(
        &mut self,
        mut message: InboundOpaqueMessage<'a>,
        seq: u64,
    ) -> Result<InboundPlainMessage<'a>, Error> {
        let payload = &mut message.payload;
        let Some(plain) = payload.len().checked_sub(TAG_BYTES) else {
            return Err(Error::DecryptError);
        };
        let nonce = Nonce::new(&self.1, seq);
        let aad = make_tls13_aad(payload.len());
        let (cipher, tag) = payload.split_at_mut(plain);
        self.0.decrypt(&nonce.0, &aad, cipher, tag)?;
        payload.truncate(plain);
        message.into_tls13_unpadded_message()
    }
}

struct ChachaTls12(ChaCha20Poly1305, Iv);

impl MessageEncrypter for ChachaTls12 {
    fn encrypt(
        &mut self,
        message: OutboundPlainMessage<'_>,
        seq: u64,
    ) -> Result<OutboundOpaqueMessage, Error> {
        let total = self.encrypted_payload_len(message.payload.len());
        let mut payload = PrefixedPayload::with_capacity(total);
        payload.extend_from_chunks(&message.payload);
        let nonce = Nonce::new(&self.1, seq);
        let aad = make_tls12_aad(seq, message.typ, message.version, message.payload.len());
        let mut tag = [0; TAG_BYTES];
        self.0.encrypt(&nonce.0, &aad, payload.as_mut(), &mut tag);
        payload.extend_from_slice(&tag);
        Ok(OutboundOpaqueMessage::new(
            message.typ,
            message.version,
            payload,
        ))
    }

    fn encrypted_payload_len(&self, payload_len: usize) -> usize {
        payload_len + TAG_BYTES
    }
}

impl MessageDecrypter for ChachaTls12 {
    fn decrypt<'a>(
        &mut self,
        mut message: InboundOpaqueMessage<'a>,
        seq: u64,
    ) -> Result<InboundPlainMessage<'a>, Error> {
        let Some(plain) = message.payload.len().checked_sub(TAG_BYTES) else {
            return Err(Error::DecryptError);
        };
        let nonce = Nonce::new(&self.1, seq);
        let aad = make_tls12_aad(seq, message.typ, message.version, plain);
        let (cipher, tag) = message.payload.split_at_mut(plain);
        self.0
            .decrypt(&nonce.0, &aad, cipher, tag)
            .map_err(|_| Error::DecryptError)?;
        message.payload.truncate(plain);
        Ok(message.into_plain_message())
    }
}

struct GcmTls12Encrypter(Gcm, Iv);

impl MessageEncrypter for GcmTls12Encrypter {
    fn encrypt(
        &mut self,
        message: OutboundPlainMessage<'_>,
        seq: u64,
    ) -> Result<OutboundOpaqueMessage, Error> {
        let total = self.encrypted_payload_len(message.payload.len());
        let mut payload = PrefixedPayload::with_capacity(total);
        let nonce = Nonce::new(&self.1, seq);
        let aad = make_tls12_aad(seq, message.typ, message.version, message.payload.len());
        payload.extend_from_slice(&nonce.0[GCM_FIXED_IV_BYTES..]);
        payload.extend_from_chunks(&message.payload);
        let mut tag = [0; TAG_BYTES];
        self.0.encrypt(
            &nonce.0,
            &aad,
            &mut payload.as_mut()[GCM_EXPLICIT_NONCE_BYTES..],
            &mut tag,
        );
        payload.extend_from_slice(&tag);
        Ok(OutboundOpaqueMessage::new(
            message.typ,
            message.version,
            payload,
        ))
    }

    fn encrypted_payload_len(&self, payload_len: usize) -> usize {
        payload_len + GCM_TLS12_OVERHEAD
    }
}

struct GcmTls12Decrypter(Gcm, [u8; GCM_FIXED_IV_BYTES]);

impl MessageDecrypter for GcmTls12Decrypter {
    fn decrypt<'a>(
        &mut self,
        mut message: InboundOpaqueMessage<'a>,
        seq: u64,
    ) -> Result<InboundPlainMessage<'a>, Error> {
        let Some(plain) = message.payload.len().checked_sub(GCM_TLS12_OVERHEAD) else {
            return Err(Error::DecryptError);
        };
        let mut nonce = [0; NONCE_LEN];
        nonce[..GCM_FIXED_IV_BYTES].copy_from_slice(&self.1);
        nonce[GCM_FIXED_IV_BYTES..].copy_from_slice(&message.payload[..GCM_EXPLICIT_NONCE_BYTES]);
        let aad = make_tls12_aad(seq, message.typ, message.version, plain);
        let (_, cipher) = message.payload.split_at_mut(GCM_EXPLICIT_NONCE_BYTES);
        let (cipher, tag) = cipher.split_at_mut(plain);
        self.0
            .decrypt(&nonce, &aad, cipher, tag)
            .map_err(|_| Error::DecryptError)?;
        if plain > MAX_FRAGMENT_BYTES {
            return Err(Error::PeerSentOversizedRecord);
        }
        Ok(message
            .into_plain_message_range(GCM_EXPLICIT_NONCE_BYTES..GCM_EXPLICIT_NONCE_BYTES + plain))
    }
}

fn gcm_iv(write_iv: &[u8], explicit: &[u8]) -> Iv {
    assert_eq!(
        write_iv.len(),
        GCM_FIXED_IV_BYTES,
        "a TLS 1.2 AES-GCM fixed IV"
    );
    assert_eq!(
        explicit.len(),
        GCM_EXPLICIT_NONCE_BYTES,
        "a TLS 1.2 AES-GCM explicit nonce"
    );
    let mut iv = [0; NONCE_LEN];
    iv[..GCM_FIXED_IV_BYTES].copy_from_slice(write_iv);
    iv[GCM_FIXED_IV_BYTES..].copy_from_slice(explicit);
    Iv::new(iv)
}
