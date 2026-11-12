use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    str::FromStr,
};

use nanocode_tempo::Address;
use serde::{Deserialize, Deserializer, de};

const TEMPO_HOME: &str = "TEMPO_HOME";
const STORE: &str = "wallet/store.json";
const DEFAULT_STORE: &str = ".tempo/wallet/store.json";

#[derive(Debug)]
pub(crate) enum AccountsError {
    HomeUnavailable,
    Read { path: PathBuf, source: io::Error },
    Decode { path: PathBuf, source: serde_json::Error },
    ActiveAccount,
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
        }
    }
}

impl std::error::Error for AccountsError {}

#[derive(Clone, Copy)]
pub(crate) struct Hex(pub(crate) Address);

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = std::borrow::Cow::<'de, str>::deserialize(deserializer)?;
        Address::from_str(&text).map(Self).map_err(de::Error::custom)
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
    #[allow(dead_code)]
    chain_id: u64,
    accounts: Vec<PersistedAccount>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(untagged)]
enum PersistedActiveAccount {
    Index(usize),
    Address(Hex),
}

#[derive(Clone, Deserialize)]
struct PersistedAccount {
    address: Hex,
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
