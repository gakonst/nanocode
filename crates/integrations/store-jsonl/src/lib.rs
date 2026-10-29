#![cfg(not(target_family = "wasm"))]

mod compact;
mod index;

pub use compact::CompactionStats;

use nanocode_durability_core::{
    Committed, KnownSummary, ListedState, ListedSummary, OwnedCommit, OwnedState, OwnerId,
    OwnerToken, StateStore, StoreError, StoreFuture, StoreRecord, StoreReplacement, StoreUpdate,
    StoredState,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeStruct};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fs::{File, Metadata, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    ops::Range,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};

const JOB_CAPACITY: usize = 1;
const JOURNAL_READ_BUFFER_BYTES: usize = 128 * 1024;
const SEAL_POLL_FIRST: Duration = Duration::from_millis(1);
const SEAL_POLL_LONGEST: Duration = Duration::from_millis(100);
const SHARED_RECORD: char = '\u{1}';

type Job = Box<dyn FnOnce(&mut Journal) + Send>;

enum Entry {
    Acquire {
        state_id: String,
        owner: OwnerToken,
    },
    Replace {
        state_id: String,
        owner: OwnerToken,
        revision: u64,
        payload: String,
        records: Vec<StoreRecord>,
    },
}

impl Entry {
    fn claim(&self) -> Claim<'_> {
        match self {
            Self::Acquire { state_id, owner } => Claim {
                state_id,
                owner,
                revision: None,
            },
            Self::Replace {
                state_id,
                owner,
                revision,
                ..
            } => Claim {
                state_id,
                owner,
                revision: Some(*revision),
            },
        }
    }
}

fn replacement(entries: Vec<Entry>) -> Option<StoreReplacement> {
    entries.into_iter().find_map(|entry| match entry {
        Entry::Replace {
            payload, records, ..
        } => Some(StoreReplacement { payload, records }),
        Entry::Acquire { .. } => None,
    })
}

fn encode(
    entries: &[Entry],
    batch: Option<&Batch>,
    spans: &Spans,
) -> Result<(Vec<u8>, Vec<usize>), StoreError> {
    let mut bytes = Vec::new();
    let mut lengths = Vec::with_capacity(entries.len());
    for entry in entries {
        let start = bytes.len();
        let line = match entry {
            Entry::Acquire { state_id, owner } => Line::Acquire {
                state_id,
                owner,
                batch,
            },
            Entry::Replace {
                state_id,
                owner,
                revision,
                payload,
                records,
            } => Line::Replace {
                state_id,
                owner,
                revision: *revision,
                payload,
                records: Spanned {
                    value: records,
                    spans,
                },
                batch,
            },
        };
        serde_json::to_writer(
            Counted {
                bytes: &mut bytes,
                spans,
            },
            &line,
        )
        .map_err(backend)?;
        bytes.push(b'\n');
        lengths.push(bytes.len() - start);
    }
    Ok((bytes, lengths))
}

struct Replacement {
    state_id: String,
    owner: OwnerToken,
    expected: u64,
    payload: String,
    records: Vec<StoreRecord>,
}

enum Judged {
    Committed,
    Stale(Entry),
    Sealed(Entry),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Batch {
    id: String,
    count: usize,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Line<'a> {
    Acquire {
        state_id: &'a str,
        owner: &'a OwnerToken,
        #[serde(skip_serializing_if = "Option::is_none")]
        batch: Option<&'a Batch>,
    },
    Replace {
        state_id: &'a str,
        owner: &'a OwnerToken,
        revision: u64,
        payload: &'a str,
        records: Spanned<'a, [StoreRecord]>,
        #[serde(skip_serializing_if = "Option::is_none")]
        batch: Option<&'a Batch>,
    },
    Seal {
        seal: &'a str,
        pid: u32,
    },
    Unseal {
        seal: &'a str,
    },
}

struct Claim<'a> {
    state_id: &'a str,
    owner: &'a OwnerToken,
    revision: Option<u64>,
}

enum Verdict {
    Valid,
    Stale,
}

trait Owners {
    fn owner(&self, id: &str) -> Option<&OwnerToken>;
    fn revision(&self, id: &str) -> u64;
}

fn judge(view: &impl Owners, claims: &[Claim<'_>]) -> Result<Verdict, StoreError> {
    let mut owners: HashMap<&str, &OwnerToken> = HashMap::new();
    let mut revisions: HashMap<&str, u64> = HashMap::new();
    for claim in claims {
        let owner = owners
            .get(claim.state_id)
            .copied()
            .or_else(|| view.owner(claim.state_id));
        match claim.revision {
            None => {
                let next = owner
                    .map_or(Some(1), |owner| owner.fence().checked_add(1))
                    .ok_or_else(|| backend("JSONL owner fence overflow"))?;
                match claim.owner.fence().cmp(&next) {
                    Ordering::Equal => {
                        owners.insert(claim.state_id, claim.owner);
                    }
                    Ordering::Less => return Ok(Verdict::Stale),
                    Ordering::Greater => return Err(backend("invalid JSONL owner fence sequence")),
                }
            }
            Some(revision) => match owner {
                Some(current) if current == claim.owner => {
                    let next = revisions
                        .get(claim.state_id)
                        .copied()
                        .unwrap_or_else(|| view.revision(claim.state_id))
                        .checked_add(1)
                        .ok_or_else(|| backend("JSONL revision overflow"))?;
                    match revision.cmp(&next) {
                        Ordering::Equal => {
                            revisions.insert(claim.state_id, revision);
                        }
                        Ordering::Less => return Ok(Verdict::Stale),
                        Ordering::Greater => {
                            return Err(backend("invalid JSONL state revision or owner"));
                        }
                    }
                }
                Some(current) if claim.owner.fence() <= current.fence() => {
                    return Ok(Verdict::Stale);
                }
                _ => return Err(backend("invalid JSONL state revision or owner")),
            },
        }
    }
    Ok(Verdict::Valid)
}

#[derive(Clone)]
struct Sealed {
    id: String,
    pid: u32,
}

enum Outcome {
    Committed,
    Stale(Vec<Entry>),
    Sealed(Vec<Entry>),
}

enum Unit<'a> {
    Entries(Vec<(Location, Indexed<'a>, &'a [u8])>),
    Void,
}

fn scan(
    reader: &mut impl BufRead,
    mut position: u64,
    end: u64,
    interrupted_at_end: bool,
    mut each: impl FnMut(Unit<'_>, Location) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let mut line = Vec::new();
    let mut group: Vec<(Location, Vec<u8>)> = Vec::new();
    let mut open: Option<Batch> = None;
    while position < end {
        line.clear();
        reader.read_until(b'\n', &mut line).map_err(backend)?;
        if line.last() != Some(&b'\n') {
            break;
        }
        let location = Location {
            position,
            length: line.len(),
        };
        position = position
            .checked_add(line.len() as u64)
            .ok_or_else(|| backend("JSONL offset overflow"))?;
        if position > end {
            return Err(backend("JSONL entry crossed an appended boundary"));
        }
        let parsed = serde_json::from_slice::<Indexed>(&line);
        let batch = parsed
            .as_ref()
            .ok()
            .and_then(|entry| entry.batch.clone())
            .filter(|batch| batch.count > 1);
        if open.is_some() && open != batch {
            let (last, _) = group.pop().ok_or_else(|| backend("empty JSONL batch"))?;
            group.clear();
            open = None;
            each(Unit::Void, last)?;
        }
        match parsed {
            Err(error) => {
                tracing::warn!(
                    position = location.position,
                    %error,
                    "skipping an unparseable JSONL line"
                );
                each(Unit::Void, location)?;
            }
            Ok(entry) => match batch {
                Some(batch) => {
                    group.push((location, line.clone()));
                    let complete = group.len() == batch.count;
                    open = Some(batch);
                    if complete {
                        let lines = group
                            .iter()
                            .map(|(location, bytes)| {
                                serde_json::from_slice::<Indexed>(bytes)
                                    .map(|entry| (*location, entry, bytes.as_slice()))
                            })
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(backend)?;
                        each(Unit::Entries(lines), location)?;
                        group.clear();
                        open = None;
                    }
                }
                None => each(Unit::Entries(vec![(location, entry, &line)]), location)?,
            },
        }
    }
    if interrupted_at_end
        && position == end
        && let Some((last, _)) = group.pop()
    {
        each(Unit::Void, last)?;
    }
    Ok(())
}

fn claim<'a>(entry: &'a Indexed<'_>) -> Result<Claim<'a>, StoreError> {
    let state_id = entry
        .state_id
        .as_deref()
        .ok_or_else(|| backend("JSONL entry has no state"))?;
    let owner = entry
        .owner
        .as_ref()
        .ok_or_else(|| backend("JSONL entry has no owner"))?;
    match entry.kind {
        Kind::Acquire => Ok(Claim {
            state_id,
            owner,
            revision: None,
        }),
        Kind::Replace => Ok(Claim {
            state_id,
            owner,
            revision: Some(
                entry
                    .revision
                    .ok_or_else(|| backend("invalid JSONL replacement entry"))?,
            ),
        }),
        Kind::Seal | Kind::Unseal => Err(backend("JSONL seal inside an entry batch")),
    }
}

#[cfg(target_vendor = "apple")]
fn sync_journal(file: &File) -> std::io::Result<()> {
    nix::fcntl::fcntl(file, nix::fcntl::FcntlArg::F_BARRIERFSYNC)
        .map(drop)
        .or_else(|_| file.sync_all())
}

#[cfg(not(target_vendor = "apple"))]
fn sync_journal(file: &File) -> std::io::Result<()> {
    file.sync_all()
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    use nix::{errno::Errno, sys::signal::kill, unistd::Pid};
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    !matches!(kill(Pid::from_raw(pid), None), Err(Errno::ESRCH))
}

#[cfg(not(unix))]
const fn process_alive(_: u32) -> bool {
    true
}

#[derive(Default)]
struct Spans {
    written: Cell<usize>,
    values: RefCell<Vec<Range<usize>>>,
}

struct Spanned<'a, T: ?Sized> {
    value: &'a T,
    spans: &'a Spans,
}

impl Serialize for Spanned<'_, [StoreRecord]> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.value.iter().map(|value| Spanned {
            value,
            spans: self.spans,
        }))
    }
}

impl Serialize for Spanned<'_, StoreRecord> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut record = serializer.serialize_struct("StoreRecord", 2)?;
        record.serialize_field("key", &self.value.key)?;
        record.serialize_field(
            "value",
            &Spanned {
                value: self.value.value.as_str(),
                spans: self.spans,
            },
        )?;
        record.end()
    }
}

impl Serialize for Spanned<'_, str> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let start = self.spans.written.get();
        let written = serializer.serialize_str(self.value)?;
        self.spans
            .values
            .borrow_mut()
            .push(start..self.spans.written.get());
        Ok(written)
    }
}

struct Counted<'a> {
    bytes: &'a mut Vec<u8>,
    spans: &'a Spans,
}

impl Write for Counted<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        self.spans.written.set(self.bytes.len());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Acquire,
    Replace,
    Seal,
    Unseal,
}

#[derive(Deserialize)]
struct Records {
    records: Vec<StoreRecord>,
}

#[derive(Deserialize)]
struct Indexed<'a> {
    #[serde(rename = "type")]
    kind: Kind,
    #[serde(borrow)]
    state_id: Option<Cow<'a, str>>,
    owner: Option<OwnerToken>,
    revision: Option<u64>,
    payload: Option<Text>,
    #[serde(borrow)]
    records: Option<Vec<IndexedRecord<'a>>>,
    batch: Option<Batch>,
    #[serde(borrow)]
    seal: Option<Cow<'a, str>>,
    pid: Option<u32>,
}

#[derive(Deserialize, Serialize)]
struct IndexedRecord<'a> {
    #[serde(borrow)]
    key: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "raw_text")]
    value: &'a serde_json::value::RawValue,
}

struct Text;

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        raw_text(deserializer)?;
        Ok(Self)
    }
}

fn raw_text<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<&'de serde_json::value::RawValue, D::Error> {
    let raw = <&serde_json::value::RawValue>::deserialize(deserializer)?;
    if !raw.get().starts_with('"') {
        return Err(de::Error::custom("expected a string"));
    }
    if raw.get().contains("\\u") {
        let mut decoder = serde_json::Deserializer::from_str(raw.get());
        decoder
            .deserialize_str(TextVisitor)
            .map_err(de::Error::custom)?;
    }
    Ok(raw)
}

struct TextVisitor;

impl de::Visitor<'_> for TextVisitor {
    type Value = Text;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a string")
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<Text, E> {
        Ok(Text)
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq, Serialize, Deserialize)]
struct Location {
    position: u64,
    length: usize,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Record {
    Entry(Location),
    Value {
        location: Location,
        digest: [u8; 32],
    },
}

impl Record {
    fn value(id: &str, key: &str, position: u64, bytes: &[u8]) -> Self {
        Self::Value {
            location: Location {
                position,
                length: bytes.len(),
            },
            digest: value_digest(id, key, bytes),
        }
    }

    fn located(
        id: &str,
        key: &str,
        line: &[u8],
        position: u64,
        value: &serde_json::value::RawValue,
    ) -> Self {
        let start = value.get().as_ptr().addr() - line.as_ptr().addr();
        Self::value(id, key, position + start as u64, value.get().as_bytes())
    }

    const fn location(self) -> Location {
        match self {
            Self::Entry(location) | Self::Value { location, .. } => location,
        }
    }
}

fn value_digest(id: &str, key: &str, bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    for part in [id.as_bytes(), key.as_bytes()] {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part);
    }
    digest.update(bytes);
    digest.finalize().into()
}

struct State {
    revision: u64,
    location: Location,
    records: HashMap<String, Record>,
}

#[derive(Deserialize)]
struct StoredEntry<'a> {
    #[serde(rename = "type")]
    kind: Kind,
    #[serde(borrow)]
    state_id: Cow<'a, str>,
    #[serde(rename = "owner")]
    _owner: OwnerToken,
    revision: u64,
    #[serde(borrow, deserialize_with = "raw_text")]
    payload: &'a serde_json::value::RawValue,
    #[serde(borrow)]
    records: Vec<StoredRecord<'a>>,
}

#[derive(Deserialize)]
struct StoredRecord<'a> {
    #[serde(borrow)]
    key: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "raw_text")]
    value: &'a serde_json::value::RawValue,
}

struct CachedEntry {
    location: Location,
    bytes: Vec<u8>,
    state_id: String,
    revision: u64,
    payload: Range<usize>,
    records: HashMap<String, Range<usize>>,
}

impl CachedEntry {
    fn decode(location: Location, bytes: Vec<u8>) -> Result<Self, StoreError> {
        let entry: StoredEntry = serde_json::from_slice(&bytes).map_err(backend)?;
        if !matches!(entry.kind, Kind::Replace) {
            return Err(backend("JSONL index does not reference a replacement"));
        }
        let range = |value: &serde_json::value::RawValue| {
            let start = value.get().as_ptr().addr() - bytes.as_ptr().addr();
            start..start + value.get().len()
        };
        let payload = range(entry.payload);
        let mut records = HashMap::with_capacity(entry.records.len());
        for record in entry.records {
            records
                .entry(record.key.into_owned())
                .or_insert_with(|| range(record.value));
        }
        Ok(Self {
            location,
            state_id: entry.state_id.into_owned(),
            revision: entry.revision,
            payload,
            records,
            bytes,
        })
    }

    fn text(&self, range: Range<usize>) -> Result<String, StoreError> {
        serde_json::from_slice(&self.bytes[range]).map_err(backend)
    }
}

struct Journal {
    file: File,
    reads: File,
    path: PathBuf,
    lineage: String,
    index: index::Index,
    persisted_offset: Option<u64>,
    ready: bool,
    invalid_index: Option<String>,
    cached: Option<CachedEntry>,
    sealed: Option<Sealed>,
}

impl Owners for index::Index {
    fn owner(&self, id: &str) -> Option<&OwnerToken> {
        self.owners.get(id)
    }

    fn revision(&self, id: &str) -> u64 {
        self.states.get(id).map_or(0, |state| state.revision)
    }
}

impl Journal {
    fn open(path: &Path) -> Result<Self, StoreError> {
        let _span = tracing::info_span!("durability.jsonl.open").entered();
        let (file, path, mut reads, source) = loop {
            let file = OpenOptions::new()
                .create(true)
                .read(true)
                .append(true)
                .open(path)
                .map_err(backend)?;
            let canonical = std::fs::canonicalize(path).map_err(backend)?;
            let reads = File::open(&canonical).map_err(backend)?;
            let source = identity(&file.metadata().map_err(backend)?)?;
            if identity(&reads.metadata().map_err(backend)?)? == source {
                break (file, canonical, reads, source);
            }
        };
        let lineage = compact::lineage(&mut reads, &source)?;
        let mut store = Self {
            file,
            reads,
            path,
            lineage,
            index: index::Index::new(source),
            persisted_offset: None,
            ready: false,
            invalid_index: None,
            cached: None,
            sealed: None,
        };
        store.restore_index();
        store.sync()?;
        store.clear_temporaries().map_err(backend)?;
        if store.persisted_offset.is_none() {
            store.persist_index();
        }
        store.ready = true;
        Ok(store)
    }

    fn temporary(&self, suffix: &str) -> std::io::Result<tempfile::NamedTempFile> {
        let directory = self
            .path
            .parent()
            .ok_or_else(|| std::io::Error::other("journal has no directory"))?;
        let mut prefix = self
            .path
            .file_name()
            .ok_or_else(|| std::io::Error::other("journal has no filename"))?
            .to_os_string();
        prefix.push(suffix);
        prefix.push(format!("{}-", std::process::id()));
        tempfile::Builder::new()
            .prefix(&prefix)
            .tempfile_in(directory)
    }

    fn clear_temporaries(&self) -> std::io::Result<()> {
        let directory = self
            .path
            .parent()
            .ok_or_else(|| std::io::Error::other("journal has no directory"))?;
        let name = self
            .path
            .file_name()
            .ok_or_else(|| std::io::Error::other("journal has no filename"))?;
        let prefixes: Vec<_> = [".compact-", ".index-"]
            .into_iter()
            .map(|suffix| {
                let mut prefix = name.to_os_string();
                prefix.push(suffix);
                prefix
            })
            .collect();
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let abandoned = prefixes.iter().any(|prefix| {
                name.as_encoded_bytes()
                    .strip_prefix(prefix.as_encoded_bytes())
                    .and_then(|rest| std::str::from_utf8(rest).ok())
                    .and_then(|rest| rest.split_once('-'))
                    .and_then(|(pid, _)| pid.parse::<u32>().ok())
                    .is_some_and(|pid| !process_alive(pid))
            });
            if abandoned && entry.file_type()?.is_file() {
                match std::fs::remove_file(entry.path()) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
    }

    fn home(home: &Path) -> Result<PathBuf, StoreError> {
        std::fs::create_dir_all(home).map_err(backend)?;
        Ok(Self::home_path(home))
    }

    fn home_path(home: &Path) -> PathBuf {
        home.join("sessions.jsonl")
    }

    fn sync(&mut self) -> Result<(), StoreError> {
        loop {
            let current = identity(&std::fs::metadata(&self.path).map_err(backend)?)?;
            if current == self.index.source {
                break;
            }
            self.reopen_compacted()?;
        }
        self.adopt_saved_index();
        self.refresh_index()
    }

    fn writable(&mut self) -> Result<(), StoreError> {
        let mut delay = SEAL_POLL_FIRST;
        loop {
            self.sync()?;
            let Some(sealed) = self.sealed.clone() else {
                return Ok(());
            };
            if !process_alive(sealed.pid) {
                tracing::warn!(
                    seal = sealed.id,
                    pid = sealed.pid,
                    "releasing the seal of an exited JSONL compactor"
                );
                self.append_marker(&Line::Unseal { seal: &sealed.id })?;
                continue;
            }
            thread::sleep(delay);
            delay = delay.saturating_mul(2).min(SEAL_POLL_LONGEST);
        }
    }

    fn refresh(&mut self) -> Result<(), StoreError> {
        self.consume(None)
    }

    fn consume(&mut self, until: Option<u64>) -> Result<(), StoreError> {
        let length = self.reads.metadata().map_err(backend)?.len();
        if length < self.index.offset {
            return Err(backend(
                "JSONL durability log was truncated before its committed offset",
            ));
        }
        let end = until.unwrap_or(length);
        if end > length {
            return Err(backend("JSONL append position is beyond the journal"));
        }
        if end <= self.index.offset {
            return Ok(());
        }
        let _span = tracing::info_span!(
            "durability.jsonl.recover",
            log.unindexed_bytes = end - self.index.offset
        )
        .entered();
        let mut reader = BufReader::with_capacity(
            JOURNAL_READ_BUFFER_BYTES,
            self.reads.try_clone().map_err(backend)?,
        );
        reader
            .seek(SeekFrom::Start(self.index.offset))
            .map_err(backend)?;
        let start = self.index.offset;
        scan(&mut reader, start, end, until.is_some(), |unit, last| {
            if let Unit::Entries(lines) = unit {
                self.settle(&lines)?;
            }
            self.index.last = Some(last);
            self.index.offset = last
                .position
                .checked_add(last.length as u64)
                .ok_or_else(|| backend("JSONL offset overflow"))?;
            Ok(())
        })
    }

    fn settle(&mut self, lines: &[(Location, Indexed<'_>, &[u8])]) -> Result<(), StoreError> {
        if let [(_, entry, _)] = lines
            && matches!(entry.kind, Kind::Seal | Kind::Unseal)
        {
            let seal = entry
                .seal
                .as_deref()
                .ok_or_else(|| backend("invalid JSONL seal entry"))?;
            match (entry.kind, &self.sealed) {
                (Kind::Seal, None) => {
                    self.sealed = Some(Sealed {
                        id: seal.to_owned(),
                        pid: entry
                            .pid
                            .ok_or_else(|| backend("invalid JSONL seal entry"))?,
                    });
                }
                (Kind::Unseal, Some(sealed)) if sealed.id == seal => self.sealed = None,
                _ => {}
            }
            return Ok(());
        }
        if self.sealed.is_some() {
            return Ok(());
        }
        let claims = lines
            .iter()
            .map(|(_, entry, _)| claim(entry))
            .collect::<Result<Vec<_>, _>>()?;
        if matches!(judge(&self.index, &claims)?, Verdict::Stale) {
            tracing::debug!(
                position = lines.first().map(|(location, _, _)| location.position),
                "skipping a JSONL entry superseded by a concurrent writer"
            );
            return Ok(());
        }
        for (location, entry, line) in lines {
            self.replay(*location, entry, line)?;
        }
        Ok(())
    }

    fn replay(
        &mut self,
        location: Location,
        indexed: &Indexed<'_>,
        line: &[u8],
    ) -> Result<(), StoreError> {
        let id = indexed
            .state_id
            .as_deref()
            .ok_or_else(|| backend("JSONL entry has no state"))?;
        let owner = indexed
            .owner
            .clone()
            .ok_or_else(|| backend("JSONL entry has no owner"))?;
        match indexed.kind {
            Kind::Acquire => self.acquired(id, owner),
            Kind::Replace => {
                let (Some(revision), Some(Text), Some(records)) =
                    (indexed.revision, &indexed.payload, &indexed.records)
                else {
                    return Err(backend("invalid JSONL replacement entry"));
                };
                self.check_replacement(id, &owner, revision)?;
                if !records.is_empty()
                    && let Err(error) = self.index.load_records(id)
                {
                    self.invalid_index = Some(error.to_string());
                    return Err(backend(error));
                }
                let known = self.index.states.get(id).map(|state| &state.records);
                let mut keys = HashSet::with_capacity(records.len());
                let repeated = records.iter().any(|record| {
                    !keys.insert(&*record.key)
                        || known.is_some_and(|known| known.contains_key(&*record.key))
                });
                if repeated {
                    let Records { records } = serde_json::from_slice(line).map_err(backend)?;
                    self.validate_records(id, &records)?;
                }
                self.replaced(
                    id,
                    revision,
                    location,
                    records.iter().map(|record| {
                        (
                            &*record.key,
                            Record::located(id, &record.key, line, location.position, record.value),
                        )
                    }),
                );
                Ok(())
            }
            Kind::Seal | Kind::Unseal => Err(backend("JSONL seal inside an entry batch")),
        }
    }

    fn next_fence(&self, id: &str) -> Option<u64> {
        self.index
            .owners
            .get(id)
            .map_or(Some(1), |owner| owner.fence().checked_add(1))
    }

    fn revision(&self, id: &str) -> u64 {
        self.index.states.get(id).map_or(0, |state| state.revision)
    }

    fn acquired(&mut self, id: &str, owner: OwnerToken) -> Result<(), StoreError> {
        if self.next_fence(id) != Some(owner.fence()) {
            return Err(backend("invalid JSONL owner fence sequence"));
        }
        match self.index.owners.get_mut(id) {
            Some(current) => *current = owner,
            None => {
                self.index.owners.insert(id.into(), owner);
            }
        }
        Ok(())
    }

    fn check_replacement(
        &self,
        id: &str,
        owner: &OwnerToken,
        revision: u64,
    ) -> Result<(), StoreError> {
        if self.index.owners.get(id) != Some(owner)
            || self.revision(id).checked_add(1) != Some(revision)
        {
            return Err(backend("invalid JSONL state revision or owner"));
        }
        Ok(())
    }

    fn replaced<'k>(
        &mut self,
        id: &str,
        revision: u64,
        location: Location,
        records: impl Iterator<Item = (&'k str, Record)>,
    ) {
        if !self.index.states.contains_key(id) {
            self.index.created.push(id.into());
            self.index.states.insert(
                id.into(),
                State {
                    revision,
                    location,
                    records: HashMap::new(),
                },
            );
        }
        let state = self
            .index
            .states
            .get_mut(id)
            .expect("a replaced state is indexed");
        state.revision = revision;
        state.location = location;
        for (key, record) in records {
            if !state.records.contains_key(key) {
                state.records.insert(key.into(), record);
            }
        }
    }

    fn entry(&mut self, location: Location) -> Result<&CachedEntry, StoreError> {
        if self
            .cached
            .as_ref()
            .is_none_or(|cached| cached.location != location)
        {
            self.reads
                .seek(SeekFrom::Start(location.position))
                .map_err(backend)?;
            let mut bytes = vec![0; location.length];
            self.reads.read_exact(&mut bytes).map_err(backend)?;
            self.cached = Some(CachedEntry::decode(location, bytes)?);
        }
        Ok(self.cached.as_ref().expect("the entry was loaded"))
    }

    fn state(&mut self, id: &str) -> Result<StoredState, StoreError> {
        let Some(location) = self.index.states.get(id).map(|state| state.location) else {
            return Ok(StoredState::default());
        };
        let entry = self.entry(location)?;
        if entry.state_id != id {
            return Err(backend("JSONL state index does not reference its state"));
        }
        Ok(StoredState {
            revision: entry.revision,
            payload: Some(entry.text(entry.payload.clone())?),
        })
    }

    fn invalidated(&mut self, error: impl std::fmt::Display) -> StoreError {
        let error = error.to_string();
        self.invalid_index = Some(error.clone());
        backend(error)
    }

    fn rebuilt<T>(
        &mut self,
        mut action: impl FnMut(&mut Self) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let result = action(self);
        let Some(error) = self.invalid_index.take() else {
            return result;
        };
        self.discard_index(error);
        self.refresh_index()?;
        self.persist_index();
        action(self)
    }

    fn location(&mut self, id: &str, key: &str) -> Result<Option<Record>, StoreError> {
        if let Err(error) = self.index.load_records(id) {
            return Err(self.invalidated(error));
        }
        Ok(self
            .index
            .states
            .get(id)
            .and_then(|state| state.records.get(key))
            .copied())
    }

    fn record_keys(&mut self, id: &str) -> Result<Vec<String>, StoreError> {
        if let Err(error) = self.index.load_records(id) {
            return Err(self.invalidated(error));
        }
        Ok(self
            .index
            .states
            .get(id)
            .map(|state| state.records.keys().cloned().collect())
            .unwrap_or_default())
    }

    fn record(&mut self, id: &str, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self.resolved(id, key)?.map(|(_, value)| value))
    }

    fn resolved(&mut self, id: &str, key: &str) -> Result<Option<(String, String)>, StoreError> {
        let Some(stored) = self.stored(id, key)? else {
            return Ok(None);
        };
        let Some(marked) = stored.strip_prefix(SHARED_RECORD) else {
            return Ok(Some((id.to_owned(), stored)));
        };
        if marked.starts_with(SHARED_RECORD) {
            return Ok(Some((id.to_owned(), marked.to_owned())));
        }
        let holder = marked.to_owned();
        match self.stored(&holder, key)? {
            Some(value) if !value.starts_with(SHARED_RECORD) => Ok(Some((holder, value))),
            Some(value) if value[SHARED_RECORD.len_utf8()..].starts_with(SHARED_RECORD) => {
                Ok(Some((holder, value[SHARED_RECORD.len_utf8()..].to_owned())))
            }
            _ => Err(self.invalidated(format!(
                "JSONL record {key} of {id} names {holder}, which does not hold it"
            ))),
        }
    }

    fn stored(&mut self, id: &str, key: &str) -> Result<Option<String>, StoreError> {
        match self.location(id, key)? {
            None => Ok(None),
            Some(Record::Value { location, digest }) => {
                self.value(id, key, location, digest).map(Some)
            }
            Some(Record::Entry(location)) => self.entry_record(id, key, location).map(Some),
        }
    }

    fn share(
        &mut self,
        id: &str,
        records: Vec<StoreRecord>,
        fresh: Vec<bool>,
    ) -> Result<Vec<StoreRecord>, StoreError> {
        let mut records = records
            .into_iter()
            .zip(fresh)
            .filter_map(|(record, fresh)| fresh.then_some((record, None::<String>)))
            .collect::<Vec<_>>();
        let mut budget = records
            .iter()
            .map(|(record, _)| record.value.len())
            .sum::<usize>();
        let candidates = self
            .index
            .created
            .iter()
            .rev()
            .filter(|candidate| candidate.as_str() != id)
            .cloned()
            .collect::<Vec<_>>();
        for candidate in candidates {
            let cost = self.index.deferred_bytes(&candidate);
            if cost > budget {
                break;
            }
            budget -= cost;
            for (record, holder) in &mut records {
                if holder.is_some()
                    || record.value.len() <= SHARED_RECORD.len_utf8() + candidate.len()
                    || self.location(&candidate, &record.key)?.is_none()
                {
                    continue;
                }
                if let Some((found, value)) = self.resolved(&candidate, &record.key)?
                    && value == record.value
                    && record.value.len() > SHARED_RECORD.len_utf8() + found.len()
                {
                    budget = budget.saturating_sub(record.value.len());
                    *holder = Some(found);
                }
            }
            if records.iter().all(|(_, holder)| holder.is_some()) {
                break;
            }
        }
        Ok(records
            .into_iter()
            .map(|(record, holder)| match holder {
                Some(holder) => StoreRecord {
                    key: record.key,
                    value: format!("{SHARED_RECORD}{holder}"),
                },
                None if record.value.starts_with(SHARED_RECORD) => StoreRecord {
                    key: record.key,
                    value: format!("{SHARED_RECORD}{}", record.value),
                },
                None => record,
            })
            .collect())
    }

    fn value(
        &mut self,
        id: &str,
        key: &str,
        location: Location,
        digest: [u8; 32],
    ) -> Result<String, StoreError> {
        let mut bytes = vec![0; location.length];
        if let Err(error) = self
            .reads
            .seek(SeekFrom::Start(location.position))
            .and_then(|_| self.reads.read_exact(&mut bytes))
        {
            return Err(self.invalidated(error));
        }
        if value_digest(id, key, &bytes) != digest {
            return Err(self.invalidated("JSONL record index does not match its journal"));
        }
        serde_json::from_slice(&bytes).map_err(|error| self.invalidated(error))
    }

    fn entry_record(
        &mut self,
        id: &str,
        key: &str,
        location: Location,
    ) -> Result<String, StoreError> {
        if let Err(error) = self.entry(location).map(|_| ()) {
            return Err(self.invalidated(error));
        }
        let entry = self.cached.as_ref().expect("the entry was loaded");
        let Some(range) = entry
            .records
            .get(key)
            .filter(|_| entry.state_id == id)
            .cloned()
        else {
            return Err(self.invalidated("JSONL record index does not reference its record"));
        };
        let value = match entry.text(range) {
            Ok(value) => value,
            Err(error) => return Err(self.invalidated(error)),
        };
        let state = self
            .index
            .states
            .get_mut(id)
            .expect("an indexed record has a state");
        if state.records.get(key) == Some(&Record::Entry(location)) {
            for (key, range) in &entry.records {
                if let Some(record) = state.records.get_mut(key)
                    && *record == Record::Entry(location)
                {
                    *record = Record::value(
                        id,
                        key,
                        location.position + range.start as u64,
                        &entry.bytes[range.clone()],
                    );
                }
            }
            self.index.migrated = true;
        }
        if state.location != location {
            self.cached = None;
        }
        Ok(value)
    }

    fn validate_records(
        &mut self,
        id: &str,
        records: &[StoreRecord],
    ) -> Result<Vec<bool>, StoreError> {
        let mut staged = HashMap::new();
        let mut fresh = Vec::with_capacity(records.len());
        let (mut encoded, mut stored) = (Vec::new(), Vec::new());
        for record in records {
            let retained = match self.matches(id, record, &mut encoded, &mut stored)? {
                Some(equal) => Some(equal),
                None => staged
                    .get(record.key.as_str())
                    .map(|value| *value == record.value),
            };
            if retained == Some(false) {
                return Err(StoreError::NotCommitted(format!(
                    "durability record `{}` has conflicting content",
                    record.key
                )));
            }
            fresh.push(retained.is_none());
            staged.insert(record.key.as_str(), record.value.as_str());
        }
        Ok(fresh)
    }

    fn matches(
        &mut self,
        id: &str,
        record: &StoreRecord,
        encoded: &mut Vec<u8>,
        stored: &mut Vec<u8>,
    ) -> Result<Option<bool>, StoreError> {
        let Some(indexed) = self.location(id, &record.key)? else {
            return Ok(None);
        };
        if let Record::Value { location, digest } = indexed {
            encoded.clear();
            serde_json::to_writer(&mut *encoded, &record.value).map_err(backend)?;
            if encoded.len() == location.length {
                stored.resize(location.length, 0);
                if let Err(error) = self
                    .reads
                    .seek(SeekFrom::Start(location.position))
                    .and_then(|_| self.reads.read_exact(stored))
                {
                    return Err(self.invalidated(error));
                }
                if stored == encoded && value_digest(id, &record.key, stored) == digest {
                    return Ok(Some(true));
                }
            }
        }
        Ok(Some(
            self.record(id, &record.key)?
                .is_some_and(|value| value == record.value),
        ))
    }

    fn append_raw(&mut self, bytes: &[u8]) -> Result<u64, StoreError> {
        let length = self.file.metadata().map_err(backend)?.len();
        let separated = length == 0 || {
            let mut last = [0];
            self.reads
                .seek(SeekFrom::Start(length - 1))
                .and_then(|_| self.reads.read_exact(&mut last))
                .map_err(backend)?;
            last[0] == b'\n'
        };
        let framed;
        let data = if separated {
            bytes
        } else {
            let mut separated = Vec::with_capacity(bytes.len() + 1);
            separated.push(b'\n');
            separated.extend_from_slice(bytes);
            framed = separated;
            framed.as_slice()
        };
        let written = self.file.write(data).map_err(backend)?;
        if written != data.len() {
            return Err(backend("JSONL append wrote only part of its entries"));
        }
        let end = self.file.stream_position().map_err(backend)?;
        let started = Instant::now();
        sync_journal(&self.file).map_err(backend)?;
        tracing::Span::current().record(
            "fsync.duration_ms",
            started.elapsed().as_secs_f64() * 1_000.0,
        );
        end.checked_sub(bytes.len() as u64)
            .ok_or_else(|| backend("JSONL append position underflow"))
    }

    fn append_marker(&mut self, line: &Line<'_>) -> Result<(), StoreError> {
        let mut bytes = serde_json::to_vec(line).map_err(backend)?;
        bytes.push(b'\n');
        let start = self.append_raw(&bytes)?;
        self.consume(Some(start))?;
        self.consume(None)
    }

    fn seal(&mut self, seal: &str) -> Result<Option<u64>, StoreError> {
        let mut bytes = serde_json::to_vec(&Line::Seal {
            seal,
            pid: std::process::id(),
        })
        .map_err(backend)?;
        bytes.push(b'\n');
        let start = self.append_raw(&bytes)?;
        self.consume(Some(start))?;
        if self.index.offset != start {
            return Err(backend("JSONL journal did not reach its appended seal"));
        }
        let won = self.sealed.is_none();
        self.consume(None)?;
        Ok(won.then_some(start))
    }

    fn commit(&mut self, entries: Vec<Entry>) -> Result<Outcome, StoreError> {
        let span = tracing::info_span!(
            "durability.jsonl.append",
            entry.count = entries.len(),
            log.bytes = tracing::field::Empty,
            encode.duration_ms = tracing::field::Empty,
            fsync.duration_ms = tracing::field::Empty,
            index.duration_ms = tracing::field::Empty,
            outcome = tracing::field::Empty
        );
        let _entered = span.enter();
        let started = Instant::now();
        let batch = (entries.len() > 1).then(|| Batch {
            id: OwnerId::new().as_str().to_owned(),
            count: entries.len(),
        });
        let spans = Spans::default();
        let (bytes, lengths) = encode(&entries, batch.as_ref(), &spans)?;
        span.record("log.bytes", bytes.len());
        span.record(
            "encode.duration_ms",
            started.elapsed().as_secs_f64() * 1_000.0,
        );
        let origin = self.append_raw(&bytes)?;
        self.consume(Some(origin))?;
        if self.index.offset != origin {
            return Err(backend("JSONL journal did not reach its appended entries"));
        }
        let started = Instant::now();
        let void = if self.sealed.is_some() {
            Some(true)
        } else {
            let claims: Vec<_> = entries.iter().map(Entry::claim).collect();
            matches!(judge(&self.index, &claims)?, Verdict::Stale).then_some(false)
        };
        if let Some(sealed) = void {
            let length = *lengths
                .last()
                .ok_or_else(|| backend("empty JSONL append"))?;
            let end = origin
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| backend("JSONL offset overflow"))?;
            self.index.last = Some(Location {
                position: end - length as u64,
                length,
            });
            self.index.offset = end;
            span.record("outcome", if sealed { "sealed" } else { "superseded" });
            return Ok(if sealed {
                Outcome::Sealed(entries)
            } else {
                Outcome::Stale(entries)
            });
        }
        let mut values = spans.values.into_inner().into_iter();
        for (entry, length) in entries.into_iter().zip(lengths) {
            let location = Location {
                position: self.index.offset,
                length,
            };
            match entry {
                Entry::Acquire { state_id, owner } => self.acquired(&state_id, owner)?,
                Entry::Replace {
                    state_id,
                    owner,
                    revision,
                    records,
                    ..
                } => {
                    self.check_replacement(&state_id, &owner, revision)?;
                    self.replaced(
                        &state_id,
                        revision,
                        location,
                        records.iter().zip(values.by_ref()).map(|(record, span)| {
                            (
                                record.key.as_str(),
                                Record::value(
                                    &state_id,
                                    &record.key,
                                    origin + span.start as u64,
                                    &bytes[span],
                                ),
                            )
                        }),
                    );
                }
            }
            self.index.last = Some(location);
            self.index.offset = self
                .index
                .offset
                .checked_add(length as u64)
                .ok_or_else(|| backend("JSONL offset overflow"))?;
        }
        span.record(
            "index.duration_ms",
            started.elapsed().as_secs_f64() * 1_000.0,
        );
        span.record("outcome", "committed");
        Ok(Outcome::Committed)
    }

    fn commit_each(&mut self, entries: Vec<Entry>) -> Result<Vec<Judged>, StoreError> {
        let span = tracing::info_span!(
            "durability.jsonl.append",
            entry.count = entries.len(),
            log.bytes = tracing::field::Empty,
            encode.duration_ms = tracing::field::Empty,
            fsync.duration_ms = tracing::field::Empty,
            index.duration_ms = tracing::field::Empty,
            outcome = tracing::field::Empty
        );
        let _entered = span.enter();
        let started = Instant::now();
        let spans = Spans::default();
        let (bytes, lengths) = encode(&entries, None, &spans)?;
        span.record("log.bytes", bytes.len());
        span.record(
            "encode.duration_ms",
            started.elapsed().as_secs_f64() * 1_000.0,
        );
        let origin = self.append_raw(&bytes)?;
        self.consume(Some(origin))?;
        if self.index.offset != origin {
            return Err(backend("JSONL journal did not reach its appended entries"));
        }
        let started = Instant::now();
        let sealed = self.sealed.is_some();
        let mut values = spans.values.into_inner().into_iter();
        let mut judged = Vec::with_capacity(entries.len());
        for (entry, length) in entries.into_iter().zip(lengths) {
            let location = Location {
                position: self.index.offset,
                length,
            };
            let spans = match &entry {
                Entry::Acquire { .. } => Vec::new(),
                Entry::Replace { records, .. } => values.by_ref().take(records.len()).collect(),
            };
            let verdict = if sealed {
                Verdict::Stale
            } else {
                judge(&self.index, &[entry.claim()])?
            };
            judged.push(match (verdict, sealed) {
                (Verdict::Stale, true) => Judged::Sealed(entry),
                (Verdict::Stale, false) => Judged::Stale(entry),
                (Verdict::Valid, _) => {
                    match entry {
                        Entry::Acquire { state_id, owner } => self.acquired(&state_id, owner)?,
                        Entry::Replace {
                            state_id,
                            owner,
                            revision,
                            records,
                            ..
                        } => {
                            self.check_replacement(&state_id, &owner, revision)?;
                            self.replaced(
                                &state_id,
                                revision,
                                location,
                                records.iter().zip(spans).map(|(record, span)| {
                                    (
                                        record.key.as_str(),
                                        Record::value(
                                            &state_id,
                                            &record.key,
                                            origin + span.start as u64,
                                            &bytes[span],
                                        ),
                                    )
                                }),
                            );
                        }
                    }
                    Judged::Committed
                }
            });
            self.index.last = Some(location);
            self.index.offset = self
                .index
                .offset
                .checked_add(length as u64)
                .ok_or_else(|| backend("JSONL offset overflow"))?;
        }
        span.record(
            "index.duration_ms",
            started.elapsed().as_secs_f64() * 1_000.0,
        );
        span.record("outcome", if sealed { "sealed" } else { "committed" });
        Ok(judged)
    }
}

impl Journal {
    fn acquire_batch(
        &mut self,
        id: &str,
        owner_id: OwnerId,
        update: StoreUpdate,
        companion: Option<(&str, OwnerId)>,
    ) -> Result<(OwnedState, Option<OwnedState>), StoreError> {
        if companion
            .as_ref()
            .is_some_and(|(companion_id, _)| *companion_id == id)
        {
            return Err(StoreError::NotCommitted(
                "a companion state must differ from its state".into(),
            ));
        }
        let mut update = Some(update);
        let mut planned: Option<(u64, Option<StoreReplacement>)> = None;
        loop {
            self.writable()?;
            let mut entries = Vec::new();
            let held = match &companion {
                Some((companion_id, companion_owner)) => {
                    let fence = self.next_fence(companion_id).ok_or_else(|| {
                        StoreError::NotCommitted("JSONL owner fence overflow".into())
                    })?;
                    let owner = OwnerToken::new(companion_owner.clone(), fence);
                    let state = self.state(companion_id)?;
                    entries.push(Entry::Acquire {
                        state_id: (*companion_id).into(),
                        owner: owner.clone(),
                    });
                    Some(OwnedState { owner, state })
                }
                None => None,
            };
            let fence = self
                .next_fence(id)
                .ok_or_else(|| StoreError::NotCommitted("JSONL owner fence overflow".into()))?;
            let owner = OwnerToken::new(owner_id.clone(), fence);
            let mut state = self.state(id)?;
            let current = state.revision;
            let replacement = match planned.take() {
                Some((revision, replacement)) if revision == current => replacement,
                Some(_) => {
                    return Err(StoreError::NotCommitted(
                        "JSONL state changed while its acquisition was retried".into(),
                    ));
                }
                None => update
                    .take()
                    .ok_or_else(|| backend("JSONL acquisition update already ran"))?(
                    &state
                )?,
            };
            entries.push(Entry::Acquire {
                state_id: id.into(),
                owner: owner.clone(),
            });
            if let Some(replacement) = replacement {
                let revision = state
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| StoreError::NotCommitted("JSONL revision overflow".into()))?;
                let fresh =
                    self.rebuilt(|store| store.validate_records(id, &replacement.records))?;
                if self.next_fence(id) != Some(fence)
                    || self.revision(id) != current
                    || held.as_ref().zip(companion.as_ref()).is_some_and(
                        |(held, (companion_id, _))| {
                            self.next_fence(companion_id) != Some(held.owner.fence())
                                || self.revision(companion_id) != held.state.revision
                        },
                    )
                {
                    planned = Some((current, Some(replacement)));
                    continue;
                }
                let records = self.share(id, replacement.records, fresh)?;
                state = StoredState {
                    revision,
                    payload: Some(replacement.payload.clone()),
                };
                entries.push(Entry::Replace {
                    state_id: id.into(),
                    owner: owner.clone(),
                    revision,
                    payload: replacement.payload,
                    records,
                });
            }
            match self.commit(entries)? {
                Outcome::Committed => return Ok((OwnedState { owner, state }, held)),
                Outcome::Stale(entries) | Outcome::Sealed(entries) => {
                    planned = Some((current, crate::replacement(entries)));
                }
            }
        }
    }

    fn read_record(&mut self, id: &str, key: &str) -> Result<Option<String>, StoreError> {
        self.sync()?;
        self.rebuilt(|store| store.record(id, key))
    }

    fn read_records(
        &mut self,
        id: &str,
        keys: &[String],
    ) -> Result<Vec<Option<String>>, StoreError> {
        self.sync()?;
        self.rebuilt(|store| keys.iter().map(|key| store.record(id, key)).collect())
    }

    fn peek(&mut self, id: &str) -> Result<StoredState, StoreError> {
        self.sync()?;
        self.state(id)
    }

    fn owner(&mut self, id: &str) -> Result<Option<OwnerToken>, StoreError> {
        self.sync()?;
        Ok(self.index.owners.get(id).cloned())
    }

    fn list_records(&mut self, id: &str) -> Result<Vec<String>, StoreError> {
        self.sync()?;
        self.rebuilt(|store| store.record_keys(id))
    }

    fn list_states(&mut self, limit: usize) -> Result<Vec<String>, StoreError> {
        self.sync()?;
        Ok(self
            .index
            .created
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect())
    }

    fn list_summaries(&mut self, limit: usize) -> Result<Vec<ListedState>, StoreError> {
        self.sync()?;
        let created = self
            .index
            .created
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let summaries = self.index.load_summaries().clone();
        Ok(created
            .into_iter()
            .map(|state_id| {
                let revision = self.revision(&state_id);
                let summary = match summaries.get(&state_id) {
                    Some((known, Some(summary))) if *known == revision => {
                        ListedSummary::Listed(summary.clone())
                    }
                    Some((known, None)) if *known == revision => ListedSummary::Unlisted,
                    _ => ListedSummary::Unknown,
                };
                ListedState {
                    state_id,
                    revision,
                    summary,
                }
            })
            .collect())
    }

    fn remember_summaries(&mut self, known: Vec<KnownSummary>) -> Result<(), StoreError> {
        self.sync()?;
        let revisions = known
            .iter()
            .map(|summary| self.revision(&summary.state_id))
            .collect::<Vec<_>>();
        let summaries = self.index.load_summaries();
        let mut changed = false;
        for (summary, revision) in known.into_iter().zip(revisions) {
            if summary.revision == revision && revision > 0 {
                summaries.insert(summary.state_id, (revision, summary.summary));
                changed = true;
            }
        }
        if changed {
            self.index.migrated = true;
        }
        Ok(())
    }

    fn acquire(&mut self, id: &str, owner_id: OwnerId) -> Result<OwnedState, StoreError> {
        loop {
            self.writable()?;
            let fence = self
                .next_fence(id)
                .ok_or_else(|| StoreError::NotCommitted("JSONL owner fence overflow".into()))?;
            let owner = OwnerToken::new(owner_id.clone(), fence);
            let entry = Entry::Acquire {
                state_id: id.into(),
                owner: owner.clone(),
            };
            if let Outcome::Committed = self.commit(vec![entry])? {
                return Ok(OwnedState {
                    owner,
                    state: self.state(id)?,
                });
            }
        }
    }

    fn replace(
        &mut self,
        id: &str,
        owner: &OwnerToken,
        expected: u64,
        payload: String,
        records: Vec<StoreRecord>,
    ) -> Result<u64, StoreError> {
        let mut replacement = StoreReplacement { payload, records };
        loop {
            self.writable()?;
            let (revision, fresh) = self.rebuilt(|store| {
                if store.index.owners.get(id) != Some(owner) {
                    return Err(StoreError::Fenced);
                }
                let actual = store.revision(id);
                if actual != expected {
                    return Err(StoreError::Conflict { expected, actual });
                }
                let revision = actual
                    .checked_add(1)
                    .ok_or_else(|| StoreError::NotCommitted("JSONL revision overflow".into()))?;
                let fresh = store.validate_records(id, &replacement.records)?;
                Ok((revision, fresh))
            })?;
            let records = self.share(id, replacement.records, fresh)?;
            let entry = Entry::Replace {
                state_id: id.into(),
                owner: owner.clone(),
                revision,
                payload: replacement.payload,
                records,
            };
            match self.commit(vec![entry])? {
                Outcome::Committed => return Ok(revision),
                Outcome::Sealed(entries) => {
                    replacement = crate::replacement(entries)
                        .ok_or_else(|| backend("JSONL replacement entry disappeared"))?;
                }
                Outcome::Stale(_) => {
                    return Err(if self.index.owners.get(id) == Some(owner) {
                        StoreError::Conflict {
                            expected,
                            actual: self.revision(id),
                        }
                    } else {
                        StoreError::Fenced
                    });
                }
            }
        }
    }

    fn replace_many(
        &mut self,
        replacements: Vec<Result<Replacement, StoreError>>,
    ) -> Vec<Result<u64, StoreError>> {
        let mut results: Vec<Option<Result<u64, StoreError>>> =
            Vec::with_capacity(replacements.len());
        let mut pending = Vec::new();
        for (slot, replacement) in replacements.into_iter().enumerate() {
            match replacement {
                Ok(replacement) => {
                    results.push(None);
                    pending.push((slot, replacement));
                }
                Err(error) => results.push(Some(Err(error))),
            }
        }
        while !pending.is_empty() {
            if let Err(error) = self.writable() {
                for (slot, _) in pending.drain(..) {
                    results[slot] = Some(Err(error.clone()));
                }
                break;
            }
            let mut entries = Vec::with_capacity(pending.len());
            let mut slots = Vec::with_capacity(pending.len());
            for (slot, replacement) in pending.drain(..) {
                let Replacement {
                    state_id,
                    owner,
                    expected,
                    payload,
                    records,
                } = replacement;
                let validated = self.rebuilt(|store| {
                    if store.index.owners.get(&state_id) != Some(&owner) {
                        return Err(StoreError::Fenced);
                    }
                    let actual = store.revision(&state_id);
                    if actual != expected {
                        return Err(StoreError::Conflict { expected, actual });
                    }
                    let revision = actual.checked_add(1).ok_or_else(|| {
                        StoreError::NotCommitted("JSONL revision overflow".into())
                    })?;
                    let fresh = store.validate_records(&state_id, &records)?;
                    Ok((revision, fresh))
                });
                match validated.and_then(|(revision, fresh)| {
                    Ok((revision, self.share(&state_id, records, fresh)?))
                }) {
                    Ok((revision, records)) => {
                        entries.push(Entry::Replace {
                            state_id,
                            owner,
                            revision,
                            payload,
                            records,
                        });
                        slots.push((slot, expected, revision));
                    }
                    Err(error) => results[slot] = Some(Err(error)),
                }
            }
            if entries.is_empty() {
                break;
            }
            let judged = match self.commit_each(entries) {
                Ok(judged) => judged,
                Err(error) => {
                    for (slot, _, _) in slots {
                        results[slot] = Some(Err(error.clone()));
                    }
                    break;
                }
            };
            for ((slot, expected, revision), judged) in slots.into_iter().zip(judged) {
                match judged {
                    Judged::Committed => results[slot] = Some(Ok(revision)),
                    Judged::Sealed(Entry::Replace {
                        state_id,
                        owner,
                        payload,
                        records,
                        ..
                    }) => pending.push((
                        slot,
                        Replacement {
                            state_id,
                            owner,
                            expected,
                            payload,
                            records,
                        },
                    )),
                    Judged::Stale(Entry::Replace {
                        state_id, owner, ..
                    }) => {
                        results[slot] =
                            Some(Err(if self.index.owners.get(&state_id) == Some(&owner) {
                                StoreError::Conflict {
                                    expected,
                                    actual: self.revision(&state_id),
                                }
                            } else {
                                StoreError::Fenced
                            }));
                    }
                    Judged::Sealed(Entry::Acquire { .. })
                    | Judged::Stale(Entry::Acquire { .. }) => {
                        results[slot] = Some(Err(backend("JSONL replacement entry disappeared")));
                    }
                }
            }
        }
        results
            .into_iter()
            .map(|result| {
                result.unwrap_or_else(|| Err(backend("JSONL replacement was not attempted")))
            })
            .collect()
    }
}
pub struct JsonlStore {
    jobs: mpsc::Sender<Job>,
    closed: oneshot::Receiver<()>,
}

impl JsonlStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let journal = Journal::open(path.as_ref())?;
        Self::start(move || Ok(journal)).map(|(store, _)| store)
    }

    pub fn open_home(home: &Path) -> Result<Self, StoreError> {
        Self::open(Journal::home(home)?)
    }

    pub async fn open_async(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_owned();
        Self::opened(Self::start(move || Journal::open(&path))?).await
    }

    pub async fn open_home_async(home: &Path) -> Result<Self, StoreError> {
        let home = home.to_owned();
        Self::opened(Self::start(move || Journal::open(&Journal::home(&home)?))?).await
    }

    pub async fn compact(&mut self) -> Result<CompactionStats, StoreError> {
        self.write(Journal::compact).await
    }

    pub async fn close(self) -> Result<(), StoreError> {
        let Self { jobs, closed } = self;
        drop(jobs);
        closed.await.map_err(|_| stopped())
    }

    fn start(
        open: impl FnOnce() -> Result<Journal, StoreError> + Send + 'static,
    ) -> Result<(Self, oneshot::Receiver<Result<(), StoreError>>), StoreError> {
        let (jobs, mut receiver) = mpsc::channel::<Job>(JOB_CAPACITY);
        let (finished, closed) = oneshot::channel();
        let (ready, opened) = oneshot::channel();
        let span = tracing::Span::current();
        thread::Builder::new()
            .name("nanocode-jsonl".into())
            .spawn(move || {
                let mut journal = match span.in_scope(open) {
                    Ok(journal) => journal,
                    Err(error) => return drop(ready.send(Err(error))),
                };
                drop(ready.send(Ok(())));
                while let Some(job) = receiver.blocking_recv() {
                    job(&mut journal);
                    if receiver.is_empty() {
                        journal.flush_lagging_index();
                    }
                }
                drop(journal);
                let _ = finished.send(());
            })
            .map_err(backend)?;
        Ok((Self { jobs, closed }, opened))
    }

    async fn opened(
        (store, opened): (Self, oneshot::Receiver<Result<(), StoreError>>),
    ) -> Result<Self, StoreError> {
        opened.await.map_err(|_| stopped())??;
        Ok(store)
    }

    fn write<T: Send + 'static>(
        &mut self,
        action: impl FnOnce(&mut Journal) -> Result<T, StoreError> + Send + 'static,
    ) -> StoreFuture<'_, Result<T, StoreError>> {
        self.dispatch(false, action)
    }

    fn read<T: Send + 'static>(
        &mut self,
        action: impl FnOnce(&mut Journal) -> Result<T, StoreError> + Send + 'static,
    ) -> StoreFuture<'_, Result<T, StoreError>> {
        self.dispatch(true, action)
    }

    fn dispatch<T: Send + 'static>(
        &mut self,
        abandonable: bool,
        action: impl FnOnce(&mut Journal) -> Result<T, StoreError> + Send + 'static,
    ) -> StoreFuture<'_, Result<T, StoreError>> {
        let parent = tracing::Span::current();
        let queued = Instant::now();
        let (reply, result) = oneshot::channel();
        let job: Job = Box::new(move |journal| {
            if abandonable && reply.is_closed() {
                return;
            }
            let span = tracing::info_span!(
                parent: &parent,
                "durability.jsonl.worker",
                queue.duration_ms = queued.elapsed().as_secs_f64() * 1_000.0
            );
            let _entered = span.enter();
            drop(reply.send(action(journal)));
        });
        Box::pin(async move {
            self.jobs.send(job).await.map_err(|_| stopped())?;
            result.await.map_err(|_| stopped())?
        })
    }
}

impl StateStore for JsonlStore {
    fn acquire_with<'a>(
        &'a mut self,
        id: &'a str,
        owner_id: OwnerId,
        update: StoreUpdate,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        let id = id.to_owned();
        self.write(move |journal| {
            journal
                .acquire_batch(&id, owner_id, update, None)
                .map(|(owned, _)| owned)
        })
    }

    fn batches_companion_acquisition(&self) -> bool {
        true
    }

    fn acquire_with_companion<'a>(
        &'a mut self,
        id: &'a str,
        owner_id: OwnerId,
        update: StoreUpdate,
        companion_id: &'a str,
        companion_owner: OwnerId,
    ) -> StoreFuture<'a, Result<(OwnedState, OwnedState), StoreError>> {
        let (id, companion_id) = (id.to_owned(), companion_id.to_owned());
        self.write(move |journal| {
            journal
                .acquire_batch(
                    &id,
                    owner_id,
                    update,
                    Some((&companion_id, companion_owner)),
                )
                .and_then(|(owned, companion)| {
                    companion
                        .map(|companion| (owned, companion))
                        .ok_or_else(|| backend("JSONL companion acquisition was not staged"))
                })
        })
    }

    fn read_record<'a>(
        &'a mut self,
        id: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Result<Option<String>, StoreError>> {
        let (id, key) = (id.to_owned(), key.to_owned());
        self.read(move |journal| journal.read_record(&id, &key))
    }
    fn read_records<'a>(
        &'a mut self,
        id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>> {
        let (id, keys) = (id.to_owned(), keys.to_vec());
        self.read(move |journal| journal.read_records(&id, &keys))
    }
    fn peek<'a>(&'a mut self, id: &'a str) -> StoreFuture<'a, Result<StoredState, StoreError>> {
        let id = id.to_owned();
        self.read(move |journal| journal.peek(&id))
    }
    fn owner<'a>(
        &'a mut self,
        id: &'a str,
    ) -> StoreFuture<'a, Result<Option<OwnerToken>, StoreError>> {
        let id = id.to_owned();
        self.read(move |journal| journal.owner(&id))
    }
    fn list_states<'a>(
        &'a mut self,
        limit: usize,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        self.read(move |journal| journal.list_states(limit))
    }
    fn list_records<'a>(
        &'a mut self,
        id: &'a str,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        let id = id.to_owned();
        self.read(move |journal| journal.list_records(&id))
    }
    fn list_summaries<'a>(
        &'a mut self,
        limit: usize,
    ) -> StoreFuture<'a, Result<Option<Vec<ListedState>>, StoreError>> {
        self.read(move |journal| journal.list_summaries(limit).map(Some))
    }
    fn remember_summaries<'a>(
        &'a mut self,
        summaries: Vec<KnownSummary>,
    ) -> StoreFuture<'a, Result<(), StoreError>> {
        self.write(move |journal| journal.remember_summaries(summaries))
    }
    fn acquire<'a>(
        &'a mut self,
        id: &'a str,
        owner_id: OwnerId,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        let id = id.to_owned();
        self.write(move |journal| journal.acquire(&id, owner_id))
    }
    fn replace<'a>(
        &'a mut self,
        id: &'a str,
        owner: &'a OwnerToken,
        expected: u64,
        payload: &'a str,
        records: &'a [StoreRecord],
    ) -> StoreFuture<'a, Result<u64, StoreError>> {
        let (id, owner, payload, records) = (
            id.to_owned(),
            owner.clone(),
            payload.to_owned(),
            records.to_vec(),
        );
        self.write(move |journal| journal.replace(&id, &owner, expected, payload, records))
    }

    fn commit_many(
        &mut self,
        commits: Vec<OwnedCommit>,
    ) -> StoreFuture<'_, Vec<Result<Committed, StoreError>>> {
        let count = commits.len();
        let replacements = commits
            .into_iter()
            .map(|commit| {
                if commit.events.is_empty() {
                    Ok(Replacement {
                        state_id: commit.state_id,
                        owner: commit.owner,
                        expected: commit.expected_revision,
                        payload: commit.payload,
                        records: commit.records,
                    })
                } else {
                    Err(StoreError::NotCommitted(
                        "this store does not record state events".to_owned(),
                    ))
                }
            })
            .collect();
        let replaced = self.write(move |journal| Ok(journal.replace_many(replacements)));
        Box::pin(async move {
            match replaced.await {
                Ok(results) => results
                    .into_iter()
                    .map(|result| {
                        result.map(|revision| Committed {
                            revision,
                            first_event: None,
                        })
                    })
                    .collect(),
                Err(error) => vec![Err(error); count],
            }
        })
    }

    fn flush(&mut self) -> StoreFuture<'_, Result<(), StoreError>> {
        self.write(|journal| {
            journal.flush_index();
            Ok(())
        })
    }
}

#[cfg(unix)]
fn identity(metadata: &Metadata) -> Result<String, StoreError> {
    use std::os::unix::fs::MetadataExt;
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn identity(metadata: &Metadata) -> Result<String, StoreError> {
    Ok(format!("{:?}", metadata.created().map_err(backend)?))
}

fn identity_changed() -> StoreError {
    backend("JSONL log identity changed while its store was open")
}

fn stopped() -> StoreError {
    backend("JSONL durability worker stopped")
}

fn backend(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
}
