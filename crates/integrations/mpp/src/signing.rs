use nanocode_tempo::{
    Address, KeychainSignature, PrimitiveSignature, SignatureType, SignedKeyAuthorization,
    TempoSignature, TempoTransaction, U256,
};

use super::error::Fault;
use super::keys::{AccessKey, KeyError, KeySigner, SignedTransaction, unix_now};
use super::rpc::Rpc;
use super::tx::{self, KeyInfo};

const HASH_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum KeychainVersion {
    #[default]
    V2,
}

#[derive(Clone, Debug, Default)]
pub(crate) enum TempoSigningMode {
    #[default]
    Direct,
    Keychain {
        wallet: Address,
        key_authorization: Option<Box<SignedKeyAuthorization>>,
        version: KeychainVersion,
    },
}

impl TempoSigningMode {
    #[allow(clippy::wrong_self_convention)]
    pub(crate) const fn from_address(&self, signer_address: Address) -> Address {
        match self {
            Self::Direct => signer_address,
            Self::Keychain { wallet, .. } => *wallet,
        }
    }
}

pub(crate) fn effective_signing_hash(
    hash: [u8; HASH_BYTES],
    mode: &TempoSigningMode,
) -> [u8; HASH_BYTES] {
    match mode {
        TempoSigningMode::Direct => hash,
        TempoSigningMode::Keychain {
            wallet,
            version: KeychainVersion::V2,
            ..
        } => KeychainSignature::signing_hash(&hash, wallet),
    }
}

pub(crate) const fn build_tempo_signature_primitive(
    primitive: PrimitiveSignature,
    mode: &TempoSigningMode,
) -> TempoSignature {
    match mode {
        TempoSigningMode::Direct => TempoSignature::Primitive(primitive),
        TempoSigningMode::Keychain {
            wallet,
            version: KeychainVersion::V2,
            ..
        } => TempoSignature::Keychain(KeychainSignature {
            account: *wallet,
            signature: primitive,
        }),
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PrimitiveSigner {
    Key(KeySigner),
    Accounts(AccessKey),
}

impl From<KeySigner> for PrimitiveSigner {
    fn from(value: KeySigner) -> Self {
        Self::Key(value)
    }
}

impl From<AccessKey> for PrimitiveSigner {
    fn from(value: AccessKey) -> Self {
        Self::Accounts(value)
    }
}

impl PrimitiveSigner {
    pub(crate) const fn signature_type(&self) -> SignatureType {
        match self {
            Self::Key(signer) => signer.signature_type(),
            Self::Accounts(signer) => signer.signature_type(),
        }
    }

    pub(crate) fn address(&self) -> Address {
        match self {
            Self::Key(signer) => signer.address(),
            Self::Accounts(signer) => signer.address(),
        }
    }

    pub(crate) fn sign_hash(
        &self,
        hash: &[u8; HASH_BYTES],
    ) -> Result<PrimitiveSignature, KeyError> {
        match self {
            Self::Key(signer) => signer.sign_hash(hash),
            Self::Accounts(signer) => signer.sign_hash(hash),
        }
    }
}

fn signed(
    transaction: &TempoTransaction,
    signer: &PrimitiveSigner,
    mode: &TempoSigningMode,
) -> Result<TempoSignature, Fault> {
    let hash = effective_signing_hash(transaction.signature_hash(), mode);
    let signature = signer
        .sign_hash(&hash)
        .map_err(|error| Fault::http(format!("failed to sign transaction: {error}")))?;
    Ok(build_tempo_signature_primitive(signature, mode))
}

pub(crate) fn sign_and_encode(
    transaction: TempoTransaction,
    signer: &PrimitiveSigner,
    mode: &TempoSigningMode,
) -> Result<Vec<u8>, Fault> {
    let signature = signed(&transaction, signer, mode)?;
    Ok(transaction.encode_signed(&signature))
}

pub(crate) async fn sign_and_encode_primitive_async(
    transaction: TempoTransaction,
    signer: &PrimitiveSigner,
    mode: &TempoSigningMode,
) -> Result<Vec<u8>, Fault> {
    sign_and_encode(transaction, signer, mode)
}

fn check_envelope(transaction: &TempoTransaction, what: &str) -> Result<(), Fault> {
    if transaction.fee_payer_signature.is_none() {
        return Err(Fault::config(format!(
            "fee payer {what} requires fee_payer_signature placeholder; build tx with TempoTxOptions {{ fee_payer: true }}"
        )));
    }
    if transaction.fee_token.is_some() {
        return Err(Fault::config(format!(
            "fee payer {what} must not include fee_token (server chooses)"
        )));
    }
    if transaction.nonce_key != U256::MAX {
        return Err(Fault::config(format!(
            "fee payer {what} must use expiring nonce key (U256::MAX)"
        )));
    }
    if transaction.valid_before.is_none() {
        return Err(Fault::config(format!(
            "fee payer {what} must include valid_before"
        )));
    }
    Ok(())
}

pub(crate) fn sign_and_encode_fee_payer_envelope(
    transaction: TempoTransaction,
    signer: &PrimitiveSigner,
    mode: &TempoSigningMode,
) -> Result<Vec<u8>, Fault> {
    let sender = mode.from_address(signer.address());
    check_envelope(&transaction, "envelope")?;
    let signature = signed(&transaction, signer, mode)?;
    Ok(transaction.encode_fee_payer_envelope(&sender, &signature))
}

pub(crate) async fn sign_and_encode_fee_payer_envelope_primitive_async(
    transaction: TempoTransaction,
    signer: &PrimitiveSigner,
    mode: &TempoSigningMode,
) -> Result<Vec<u8>, Fault> {
    sign_and_encode_fee_payer_envelope(transaction, signer, mode)
}

pub(crate) fn encode_signed_fee_payer_envelope(
    signed: SignedTransaction,
    sender: Address,
) -> Result<Vec<u8>, Fault> {
    let SignedTransaction {
        transaction,
        signature,
    } = signed;
    if transaction.fee_payer_signature.is_none() {
        return Err(Fault::config(
            "fee payer envelope requires a fee_payer_signature placeholder",
        ));
    }
    if transaction.fee_token.is_some() {
        return Err(Fault::config(
            "fee payer envelope must not include fee_token",
        ));
    }
    if transaction.nonce_key != U256::MAX {
        return Err(Fault::config(
            "fee payer envelope must use the expiring nonce key",
        ));
    }
    if transaction.valid_before.is_none() {
        return Err(Fault::config(
            "fee payer envelope must include valid_before",
        ));
    }
    Ok(transaction.encode_fee_payer_envelope(&sender, &signature))
}

pub(crate) async fn resolve_key_authorization(
    rpc: &Rpc,
    signing_mode: &TempoSigningMode,
    key_address: Address,
) -> Result<Option<SignedKeyAuthorization>, Fault> {
    let TempoSigningMode::Keychain {
        wallet,
        key_authorization: Some(key_authorization),
        ..
    } = signing_mode
    else {
        return Ok(None);
    };
    let now = unix_now();
    if key_authorization
        .authorization
        .expiry
        .is_some_and(|expiry| expiry <= now)
    {
        return Ok(None);
    }
    let key_info = tx::get_key(rpc, *wallet, key_address)
        .await
        .map_err(|error| {
            Fault::http(format!("failed to query key authorization state: {error}"))
        })?;
    match should_attach_key_authorization(key_authorization, &key_info, key_address, now) {
        true => Ok(Some(key_authorization.as_ref().clone())),
        false => Ok(None),
    }
}

fn should_attach_key_authorization(
    key_authorization: &SignedKeyAuthorization,
    key_info: &KeyInfo,
    key_address: Address,
    now: u64,
) -> bool {
    if key_authorization
        .authorization
        .expiry
        .is_some_and(|expiry| expiry <= now)
    {
        return false;
    }
    key_info.key_id != key_address || key_info.is_revoked || key_info.expiry <= now
}
