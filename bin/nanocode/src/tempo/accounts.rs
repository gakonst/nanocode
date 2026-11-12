use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    str::FromStr,
};

use nanocode_tempo::{
    Address, P256Signature, PrimitiveSignature, Signature, SignatureType, U256, WebAuthnSignature,
    hex,
};
use serde::{Deserialize, Deserializer, de};
use serde_json::Value;

const TEMPO_HOME: &str = "TEMPO_HOME";
const STORE: &str = "wallet/store.json";
const DEFAULT_STORE: &str = ".tempo/wallet/store.json";
const BIGINT_SUFFIX: &str = "#__bigint";
const HEX_RADIX: u64 = 16;
const DECIMAL_RADIX: u64 = 10;
const UNCOMPRESSED_PREFIX: u8 = 4;
const WORD: usize = 32;
const SELECTOR: usize = 4;

#[derive(Debug)]
pub(crate) enum AccountsError {
    HomeUnavailable,
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Decode {
        path: PathBuf,
        source: serde_json::Error,
    },
    ActiveAccount,
    MissingChainId,
    AuthorizationResolutionRequired,
    MissingAccessKey {
        account: Address,
        chain_id: u64,
    },
    InvalidMetadata,
}

impl fmt::Display for AccountsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HomeUnavailable => f.write_str("home directory is unavailable"),
            Self::Read { path, source } => write!(
                f,
                "failed to read Tempo Accounts store at {}: {source}",
                path.display()
            ),
            Self::Decode { path, source } => write!(
                f,
                "invalid Tempo Accounts store at {}: {source}",
                path.display()
            ),
            Self::ActiveAccount => f.write_str("Tempo Accounts active account is missing or invalid"),
            Self::MissingChainId => f.write_str("Tempo Accounts needs a chain ID to select the access key"),
            Self::AuthorizationResolutionRequired => f.write_str(
                "Tempo Accounts store key has an unresolved authorization; call prepare_request or use the wallet as a provider filler",
            ),
            Self::MissingAccessKey { account, chain_id } => write!(
                f,
                "Tempo Accounts has no locally signable access key for account {account} on chain {chain_id}"
            ),
            Self::InvalidMetadata => f.write_str("invalid Tempo Accounts transaction metadata"),
        }
    }
}

impl std::error::Error for AccountsError {}

#[derive(Clone, Copy)]
pub(crate) struct Hex(pub(crate) Address);

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = std::borrow::Cow::<'de, str>::deserialize(deserializer)?;
        Address::from_str(&text)
            .map(Self)
            .map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Word(pub(crate) [u8; WORD]);

impl<'de> Deserialize<'de> for Word {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = std::borrow::Cow::<'de, str>::deserialize(deserializer)?;
        hex::decode_array(&text)
            .map(Self)
            .map_err(de::Error::custom)
    }
}

#[derive(Clone)]
pub(crate) struct HexBytes(pub(crate) Vec<u8>);

impl<'de> Deserialize<'de> for HexBytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = std::borrow::Cow::<'de, str>::deserialize(deserializer)?;
        hex::decode(&text).map(Self).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AccountsU256(pub(crate) U256);

impl<'de> Deserialize<'de> for AccountsU256 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl de::Visitor<'_> for Visitor {
            type Value = AccountsU256;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a non-negative integer or Accounts bigint string")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(AccountsU256(U256::from_u64(value)))
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                let value = value.strip_suffix(BIGINT_SUFFIX).unwrap_or(value);
                let parsed = match value.strip_prefix("0x") {
                    Some(digits) => U256::from_str_radix(digits, HEX_RADIX),
                    None => U256::from_str_radix(value, DECIMAL_RADIX),
                };
                parsed.map(AccountsU256).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AccountsU64(pub(crate) u64);

impl<'de> Deserialize<'de> for AccountsU64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = AccountsU256::deserialize(deserializer)?;
        value
            .0
            .to_u64()
            .map(Self)
            .ok_or_else(|| de::Error::custom("Accounts integer exceeds u64"))
    }
}

#[derive(Clone, Deserialize)]
struct TempoCliStore {
    #[serde(rename = "tempo-cli.store")]
    store: PersistedStoreEnvelope,
}

#[derive(Clone, Deserialize)]
struct PersistedStoreEnvelope {
    state: PersistedAccountsState,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedAccountsState {
    active_account: PersistedActiveAccount,
    pub(crate) chain_id: u64,
    pub(crate) accounts: Vec<PersistedAccount>,
    #[serde(default)]
    pub(crate) access_keys: Vec<PersistedAccessKey>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(untagged)]
enum PersistedActiveAccount {
    Index(usize),
    Address(Hex),
}

#[derive(Clone, Deserialize)]
pub(crate) struct PersistedAccount {
    pub(crate) address: Hex,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedAccessKey {
    pub(crate) address: Hex,
    pub(crate) access: Hex,
    pub(crate) chain_id: u64,
    pub(crate) key_type: PersistedKeyType,
    #[serde(default)]
    pub(crate) expiry: Option<u64>,
    #[serde(default)]
    pub(crate) handle: Option<Value>,
    #[serde(default)]
    pub(crate) private_key: Option<Word>,
    #[serde(default)]
    pub(crate) public_key: Option<HexBytes>,
    #[serde(default, rename = "limits")]
    _limits: Vec<PersistedTokenLimit>,
    #[serde(default)]
    pub(crate) scopes: Option<Vec<PersistedScope>>,
    #[serde(default)]
    pub(crate) key_authorization: Option<PersistedKeyAuthorization>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub(crate) enum PersistedKeyType {
    #[serde(rename = "secp256k1")]
    Secp256k1,
    #[serde(rename = "p256")]
    P256,
    #[serde(rename = "webAuthn")]
    WebAuthn,
    #[serde(rename = "webCrypto")]
    WebCrypto,
    #[serde(other)]
    Unsupported,
}

impl PersistedKeyType {
    pub(crate) const fn signature_type(self) -> Result<SignatureType, PersistedKeyError> {
        match self {
            Self::Secp256k1 => Ok(SignatureType::Secp256k1),
            Self::P256 | Self::WebCrypto => Ok(SignatureType::P256),
            Self::WebAuthn => Ok(SignatureType::WebAuthn),
            Self::Unsupported => Err(PersistedKeyError::UnsupportedKeyType),
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct PersistedKeyHandleKind {
    pub(crate) kind: PersistedHandleKind,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedPrivateKeyHandle {
    #[serde(default)]
    pub(crate) private_key: Option<Word>,
}

#[derive(Deserialize)]
pub(crate) struct PersistedWebCryptoHandle {
    #[serde(default)]
    pub(crate) jwk: Option<Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub(crate) enum PersistedHandleKind {
    #[serde(rename = "secp256k1")]
    Secp256k1,
    #[serde(rename = "p256")]
    P256,
    #[serde(rename = "webcrypto-p256")]
    WebCryptoP256,
    #[serde(other)]
    Unsupported,
}

#[derive(Clone, Deserialize)]
pub(crate) struct PersistedScope {
    #[serde(alias = "target")]
    pub(crate) address: Hex,
    #[serde(default)]
    pub(crate) selector: Option<PersistedSelector>,
    #[serde(default)]
    pub(crate) recipients: Option<Vec<Hex>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PersistedSelector(pub(crate) [u8; SELECTOR]);

impl<'de> Deserialize<'de> for PersistedSelector {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let encoded = std::borrow::Cow::<'de, str>::deserialize(deserializer)?;
        if let Some(digits) = encoded.strip_prefix("0x") {
            let bytes = hex::decode(digits).map_err(de::Error::custom)?;
            return bytes
                .try_into()
                .map(Self)
                .map_err(|_| de::Error::custom("call selector must contain exactly four bytes"));
        }
        if !encoded.contains('(') || !encoded.ends_with(')') {
            return Err(de::Error::custom(
                "call selector must be four-byte hex or an ABI function signature",
            ));
        }
        Ok(Self(nanocode_tempo::abi::selector(&encoded)))
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedSignedKeyAuthorization {
    pub(crate) address: Hex,
    pub(crate) chain_id: AccountsU64,
    #[serde(default)]
    pub(crate) expiry: Option<AccountsU64>,
    #[serde(default)]
    pub(crate) limits: Option<Vec<PersistedTokenLimit>>,
    #[serde(default)]
    pub(crate) scopes: Option<Vec<PersistedScope>>,
    #[serde(default)]
    pub(crate) witness: Option<Word>,
    #[serde(default)]
    pub(crate) is_admin: bool,
    #[serde(default)]
    pub(crate) account: Option<Hex>,
    #[serde(rename = "type")]
    pub(crate) key_type: PersistedKeyType,
    pub(crate) signature: PersistedPrimitiveSignature,
}

#[derive(Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum PersistedKeyAuthorization {
    Structured(Box<PersistedSignedKeyAuthorization>),
    Rlp(String),
}

#[derive(Clone, Deserialize)]
pub(crate) struct PersistedTokenLimit {
    pub(crate) token: Hex,
    pub(crate) limit: AccountsU256,
    #[serde(default)]
    pub(crate) period: Option<AccountsU64>,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum PersistedPrimitiveSignature {
    #[serde(rename = "secp256k1")]
    Secp256k1 { signature: PersistedSecpSignature },
    #[serde(rename = "p256")]
    P256 {
        signature: PersistedRs,
        #[serde(rename = "publicKey")]
        public_key: PersistedPublicKey,
        #[serde(default, alias = "preHash")]
        prehash: bool,
    },
    #[serde(rename = "webAuthn")]
    WebAuthn {
        signature: PersistedRs,
        #[serde(rename = "publicKey")]
        public_key: PersistedPublicKey,
        metadata: PersistedWebAuthnMetadata,
    },
    #[serde(other)]
    Unsupported,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedSecpSignature {
    r: AccountsU256,
    s: AccountsU256,
    y_parity: AccountsU256,
}

#[derive(Clone, Deserialize)]
pub(crate) struct PersistedRs {
    r: AccountsU256,
    s: AccountsU256,
}

#[derive(Clone, Deserialize)]
pub(crate) struct PersistedPublicKey {
    #[serde(default)]
    prefix: Option<u8>,
    x: AccountsU256,
    y: AccountsU256,
}

impl PersistedPublicKey {
    fn validate_prefix(&self) -> Result<(), PersistedKeyError> {
        match self
            .prefix
            .is_none_or(|prefix| prefix == UNCOMPRESSED_PREFIX)
        {
            true => Ok(()),
            false => Err(PersistedKeyError::InvalidPublicKeyPrefix),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedWebAuthnMetadata {
    authenticator_data: HexBytes,
    #[serde(rename = "clientDataJSON")]
    client_data_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PersistedKeyError {
    UnsupportedKeyType,
    MissingCredential,
    InvalidPrivateKey,
    InvalidHandle,
    MissingJwk,
    MissingPublicKey,
    P256(super::keys::P256SignerError),
    UnsupportedAuthorizationSignature,
    InvalidAuthorizationRlp,
    ZeroAuthorizationExpiry,
    RecipientsWithoutSelector,
    AuthorizationScopesMismatch,
    InvalidCallScope,
    InvalidRecipientConstraint,
    InvalidYParity,
    InvalidPublicKeyPrefix,
}

impl fmt::Display for PersistedKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedKeyType => "unsupported access-key type",
            Self::MissingCredential => "access key has no local credential",
            Self::InvalidPrivateKey => "invalid local private key",
            Self::InvalidHandle => "invalid built-in access-key handle",
            Self::MissingJwk => "WebCrypto P-256 handle is missing its JWK",
            Self::MissingPublicKey => "WebCrypto P-256 handle is missing its public key",
            Self::P256(error) => return write!(f, "invalid P-256 key: {error}"),
            Self::UnsupportedAuthorizationSignature => {
                "unsupported key-authorization signature type"
            }
            Self::InvalidAuthorizationRlp => "invalid RLP key authorization",
            Self::ZeroAuthorizationExpiry => "key-authorization expiry must be non-zero",
            Self::RecipientsWithoutSelector => "scope recipients require an explicit selector",
            Self::AuthorizationScopesMismatch => {
                "call-scope metadata disagrees with the signed authorization"
            }
            Self::InvalidCallScope => "call-scope targets and selectors must be unique and nonzero",
            Self::InvalidRecipientConstraint => {
                "recipient constraints require a TIP-20 transfer or approve selector"
            }
            Self::InvalidYParity => "invalid secp256k1 y parity",
            Self::InvalidPublicKeyPrefix => "P-256 public key prefix must be uncompressed (4)",
        })
    }
}

impl std::error::Error for PersistedKeyError {}

impl From<PersistedTokenLimit> for nanocode_tempo::TokenLimit {
    fn from(value: PersistedTokenLimit) -> Self {
        Self {
            token: value.token.0,
            limit: value.limit.0,
            period: value.period.map_or(0, |period| period.0),
        }
    }
}

fn parity(value: U256) -> Result<bool, PersistedKeyError> {
    match value {
        value if value == U256::ZERO => Ok(false),
        value if value == U256::ONE => Ok(true),
        _ => Err(PersistedKeyError::InvalidYParity),
    }
}

impl TryFrom<PersistedPrimitiveSignature> for PrimitiveSignature {
    type Error = PersistedKeyError;

    fn try_from(value: PersistedPrimitiveSignature) -> Result<Self, Self::Error> {
        match value {
            PersistedPrimitiveSignature::Secp256k1 { signature } => {
                Ok(Self::Secp256k1(Signature {
                    r: signature.r.0.to_be_bytes(),
                    s: signature.s.0.to_be_bytes(),
                    y_parity: parity(signature.y_parity.0)?,
                }))
            }
            PersistedPrimitiveSignature::P256 {
                signature,
                public_key,
                prehash,
            } => {
                public_key.validate_prefix()?;
                Ok(Self::P256(P256Signature {
                    r: signature.r.0.to_be_bytes(),
                    s: signature.s.0.to_be_bytes(),
                    public_x: public_key.x.0.to_be_bytes(),
                    public_y: public_key.y.0.to_be_bytes(),
                    pre_hash: prehash,
                }))
            }
            PersistedPrimitiveSignature::WebAuthn {
                signature,
                public_key,
                metadata,
            } => {
                public_key.validate_prefix()?;
                let mut data = metadata.authenticator_data.0;
                data.extend_from_slice(metadata.client_data_json.as_bytes());
                Ok(Self::WebAuthn(WebAuthnSignature {
                    r: signature.r.0.to_be_bytes(),
                    s: signature.s.0.to_be_bytes(),
                    public_x: public_key.x.0.to_be_bytes(),
                    public_y: public_key.y.0.to_be_bytes(),
                    data,
                }))
            }
            PersistedPrimitiveSignature::Unsupported => {
                Err(PersistedKeyError::UnsupportedAuthorizationSignature)
            }
        }
    }
}

pub(crate) fn default_store_path() -> Result<PathBuf, AccountsError> {
    if let Some(directory) = std::env::var_os(TEMPO_HOME).filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(directory).join(STORE));
    }
    std::env::home_dir()
        .map(|directory| directory.join(DEFAULT_STORE))
        .ok_or(AccountsError::HomeUnavailable)
}

pub(crate) fn load_state(path: &Path) -> Result<PersistedAccountsState, AccountsError> {
    let bytes = fs::read(path).map_err(|source| AccountsError::Read {
        path: path.to_owned(),
        source,
    })?;
    if let Ok(root) = serde_json::from_slice::<TempoCliStore>(&bytes) {
        return Ok(root.store.state);
    }
    if let Ok(envelope) = serde_json::from_slice::<PersistedStoreEnvelope>(&bytes) {
        return Ok(envelope.state);
    }
    serde_json::from_slice(&bytes).map_err(|source| AccountsError::Decode {
        path: path.to_owned(),
        source,
    })
}

pub(crate) fn active_account(state: &PersistedAccountsState) -> Result<Address, AccountsError> {
    match state.active_account {
        PersistedActiveAccount::Index(index) => state
            .accounts
            .get(index)
            .map(|account| account.address.0)
            .ok_or(AccountsError::ActiveAccount),
        PersistedActiveAccount::Address(Hex(address)) => state
            .accounts
            .iter()
            .any(|account| account.address.0 == address)
            .then_some(address)
            .ok_or(AccountsError::ActiveAccount),
    }
}

pub(crate) fn default_active_account() -> Result<Address, AccountsError> {
    active_account(&load_state(&default_store_path()?)?)
}
