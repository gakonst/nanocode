use crate::{
    Address, Error, U256,
    base64,
    ecdsa::{self, Signature},
    keccak::keccak256,
    rlp,
    sha256::{Sha256, sha256},
};

const BYTES: usize = 32;
const P256_BODY: usize = 4 * BYTES + 1;
const WEBAUTHN_TAIL: usize = 4 * BYTES;
const WEBAUTHN_MAX: usize = 2048;
const AUTHENTICATOR_DATA: usize = 37;
const FLAGS_OFFSET: usize = 32;
const FLAG_USER_PRESENT: u8 = 0x01;
const FLAG_USER_VERIFIED: u8 = 0x04;
const FLAG_ATTESTED: u8 = 0x40;
const FLAG_EXTENSIONS: u8 = 0x80;
const WEBAUTHN_GET: &str = "webauthn.get";

pub const TYPE_P256: u8 = 0x01;
pub const TYPE_WEBAUTHN: u8 = 0x02;
pub const TYPE_KEYCHAIN_V2: u8 = 0x04;
const SECP256K1_LENGTH: usize = 65;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SignatureType {
    Secp256k1,
    P256,
    WebAuthn,
}

impl SignatureType {
    pub const fn code(self) -> u8 {
        match self {
            Self::Secp256k1 => 0,
            Self::P256 => 1,
            Self::WebAuthn => 2,
        }
    }

    pub const fn from_code(code: u8) -> Result<Self, Error> {
        match code {
            0 => Ok(Self::Secp256k1),
            1 => Ok(Self::P256),
            2 => Ok(Self::WebAuthn),
            _ => Err(Error::Rlp("invalid signature type")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct P256Signature {
    pub r: [u8; BYTES],
    pub s: [u8; BYTES],
    pub public_x: [u8; BYTES],
    pub public_y: [u8; BYTES],
    pub pre_hash: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebAuthnSignature {
    pub r: [u8; BYTES],
    pub s: [u8; BYTES],
    pub public_x: [u8; BYTES],
    pub public_y: [u8; BYTES],
    pub data: Vec<u8>,
}

pub struct ClientData<'a> {
    pub kind: &'a str,
    pub challenge: &'a str,
}

impl WebAuthnSignature {
    pub fn client_data_json(&self) -> Result<&[u8], Error> {
        if self.data.len() < AUTHENTICATOR_DATA + BYTES {
            return Err(Error::InvalidSignature);
        }
        let flags = self.data[FLAGS_OFFSET];
        if flags & (FLAG_USER_PRESENT | FLAG_USER_VERIFIED) == 0
            || flags & FLAG_ATTESTED != 0
            || flags & FLAG_EXTENSIONS != 0
        {
            return Err(Error::InvalidSignature);
        }
        Ok(&self.data[AUTHENTICATOR_DATA..])
    }

    pub fn recover(&self, hash: &[u8; BYTES], client: &ClientData<'_>) -> Result<Address, Error> {
        let client_json = self.client_data_json()?;
        if client.kind != WEBAUTHN_GET || client.challenge != base64::encode_url(hash) {
            return Err(Error::InvalidSignature);
        }
        let mut hasher = Sha256::new();
        hasher.update(&self.data[..AUTHENTICATOR_DATA]);
        hasher.update(&sha256(client_json));
        let message = hasher.finalize();
        verify_p256(&message, &self.r, &self.s, &self.public_x, &self.public_y)
    }
}

fn verify_p256(
    hash: &[u8; BYTES],
    r: &[u8; BYTES],
    s: &[u8; BYTES],
    x: &[u8; BYTES],
    y: &[u8; BYTES],
) -> Result<Address, Error> {
    if ecdsa::p256_s_is_high(s) {
        return Err(Error::InvalidSignature);
    }
    ecdsa::verify_p256(hash, r, s, x, y)?;
    Ok(Address::from_public_key(x, y))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrimitiveSignature {
    Secp256k1(Signature),
    P256(P256Signature),
    WebAuthn(WebAuthnSignature),
}

impl PrimitiveSignature {
    pub const fn signature_type(&self) -> SignatureType {
        match self {
            Self::Secp256k1(_) => SignatureType::Secp256k1,
            Self::P256(_) => SignatureType::P256,
            Self::WebAuthn(_) => SignatureType::WebAuthn,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Self::Secp256k1(signature) => signature.to_bytes().to_vec(),
            Self::P256(signature) => {
                let mut bytes = Vec::with_capacity(1 + P256_BODY);
                bytes.push(TYPE_P256);
                bytes.extend_from_slice(&signature.r);
                bytes.extend_from_slice(&signature.s);
                bytes.extend_from_slice(&signature.public_x);
                bytes.extend_from_slice(&signature.public_y);
                bytes.push(signature.pre_hash as u8);
                bytes
            }
            Self::WebAuthn(signature) => {
                let mut bytes = Vec::with_capacity(1 + signature.data.len() + WEBAUTHN_TAIL);
                bytes.push(TYPE_WEBAUTHN);
                bytes.extend_from_slice(&signature.data);
                bytes.extend_from_slice(&signature.r);
                bytes.extend_from_slice(&signature.s);
                bytes.extend_from_slice(&signature.public_x);
                bytes.extend_from_slice(&signature.public_y);
                bytes
            }
        }
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, Error> {
        if data.len() == SECP256K1_LENGTH {
            return Signature::from_bytes(data).map(Self::Secp256k1);
        }
        let (&kind, body) = data.split_first().ok_or(Error::InvalidSignature)?;
        let word = |offset: usize| -> [u8; BYTES] {
            let mut word = [0u8; BYTES];
            word.copy_from_slice(&body[offset..offset + BYTES]);
            word
        };
        match kind {
            TYPE_P256 if body.len() == P256_BODY => Ok(Self::P256(P256Signature {
                r: word(0),
                s: word(BYTES),
                public_x: word(2 * BYTES),
                public_y: word(3 * BYTES),
                pre_hash: body[4 * BYTES] != 0,
            })),
            TYPE_WEBAUTHN if (WEBAUTHN_TAIL..=WEBAUTHN_MAX).contains(&body.len()) => {
                let start = body.len() - WEBAUTHN_TAIL;
                let tail = |offset: usize| word(start + offset);
                Ok(Self::WebAuthn(WebAuthnSignature {
                    r: tail(0),
                    s: tail(BYTES),
                    public_x: tail(2 * BYTES),
                    public_y: tail(3 * BYTES),
                    data: body[..start].to_vec(),
                }))
            }
            _ => Err(Error::InvalidSignature),
        }
    }

    pub fn recover(&self, hash: &[u8; BYTES]) -> Result<Address, Error> {
        match self {
            Self::Secp256k1(signature) => ecdsa::recover_secp256k1(hash, signature),
            Self::P256(signature) => {
                let message = match signature.pre_hash {
                    true => sha256(hash),
                    false => *hash,
                };
                verify_p256(
                    &message,
                    &signature.r,
                    &signature.s,
                    &signature.public_x,
                    &signature.public_y,
                )
            }
            Self::WebAuthn(_) => Err(Error::InvalidSignature),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeychainSignature {
    pub account: Address,
    pub signature: PrimitiveSignature,
}

impl KeychainSignature {
    pub fn signing_hash(hash: &[u8; BYTES], account: &Address) -> [u8; BYTES] {
        let mut buffer = [0u8; 1 + BYTES + crate::address::ADDRESS_BYTES];
        buffer[0] = TYPE_KEYCHAIN_V2;
        buffer[1..1 + BYTES].copy_from_slice(hash);
        buffer[1 + BYTES..].copy_from_slice(account.as_bytes());
        keccak256(buffer)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let inner = self.signature.to_bytes();
        let mut bytes = Vec::with_capacity(1 + crate::address::ADDRESS_BYTES + inner.len());
        bytes.push(TYPE_KEYCHAIN_V2);
        bytes.extend_from_slice(self.account.as_bytes());
        bytes.extend_from_slice(&inner);
        bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TempoSignature {
    Primitive(PrimitiveSignature),
    Keychain(KeychainSignature),
}

impl TempoSignature {
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Self::Primitive(signature) => signature.to_bytes(),
            Self::Keychain(signature) => signature.to_bytes(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenLimit {
    pub token: Address,
    pub limit: U256,
    pub period: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectorRule {
    pub selector: [u8; 4],
    pub recipients: Vec<Address>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallScope {
    pub target: Address,
    pub selector_rules: Vec<SelectorRule>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyAuthorization {
    pub chain_id: u64,
    pub key_type: SignatureType,
    pub key_id: Address,
    pub expiry: Option<u64>,
    pub limits: Option<Vec<TokenLimit>>,
    pub allowed_calls: Option<Vec<CallScope>>,
    pub witness: Option<[u8; BYTES]>,
    pub is_admin: bool,
    pub account: Option<Address>,
}

impl KeyAuthorization {
    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut payload = Vec::new();
        rlp::encode_u64(&mut payload, self.chain_id);
        rlp::encode_u64(&mut payload, self.key_type.code() as u64);
        rlp::encode_bytes(&mut payload, self.key_id.as_bytes());
        let present = [
            self.expiry.is_some(),
            self.limits.is_some(),
            self.allowed_calls.is_some(),
            self.witness.is_some(),
            self.is_admin,
            self.account.is_some(),
        ];
        let later = |index: usize| present[index + 1..].iter().any(|present| *present);
        match self.expiry {
            Some(expiry) => rlp::encode_u64(&mut payload, expiry),
            None if later(0) => payload.push(rlp::EMPTY_STRING),
            None => {}
        }
        match &self.limits {
            Some(limits) => encode_limits(&mut payload, limits),
            None if later(1) => payload.push(rlp::EMPTY_STRING),
            None => {}
        }
        match &self.allowed_calls {
            Some(scopes) => encode_scopes(&mut payload, scopes),
            None if later(2) => payload.push(rlp::EMPTY_STRING),
            None => {}
        }
        match &self.witness {
            Some(witness) => rlp::encode_bytes(&mut payload, witness),
            None if later(3) => payload.push(rlp::EMPTY_STRING),
            None => {}
        }
        match self.is_admin {
            true => rlp::encode_u64(&mut payload, 1),
            false if later(4) => payload.push(rlp::EMPTY_STRING),
            false => {}
        }
        if let Some(account) = &self.account {
            rlp::encode_bytes(&mut payload, account.as_bytes());
        }
        rlp::encode_list(out, &payload);
    }

    pub fn signature_hash(&self) -> [u8; BYTES] {
        let mut encoded = Vec::new();
        self.encode(&mut encoded);
        keccak256(encoded)
    }

    fn decode(reader: &mut rlp::Reader<'_>) -> Result<Self, Error> {
        let mut fields = reader.list()?;
        let chain_id = fields.u64()?;
        let key_type = SignatureType::from_code(u8::try_from(fields.u64()?).map_err(|_| Error::Rlp("invalid signature type"))?)?;
        let key_id = Address(fields.fixed()?);
        let absent = |fields: &mut rlp::Reader<'_>| {
            if fields.is_empty() {
                return true;
            }
            if fields.peek() == Some(rlp::EMPTY_STRING) && fields.remaining() > 1 {
                let _ = fields.item();
                return true;
            }
            false
        };
        let expiry = match absent(&mut fields) {
            true => None,
            false => Some(nonzero(fields.u64()?)?),
        };
        let limits = match absent(&mut fields) {
            true => None,
            false => Some(decode_limits(&mut fields)?),
        };
        let allowed_calls = match absent(&mut fields) {
            true => None,
            false => Some(decode_scopes(&mut fields)?),
        };
        let witness = match absent(&mut fields) {
            true => None,
            false => Some(fields.fixed()?),
        };
        let is_admin = match absent(&mut fields) {
            true => false,
            false => match fields.u64()? {
                1 => true,
                _ => return Err(Error::Rlp("invalid admin key authorization marker")),
            },
        };
        let account = match absent(&mut fields) {
            true => None,
            false => Some(Address(fields.fixed()?)),
        };
        fields.finish()?;
        Ok(Self {
            chain_id,
            key_type,
            key_id,
            expiry,
            limits,
            allowed_calls,
            witness,
            is_admin,
            account,
        })
    }
}

const fn nonzero(value: u64) -> Result<u64, Error> {
    match value {
        0 => Err(Error::Rlp("expected a non-zero integer")),
        value => Ok(value),
    }
}

fn encode_limits(out: &mut Vec<u8>, limits: &[TokenLimit]) {
    let mut payload = Vec::new();
    for limit in limits {
        let mut item = Vec::new();
        rlp::encode_bytes(&mut item, limit.token.as_bytes());
        rlp::encode_uint(&mut item, &limit.limit.to_be_bytes());
        if limit.period != 0 {
            rlp::encode_u64(&mut item, limit.period);
        }
        rlp::encode_list(&mut payload, &item);
    }
    rlp::encode_list(out, &payload);
}

fn decode_limits(reader: &mut rlp::Reader<'_>) -> Result<Vec<TokenLimit>, Error> {
    let mut items = reader.list()?;
    let mut limits = Vec::new();
    while !items.is_empty() {
        let mut item = items.list()?;
        let token = Address(item.fixed()?);
        let limit = U256::from_be_slice(item.uint(BYTES)?)?;
        let period = match item.is_empty() {
            true => 0,
            false => nonzero(item.u64()?)?,
        };
        item.finish()?;
        limits.push(TokenLimit { token, limit, period });
    }
    Ok(limits)
}

fn encode_scopes(out: &mut Vec<u8>, scopes: &[CallScope]) {
    let mut payload = Vec::new();
    for scope in scopes {
        let mut item = Vec::new();
        rlp::encode_bytes(&mut item, scope.target.as_bytes());
        let mut rules = Vec::new();
        for rule in &scope.selector_rules {
            let mut entry = Vec::new();
            rlp::encode_bytes(&mut entry, &rule.selector);
            let mut recipients = Vec::new();
            for recipient in &rule.recipients {
                rlp::encode_bytes(&mut recipients, recipient.as_bytes());
            }
            rlp::encode_list(&mut entry, &recipients);
            rlp::encode_list(&mut rules, &entry);
        }
        rlp::encode_list(&mut item, &rules);
        rlp::encode_list(&mut payload, &item);
    }
    rlp::encode_list(out, &payload);
}

fn decode_scopes(reader: &mut rlp::Reader<'_>) -> Result<Vec<CallScope>, Error> {
    let mut items = reader.list()?;
    let mut scopes = Vec::new();
    while !items.is_empty() {
        let mut item = items.list()?;
        let target = Address(item.fixed()?);
        let mut rules_reader = item.list()?;
        let mut selector_rules = Vec::new();
        while !rules_reader.is_empty() {
            let mut entry = rules_reader.list()?;
            let selector = entry.fixed()?;
            let mut recipients_reader = entry.list()?;
            let mut recipients = Vec::new();
            while !recipients_reader.is_empty() {
                recipients.push(Address(recipients_reader.fixed()?));
            }
            entry.finish()?;
            selector_rules.push(SelectorRule { selector, recipients });
        }
        item.finish()?;
        scopes.push(CallScope { target, selector_rules });
    }
    Ok(scopes)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedKeyAuthorization {
    pub authorization: KeyAuthorization,
    pub signature: PrimitiveSignature,
}

impl SignedKeyAuthorization {
    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut payload = Vec::new();
        self.authorization.encode(&mut payload);
        rlp::encode_bytes(&mut payload, &self.signature.to_bytes());
        rlp::encode_list(out, &payload);
    }

    pub fn decode(data: &[u8]) -> Result<Self, Error> {
        let mut reader = rlp::Reader::new(data);
        let mut fields = reader.list()?;
        reader.finish()?;
        let authorization = KeyAuthorization::decode(&mut fields)?;
        let signature = PrimitiveSignature::from_bytes(fields.bytes()?)?;
        fields.finish()?;
        Ok(Self {
            authorization,
            signature,
        })
    }

    pub fn recover_signer(&self) -> Result<Address, Error> {
        self.signature.recover(&self.authorization.signature_hash())
    }
}
