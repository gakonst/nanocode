use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nanocode_sync::Atom;
use nanocode_tempo::{Address, Sha256, hex};
use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::accounts;
use super::error::Fault;
use super::session::channel::{Hash, parse_hash};
use super::session::wire::{ChannelDescriptor, SettlementRoute};

mod sqlite;

const DATABASE_FILE: &str = "channels.db";
const DOCUMENT_EXTENSION: &str = "json";
const LOCK_EXTENSION: &str = "lock";
const TEMPORARY_EXTENSION: &str = "tmp";
const DOCUMENT_VERSION: u32 = 1;
const SCHEMA_VERSION: i64 = 2;
const LEGACY_VERSION: i64 = 1;
const ACTIVE: &str = "active";
const TIP1034_PROTOCOL: &str = "v2";
const LEGACY_PROTOCOL: &str = "v1";
const EMPTY_ECHO: &str = "{}";
const ZERO_AMOUNT: &str = "0";
const CHANNELS_TABLE: &str = "channels";
#[cfg(unix)]
const PRIVATE_FILE_MODE: u32 = 0o600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredChannelEntry {
    pub(crate) channel_id: Hash,
    pub(crate) cumulative_amount: u128,
    pub(crate) deposit: u128,
    pub(crate) descriptor: ChannelDescriptor,
    pub(crate) settlement_route: Option<SettlementRoute>,
    pub(crate) escrow: Address,
    pub(crate) chain_id: u64,
    pub(crate) opened: bool,
    pub(crate) pending_open: Option<String>,
    pub(crate) origin: Option<String>,
}

impl StoredChannelEntry {
    pub(crate) fn key(&self) -> String {
        if let Some(route) = &self.settlement_route {
            return format!(
                "{}:{}:{}:{:#x}:{}",
                route.adapter.to_ascii_lowercase(),
                route.recipient.to_ascii_lowercase(),
                route.target_token.to_ascii_lowercase(),
                self.escrow,
                self.chain_id,
            );
        }
        channel_key(
            &self.descriptor.payee,
            &self.descriptor.token,
            self.escrow,
            self.chain_id,
        )
    }
}

pub(crate) fn channel_key(payee: &str, token: &str, escrow: Address, chain_id: u64) -> String {
    format!(
        "{}:{}:{:#x}:{}",
        payee.to_ascii_lowercase(),
        token.to_ascii_lowercase(),
        escrow,
        chain_id
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoreFault {
    Io(String),
    InvalidEntry(String),
}

impl std::fmt::Display for StoreFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) => write!(f, "channel store I/O failed: {message}"),
            Self::InvalidEntry(message) => write!(f, "invalid persisted channel: {message}"),
        }
    }
}

impl std::error::Error for StoreFault {}

impl From<StoreFault> for Fault {
    fn from(error: StoreFault) -> Self {
        Self::Config(format!("channel store failed: {error}"))
    }
}

fn io_error(error: impl std::fmt::Display) -> StoreFault {
    StoreFault::Io(error.to_string())
}

fn origin_mismatch() -> StoreFault {
    StoreFault::InvalidEntry("stored channel origin mismatch".into())
}

fn merge_origin(
    current: Option<&StoredChannelEntry>,
    entry: &StoredChannelEntry,
) -> Result<Option<String>, StoreFault> {
    match (
        current.and_then(|current| current.origin.as_ref()),
        &entry.origin,
    ) {
        (Some(current), Some(origin)) if current != origin => Err(origin_mismatch()),
        (Some(current), _) => Ok(Some(current.clone())),
        (None, origin) => Ok(origin.clone()),
    }
}

fn merged(
    current: Option<StoredChannelEntry>,
    entry: &StoredChannelEntry,
) -> Result<StoredChannelEntry, StoreFault> {
    let origin = merge_origin(current.as_ref(), entry)?;
    let mut merged = match current {
        Some(current) if current.channel_id == entry.channel_id => StoredChannelEntry {
            cumulative_amount: current.cumulative_amount.max(entry.cumulative_amount),
            deposit: current.deposit.max(entry.deposit),
            ..entry.clone()
        },
        _ => entry.clone(),
    };
    merged.origin = origin;
    Ok(merged)
}

pub(crate) fn default_channel_database_path() -> Result<PathBuf, Fault> {
    accounts::default_store_path()
        .map(|path| path.with_file_name(DATABASE_FILE))
        .map_err(|error| Fault::Io(error.to_string()))
}

pub(crate) enum StoreLease {
    Process { _permit: OwnedSemaphorePermit },
    File { _file: File },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonChannelEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
    channel_id: String,
    cumulative_amount: String,
    deposit: String,
    descriptor: ChannelDescriptor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settlement_route: Option<SettlementRoute>,
    escrow: String,
    chain_id: u64,
    opened: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_open: Option<String>,
}

impl From<&StoredChannelEntry> for JsonChannelEntry {
    fn from(entry: &StoredChannelEntry) -> Self {
        Self {
            origin: entry.origin.clone(),
            channel_id: hex::encode_prefixed(entry.channel_id),
            cumulative_amount: entry.cumulative_amount.to_string(),
            deposit: entry.deposit.to_string(),
            descriptor: entry.descriptor.clone(),
            settlement_route: entry.settlement_route.clone(),
            escrow: format!("{:#x}", entry.escrow),
            chain_id: entry.chain_id,
            opened: entry.opened,
            pending_open: entry.pending_open.clone(),
        }
    }
}

impl TryFrom<JsonChannelEntry> for StoredChannelEntry {
    type Error = StoreFault;

    fn try_from(entry: JsonChannelEntry) -> Result<Self, StoreFault> {
        fn invalid(field: &str, error: impl std::fmt::Display) -> StoreFault {
            StoreFault::InvalidEntry(format!("invalid {field}: {error}"))
        }
        Ok(Self {
            channel_id: parse_hash(&entry.channel_id)
                .map_err(|error| invalid("channelId", error))?,
            cumulative_amount: entry
                .cumulative_amount
                .parse()
                .map_err(|error| invalid("cumulativeAmount", error))?,
            deposit: entry
                .deposit
                .parse()
                .map_err(|error| invalid("deposit", error))?,
            descriptor: entry.descriptor,
            settlement_route: entry.settlement_route,
            escrow: entry
                .escrow
                .parse()
                .map_err(|error| invalid("escrow", error))?,
            chain_id: entry.chain_id,
            opened: entry.opened,
            pending_open: entry.pending_open,
            origin: entry.origin,
        })
    }
}

#[derive(Default)]
pub(crate) struct MemoryChannelStore {
    entries: Atom<HashMap<String, StoredChannelEntry>>,
    leases: Atom<HashMap<String, Arc<Semaphore>>>,
    server_spent: Atom<HashMap<String, (Hash, u128, u128)>>,
}

impl MemoryChannelStore {
    async fn acquire(&self, key: &str) -> Result<StoreLease, StoreFault> {
        let fresh = Arc::new(Semaphore::new(1));
        let lock = self.leases.update(|leases| {
            leases
                .entry(key.to_owned())
                .or_insert_with(|| fresh.clone())
                .clone()
        });
        lock.acquire_owned()
            .await
            .map(|permit| StoreLease::Process { _permit: permit })
            .map_err(io_error)
    }

    fn get(&self, key: &str) -> Option<StoredChannelEntry> {
        self.entries.read(|entries| entries.get(key).cloned())
    }

    fn set(&self, entry: &StoredChannelEntry) -> Result<(), StoreFault> {
        self.entries.modify(|entries| {
            let merged = merged(entries.get(&entry.key()).cloned(), entry)?;
            entries.insert(merged.key(), merged);
            Ok(())
        })
    }

    fn delete(&self, key: &str) {
        self.entries.update(|entries| entries.remove(key).map(drop));
        self.server_spent
            .update(|spent| spent.remove(key).map(drop));
    }

    fn cumulative(&self, key: &str, channel_id: &Hash) -> Option<u128> {
        self.entries.read(|entries| {
            entries
                .get(key)
                .filter(|entry| entry.channel_id == *channel_id)
                .map(|entry| entry.cumulative_amount)
        })
    }

    fn server_spent(&self, key: &str, channel_id: &Hash) -> Option<u128> {
        let cumulative = self.cumulative(key, channel_id);
        self.server_spent.read(|spent| {
            spent
                .get(key)
                .filter(|(id, _, recorded)| id == channel_id && Some(*recorded) == cumulative)
                .map(|(_, spent, _)| *spent)
        })
    }

    fn set_server_spent(&self, key: &str, channel_id: &Hash, spent: u128) {
        if let Some(cumulative) = self.cumulative(key, channel_id) {
            self.server_spent.update(|recorded| {
                recorded
                    .insert(key.to_owned(), (*channel_id, spent, cumulative))
                    .map(drop)
            });
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FileChannelStoreOptions {
    pub(crate) namespace: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) request_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Row {
    channel_id: String,
    version: i64,
    scope_key: Option<String>,
    origin: String,
    request_url: String,
    chain_id: i64,
    escrow_contract: String,
    token: String,
    payee: String,
    payer: String,
    authorized_signer: String,
    salt: String,
    deposit: String,
    cumulative_amount: String,
    challenge_echo: String,
    state: String,
    close_requested_at: i64,
    grace_ready_at: i64,
    created_at: i64,
    last_used_at: i64,
    accepted_cumulative: String,
    server_spent: String,
    session_protocol: String,
    descriptor_json: Option<String>,
    entry_json: Option<String>,
    server_spent_cumulative: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Document {
    version: u32,
    imported: bool,
    channels: Vec<Row>,
}

struct FileStore {
    namespace: String,
    origin: String,
    database: PathBuf,
    document: PathBuf,
    request_url: String,
}

#[derive(Clone)]
pub(crate) struct FileChannelStore {
    inner: Arc<FileStore>,
}

fn with_extension(path: &Path, extension: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(extension);
    PathBuf::from(name)
}

fn lock_file(path: &Path) -> Result<File, StoreFault> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(PRIVATE_FILE_MODE);
    }
    let file = options.open(path).map_err(io_error)?;
    file.lock().map_err(io_error)?;
    Ok(file)
}

fn unix_now() -> Result<i64, StoreFault> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io_error)?
        .as_secs();
    i64::try_from(now).map_err(|_| StoreFault::Io("system time exceeds the store's range".into()))
}

impl FileStore {
    fn http_origin(&self) -> Option<String> {
        (self.origin.starts_with("http://") || self.origin.starts_with("https://"))
            .then(|| self.origin.clone())
    }

    fn scoped_key(&self, key: &str) -> String {
        format!("{}\n{}", self.namespace, key)
    }

    fn payment_lock_path(&self, key: &str) -> PathBuf {
        let mut digest = Sha256::new();
        digest.update(self.scoped_key(key).as_bytes());
        let name = self
            .database
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(DATABASE_FILE);
        self.database.with_file_name(format!(
            "{name}.{}.{LOCK_EXTENSION}",
            hex::encode(digest.finalize())
        ))
    }

    fn read(&self) -> Result<Document, StoreFault> {
        read_document(&self.document)
    }

    fn change<T>(
        &self,
        change: impl FnOnce(&mut Document) -> Result<T, StoreFault>,
    ) -> Result<T, StoreFault> {
        let _lock = lock_file(&with_extension(&self.document, LOCK_EXTENSION))?;
        let mut document = self.read()?;
        let output = change(&mut document)?;
        write_document(&self.document, &document)?;
        Ok(output)
    }

    fn get(&self, key: &str) -> Result<Option<StoredChannelEntry>, StoreFault> {
        let scope_key = self.scoped_key(key);
        let document = self.read()?;
        let Some(row) = document
            .channels
            .iter()
            .find(|row| row.scope_key.as_deref() == Some(scope_key.as_str()))
        else {
            return Ok(None);
        };
        let chain_id = u64::try_from(row.chain_id)
            .map_err(|_| StoreFault::InvalidEntry("chainId must be non-negative".into()))?;
        if let Some(json) = &row.entry_json {
            let mut entry: JsonChannelEntry = serde_json::from_str(json)
                .map_err(|error| StoreFault::InvalidEntry(error.to_string()))?;
            entry.opened = row.state == ACTIVE;
            entry.origin = entry.origin.or_else(|| self.http_origin());
            return entry.try_into().map(Some);
        }
        let descriptor = row
            .descriptor_json
            .as_deref()
            .ok_or_else(|| StoreFault::InvalidEntry("v2 row is missing descriptor_json".into()))?;
        JsonChannelEntry {
            origin: self.http_origin(),
            channel_id: row.channel_id.clone(),
            cumulative_amount: row.cumulative_amount.clone(),
            deposit: row.deposit.clone(),
            descriptor: serde_json::from_str(descriptor)
                .map_err(|error| StoreFault::InvalidEntry(error.to_string()))?,
            settlement_route: None,
            escrow: row.escrow_contract.clone(),
            chain_id,
            opened: row.state == ACTIVE,
            pending_open: None,
        }
        .try_into()
        .map(Some)
    }

    fn set(&self, entry: &StoredChannelEntry) -> Result<(), StoreFault> {
        let scope_key = self.scoped_key(&entry.key());
        self.change(|document| {
            let current: Option<StoredChannelEntry> = document
                .channels
                .iter()
                .find(|row| row.scope_key.as_deref() == Some(scope_key.as_str()))
                .and_then(|row| row.entry_json.as_deref())
                .map(|json| {
                    serde_json::from_str::<JsonChannelEntry>(json)
                        .map_err(|error| StoreFault::InvalidEntry(error.to_string()))?
                        .try_into()
                })
                .transpose()?;
            let mut entry = entry.clone();
            if let Some(origin) = self.http_origin() {
                if entry.origin.as_ref().is_some_and(|entry| *entry != origin) {
                    return Err(origin_mismatch());
                }
                entry.origin = Some(origin);
            }
            let channel_id = hex::encode_prefixed(entry.channel_id);
            let conflict = document
                .channels
                .iter()
                .find(|row| row.channel_id == channel_id);
            if conflict.is_some_and(|row| {
                row.scope_key
                    .as_ref()
                    .is_some_and(|scope| *scope != scope_key)
                    || row.origin != self.origin
            }) {
                return Err(origin_mismatch());
            }
            let merged = merged(current, &entry)?;
            let json = serde_json::to_string(&JsonChannelEntry::from(&merged))
                .map_err(|error| StoreFault::InvalidEntry(error.to_string()))?;
            let descriptor = serde_json::to_string(&merged.descriptor)
                .map_err(|error| StoreFault::InvalidEntry(error.to_string()))?;
            let now = unix_now()?;
            let chain_id = i64::try_from(merged.chain_id).map_err(|_| {
                StoreFault::InvalidEntry("chainId exceeds the store's range".into())
            })?;
            document.channels.retain(|row| {
                row.scope_key.as_deref() != Some(scope_key.as_str()) || row.channel_id == channel_id
            });
            let fresh = Row {
                channel_id: channel_id.clone(),
                version: SCHEMA_VERSION,
                scope_key: Some(scope_key.clone()),
                origin: self.origin.clone(),
                request_url: self.request_url.clone(),
                chain_id,
                escrow_contract: format!("{:#x}", merged.escrow),
                token: merged.descriptor.token.clone(),
                payee: merged.descriptor.payee.clone(),
                payer: merged.descriptor.payer.clone(),
                authorized_signer: merged.descriptor.authorized_signer.clone(),
                salt: merged.descriptor.salt.clone(),
                deposit: merged.deposit.to_string(),
                cumulative_amount: merged.cumulative_amount.to_string(),
                challenge_echo: EMPTY_ECHO.to_owned(),
                state: ACTIVE.to_owned(),
                close_requested_at: 0,
                grace_ready_at: 0,
                created_at: now,
                last_used_at: now,
                accepted_cumulative: ZERO_AMOUNT.to_owned(),
                server_spent: ZERO_AMOUNT.to_owned(),
                session_protocol: TIP1034_PROTOCOL.to_owned(),
                descriptor_json: Some(descriptor),
                entry_json: Some(json),
                server_spent_cumulative: None,
            };
            match document
                .channels
                .iter_mut()
                .find(|row| row.channel_id == channel_id)
            {
                Some(row) => {
                    if row
                        .scope_key
                        .as_ref()
                        .is_some_and(|scope| *scope != scope_key)
                        || row.origin != self.origin
                    {
                        return Err(origin_mismatch());
                    }
                    *row = Row {
                        challenge_echo: row.challenge_echo.clone(),
                        grace_ready_at: row.grace_ready_at,
                        created_at: row.created_at,
                        accepted_cumulative: row.accepted_cumulative.clone(),
                        server_spent: row.server_spent.clone(),
                        server_spent_cumulative: row.server_spent_cumulative.clone(),
                        ..fresh
                    };
                }
                None => document.channels.push(fresh),
            }
            Ok(())
        })
    }

    fn delete(&self, key: &str) -> Result<(), StoreFault> {
        let scope_key = self.scoped_key(key);
        self.change(|document| {
            document
                .channels
                .retain(|row| row.scope_key.as_deref() != Some(scope_key.as_str()));
            Ok(())
        })
    }

    fn server_spent(&self, key: &str, channel_id: &Hash) -> Result<Option<u128>, StoreFault> {
        let scope_key = self.scoped_key(key);
        let channel_id = hex::encode_prefixed(channel_id);
        let document = self.read()?;
        Ok(document
            .channels
            .iter()
            .find(|row| {
                row.scope_key.as_deref() == Some(scope_key.as_str()) && row.channel_id == channel_id
            })
            .filter(|row| {
                row.server_spent_cumulative
                    .as_ref()
                    .is_none_or(|recorded| *recorded == row.cumulative_amount)
            })
            .and_then(|row| row.server_spent.parse::<u128>().ok())
            .filter(|spent| *spent > 0))
    }

    fn set_server_spent(
        &self,
        key: &str,
        channel_id: &Hash,
        spent: u128,
    ) -> Result<(), StoreFault> {
        let scope_key = self.scoped_key(key);
        let channel_id = hex::encode_prefixed(channel_id);
        self.change(|document| {
            for row in document.channels.iter_mut().filter(|row| {
                row.scope_key.as_deref() == Some(scope_key.as_str()) && row.channel_id == channel_id
            }) {
                row.server_spent = spent.to_string();
                row.server_spent_cumulative = Some(row.cumulative_amount.clone());
            }
            Ok(())
        })
    }
}

fn read_document(path: &Path) -> Result<Document, StoreFault> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(Document {
                version: DOCUMENT_VERSION,
                ..Document::default()
            });
        }
        Err(error) => return Err(io_error(error)),
    };
    let document: Document = serde_json::from_slice(&bytes)
        .map_err(|error| StoreFault::InvalidEntry(format!("{}: {error}", path.display())))?;
    if document.version != DOCUMENT_VERSION {
        return Err(StoreFault::InvalidEntry(format!(
            "{} has version {}, this nanocode reads version {DOCUMENT_VERSION}",
            path.display(),
            document.version
        )));
    }
    Ok(document)
}

fn write_document(path: &Path, document: &Document) -> Result<(), StoreFault> {
    let bytes = serde_json::to_vec(document)
        .map_err(|error| StoreFault::InvalidEntry(error.to_string()))?;
    let temporary = with_extension(path, TEMPORARY_EXTENSION);
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(PRIVATE_FILE_MODE);
    }
    let mut file = options.open(&temporary).map_err(io_error)?;
    file.write_all(&bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    drop(file);
    fs::rename(&temporary, path).map_err(io_error)
}

fn text(row: &BTreeMap<String, sqlite::Value>, column: &str) -> Option<String> {
    match row.get(column)? {
        sqlite::Value::Text(text) => Some(text.clone()),
        sqlite::Value::Integer(value) => Some(value.to_string()),
        sqlite::Value::Real(value) => Some(value.to_string()),
        sqlite::Value::Null | sqlite::Value::Blob(_) => None,
    }
}

fn integer(row: &BTreeMap<String, sqlite::Value>, column: &str) -> Option<i64> {
    match row.get(column)? {
        sqlite::Value::Integer(value) => Some(*value),
        sqlite::Value::Text(text) => text.trim().parse().ok(),
        sqlite::Value::Null | sqlite::Value::Real(_) | sqlite::Value::Blob(_) => None,
    }
}

fn required<T>(value: Option<T>, column: &str) -> Result<T, StoreFault> {
    value
        .ok_or_else(|| StoreFault::InvalidEntry(format!("{DATABASE_FILE} row is missing {column}")))
}

fn imported_row(row: &BTreeMap<String, sqlite::Value>) -> Result<Row, StoreFault> {
    let fallback =
        |column: &str, default: &str| text(row, column).unwrap_or_else(|| default.to_owned());
    Ok(Row {
        channel_id: required(text(row, "channel_id"), "channel_id")?,
        version: integer(row, "version").unwrap_or(LEGACY_VERSION),
        scope_key: text(row, "scope_key"),
        origin: required(text(row, "origin"), "origin")?,
        request_url: fallback("request_url", ""),
        chain_id: required(integer(row, "chain_id"), "chain_id")?,
        escrow_contract: required(text(row, "escrow_contract"), "escrow_contract")?,
        token: required(text(row, "token"), "token")?,
        payee: required(text(row, "payee"), "payee")?,
        payer: required(text(row, "payer"), "payer")?,
        authorized_signer: required(text(row, "authorized_signer"), "authorized_signer")?,
        salt: required(text(row, "salt"), "salt")?,
        deposit: required(text(row, "deposit"), "deposit")?,
        cumulative_amount: required(text(row, "cumulative_amount"), "cumulative_amount")?,
        challenge_echo: fallback("challenge_echo", EMPTY_ECHO),
        state: fallback("state", ACTIVE),
        close_requested_at: integer(row, "close_requested_at").unwrap_or(0),
        grace_ready_at: integer(row, "grace_ready_at").unwrap_or(0),
        created_at: required(integer(row, "created_at"), "created_at")?,
        last_used_at: required(integer(row, "last_used_at"), "last_used_at")?,
        accepted_cumulative: fallback("accepted_cumulative", ZERO_AMOUNT),
        server_spent: fallback("server_spent", ZERO_AMOUNT),
        session_protocol: fallback("session_protocol", LEGACY_PROTOCOL),
        descriptor_json: text(row, "descriptor_json"),
        entry_json: text(row, "entry_json"),
        server_spent_cumulative: text(row, "server_spent_cumulative"),
    })
}

fn migrate_scope_keys(rows: &mut [Row]) {
    let migratable = |row: &Row| {
        !row.origin.is_empty()
            && row.session_protocol == TIP1034_PROTOCOL
            && row.descriptor_json.is_some()
    };
    let mut best: BTreeMap<(String, String, String, String, i64), usize> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate().filter(|(_, row)| migratable(row)) {
        let partition = (
            row.origin.clone(),
            row.payee.to_lowercase(),
            row.token.to_lowercase(),
            row.escrow_contract.to_lowercase(),
            row.chain_id,
        );
        let rank = |row: &Row| {
            (
                row.scope_key.is_some(),
                row.state == ACTIVE,
                row.last_used_at,
                row.created_at,
                row.channel_id.clone(),
            )
        };
        match best.get(&partition) {
            Some(current) if rank(&rows[*current]) >= rank(row) => {}
            _ => {
                best.insert(partition, index);
            }
        }
    }
    for ((origin, payee, token, escrow, chain_id), index) in best {
        if rows[index].scope_key.is_some() {
            continue;
        }
        let scope_key = format!("{origin}\n{payee}:{token}:{escrow}:{chain_id}");
        if rows
            .iter()
            .all(|row| row.scope_key.as_deref() != Some(scope_key.as_str()))
        {
            rows[index].scope_key = Some(scope_key);
        }
    }
}

fn import_database(database: &Path) -> Result<Vec<Row>, StoreFault> {
    let unreadable = |error: String| {
        StoreFault::Io(format!(
            "reading the sessions in {}: {error}; move it aside to start without them",
            database.display()
        ))
    };
    let Some(records) = sqlite::read_table(database, CHANNELS_TABLE).map_err(unreadable)? else {
        return Ok(Vec::new());
    };
    let mut rows = records
        .iter()
        .map(imported_row)
        .collect::<Result<Vec<_>, _>>()?;
    migrate_scope_keys(&mut rows);
    rows.retain(|row| row.scope_key.is_some());
    Ok(rows)
}

impl FileChannelStore {
    pub(crate) fn open(options: FileChannelStoreOptions) -> Result<Self, StoreFault> {
        let database = match options.path {
            Some(path) => path,
            None => default_channel_database_path().map_err(|error| match error {
                Fault::Io(message) => StoreFault::Io(message),
                other => StoreFault::Io(other.to_string()),
            })?,
        };
        if let Some(parent) = database.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let document = database.with_extension(DOCUMENT_EXTENSION);
        let request_url = options
            .request_url
            .unwrap_or_else(|| options.namespace.clone());
        let origin = nanocode_http::Url::parse(&request_url)
            .map(|url| url.origin().ascii_serialization())
            .unwrap_or_else(|_| options.namespace.clone());
        let store = FileStore {
            namespace: options.namespace,
            origin,
            database,
            document,
            request_url,
        };
        store.import()?;
        Ok(Self {
            inner: Arc::new(store),
        })
    }

    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&FileStore) -> Result<T, StoreFault> + Send + 'static,
    ) -> Result<T, StoreFault> {
        let store = self.inner.clone();
        nanocode_platform::spawn_blocking(move || work(&store))
            .await
            .map_err(io_error)?
    }
}

impl FileStore {
    fn import(&self) -> Result<(), StoreFault> {
        if self.read()?.imported {
            return Ok(());
        }
        self.change(|document| {
            if document.imported {
                return Ok(());
            }
            let rows = match self.database.exists() {
                true => import_database(&self.database)?,
                false => Vec::new(),
            };
            for row in rows {
                let scope = row.scope_key.clone();
                let known = document.channels.iter().any(|current| {
                    current.channel_id == row.channel_id || current.scope_key == scope
                });
                if !known {
                    document.channels.push(row);
                }
            }
            document.imported = true;
            Ok(())
        })
    }
}

#[derive(Clone)]
pub(crate) enum ChannelStore {
    Memory(Arc<MemoryChannelStore>),
    File(FileChannelStore),
}

impl Default for ChannelStore {
    fn default() -> Self {
        Self::Memory(Arc::default())
    }
}

impl From<FileChannelStore> for ChannelStore {
    fn from(store: FileChannelStore) -> Self {
        Self::File(store)
    }
}

impl ChannelStore {
    pub(crate) async fn acquire(&self, key: &str) -> Result<StoreLease, StoreFault> {
        match self {
            Self::Memory(store) => store.acquire(key).await,
            Self::File(store) => {
                let path = store.inner.payment_lock_path(key);
                store
                    .blocking(move |_| {
                        lock_file(&path).map(|file| StoreLease::File { _file: file })
                    })
                    .await
            }
        }
    }

    pub(crate) async fn get(&self, key: &str) -> Result<Option<StoredChannelEntry>, StoreFault> {
        match self {
            Self::Memory(store) => Ok(store.get(key)),
            Self::File(store) => {
                let key = key.to_owned();
                store.blocking(move |store| store.get(&key)).await
            }
        }
    }

    pub(crate) async fn set(&self, entry: &StoredChannelEntry) -> Result<(), StoreFault> {
        match self {
            Self::Memory(store) => store.set(entry),
            Self::File(store) => {
                let entry = entry.clone();
                store.blocking(move |store| store.set(&entry)).await
            }
        }
    }

    pub(crate) async fn delete(&self, key: &str) -> Result<(), StoreFault> {
        match self {
            Self::Memory(store) => {
                store.delete(key);
                Ok(())
            }
            Self::File(store) => {
                let key = key.to_owned();
                store.blocking(move |store| store.delete(&key)).await
            }
        }
    }

    pub(crate) async fn server_spent(
        &self,
        key: &str,
        channel_id: &Hash,
    ) -> Result<Option<u128>, StoreFault> {
        match self {
            Self::Memory(store) => Ok(store.server_spent(key, channel_id)),
            Self::File(store) => {
                let (key, channel_id) = (key.to_owned(), *channel_id);
                store
                    .blocking(move |store| store.server_spent(&key, &channel_id))
                    .await
            }
        }
    }

    pub(crate) async fn set_server_spent(
        &self,
        key: &str,
        channel_id: &Hash,
        spent: u128,
    ) -> Result<(), StoreFault> {
        match self {
            Self::Memory(store) => {
                store.set_server_spent(key, channel_id, spent);
                Ok(())
            }
            Self::File(store) => {
                let (key, channel_id) = (key.to_owned(), *channel_id);
                store
                    .blocking(move |store| store.set_server_spent(&key, &channel_id, spent))
                    .await
            }
        }
    }
}
