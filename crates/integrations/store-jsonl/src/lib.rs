#![cfg(not(target_family = "wasm"))]

mod archive;
mod compact;
mod index;
mod migrate;
mod records;
mod sessions;
mod shared;
mod summaries;

pub use compact::CompactionStats;
use sessions::Sessions;

use nanocode_digest::Sha256;
use nanocode_durability_core::{
    Colocation, Committed, KnownSummary, ListedState, OwnedCommit, OwnedState, OwnerId, OwnerToken,
    Settle, StateCommit, StateStore, StoreError, StoreFuture, StoreHandles, StoreRecord,
    StoreReplacement, StoreUpdate, StoredState,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeStruct};
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fs::{File, Metadata, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    thread,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};

const JOURNAL_READ_BUFFER_BYTES: usize = 128 * 1024;
const RECOVERY_BLOCK_BYTES: usize = 64 * 1024 * 1024;
const SEAL_POLL_FIRST: Duration = Duration::from_millis(1);
const SEAL_POLL_LONGEST: Duration = Duration::from_millis(100);
const SHARED_RECORD: char = '\u{1}';

type Job = Box<dyn FnOnce(&mut Sessions) -> Option<Settled> + Send>;

type Settled = Box<dyn FnOnce(Result<(), StoreError>) + Send>;

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
    Import {
        state_id: String,
        owner: OwnerToken,
        after: Option<OwnerToken>,
        revision: u64,
        payload: String,
    },
    Records {
        state_id: String,
        owner: OwnerToken,
        records: Vec<StoreRecord>,
    },
}

impl Entry {
    fn claim(&self) -> Claim<'_> {
        match self {
            Self::Acquire { state_id, owner } => Claim {
                state_id,
                owner,
                kind: Claimed::Acquire,
            },
            Self::Replace {
                state_id,
                owner,
                revision,
                ..
            } => Claim {
                state_id,
                owner,
                kind: Claimed::Replace(*revision),
            },
            Self::Import {
                state_id,
                owner,
                after,
                revision,
                ..
            } => Claim {
                state_id,
                owner,
                kind: Claimed::Import {
                    after: after.as_ref(),
                    revision: *revision,
                },
            },
            Self::Records {
                state_id, owner, ..
            } => Claim {
                state_id,
                owner,
                kind: Claimed::Records,
            },
        }
    }

    fn records(&self) -> &[StoreRecord] {
        match self {
            Self::Replace { records, .. } | Self::Records { records, .. } => records,
            Self::Acquire { .. } | Self::Import { .. } => &[],
        }
    }
}

fn replacement(entries: Vec<Entry>) -> Option<StoreReplacement> {
    entries.into_iter().find_map(|entry| match entry {
        Entry::Replace {
            payload, records, ..
        } => Some(StoreReplacement { payload, records }),
        Entry::Acquire { .. } | Entry::Import { .. } | Entry::Records { .. } => None,
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
        let payload = match entry {
            Entry::Replace { payload, .. } | Entry::Import { payload, .. } => document(payload),
            Entry::Acquire { .. } | Entry::Records { .. } => None,
        };
        let documents = || {
            entry
                .records()
                .iter()
                .map(|record| record.value.strip_prefix('=').and_then(document))
        };
        let framed = payload.is_some() || documents().any(|document| document.is_some());
        if framed {
            bytes.push(b'[');
        }
        let first = spans.values.borrow().len();
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
                payload: Stored::of(payload),
                records: Spanned {
                    value: records,
                    spans,
                },
                batch,
            },
            Entry::Import {
                state_id,
                owner,
                after,
                revision,
                payload,
            } => Line::Import {
                state_id,
                owner,
                after: after.as_ref(),
                revision: *revision,
                payload: Stored::of(payload),
                records: Spanned { value: &[], spans },
                batch,
            },
            Entry::Records {
                state_id,
                owner,
                records,
            } => Line::Records {
                state_id,
                owner,
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
        if framed {
            if let Some(payload) = payload {
                bytes.push(b',');
                bytes.extend_from_slice(payload.as_bytes());
            }
            for (record, document) in documents().enumerate() {
                if let Some(document) = document {
                    bytes.push(b',');
                    let at = bytes.len();
                    bytes.extend_from_slice(document.as_bytes());
                    spans.values.borrow_mut()[first + record] = at..bytes.len();
                }
            }
            bytes.push(b']');
        }
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
        payload: Stored<'a>,
        records: Spanned<'a, [StoreRecord]>,
        #[serde(skip_serializing_if = "Option::is_none")]
        batch: Option<&'a Batch>,
    },
    Import {
        state_id: &'a str,
        owner: &'a OwnerToken,
        #[serde(skip_serializing_if = "Option::is_none")]
        after: Option<&'a OwnerToken>,
        revision: u64,
        payload: Stored<'a>,
        records: Spanned<'a, [StoreRecord]>,
        #[serde(skip_serializing_if = "Option::is_none")]
        batch: Option<&'a Batch>,
    },
    Records {
        state_id: &'a str,
        owner: &'a OwnerToken,
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

enum Stored<'a> {
    Text(&'a str),
    Document(usize),
}

impl<'a> Stored<'a> {
    fn of(text: &'a str) -> Self {
        document(text).map_or(Self::Text(text), |document| Self::Document(document.len()))
    }
}

impl Serialize for Stored<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Text(text) => serializer.serialize_str(text),
            Self::Document(length) => serializer.serialize_u64(*length as u64),
        }
    }
}

fn document(text: &str) -> Option<&str> {
    (text.starts_with(['{', '[']) && !text.as_bytes().contains(&b'\n')).then_some(text)
}

fn text(stored: &[u8], inline: bool) -> Result<String, StoreError> {
    match stored.first() {
        Some(b'"') => serde_json::from_slice(stored).map_err(backend),
        Some(b'{' | b'[') => {
            let document = std::str::from_utf8(stored).map_err(backend)?;
            Ok(if inline {
                format!("={document}")
            } else {
                document.to_owned()
            })
        }
        _ => Err(backend("a JSONL value is neither a string nor a document")),
    }
}

struct Documents<'a> {
    line: &'a [u8],
    at: Option<usize>,
}

impl<'a> Documents<'a> {
    fn of<T: Deserialize<'a>>(line: &'a [u8]) -> Result<(T, Self), serde_json::Error> {
        let Some(entry) = line.strip_prefix(b"[") else {
            return Ok((serde_json::from_slice(line)?, Self { line, at: None }));
        };
        let mut entries = serde_json::Deserializer::from_slice(entry).into_iter::<T>();
        let entry = entries
            .next()
            .ok_or_else(|| de::Error::custom("a JSONL frame holds no entry"))??;
        Ok((
            entry,
            Self {
                line,
                at: Some(1 + entries.byte_offset()),
            },
        ))
    }

    fn resolve(&mut self, value: &mut &'a [u8]) -> Result<(), serde_json::Error> {
        if !value.first().is_some_and(u8::is_ascii_digit) {
            return Ok(());
        }
        let document = std::str::from_utf8(value)
            .ok()
            .and_then(|length| length.parse::<usize>().ok())
            .zip(self.at)
            .and_then(|(length, at)| {
                let end = at.checked_add(1)?.checked_add(length)?;
                (self.line.get(at) == Some(&b','))
                    .then(|| self.line.get(at + 1..end))
                    .flatten()
                    .map(|document| (document, end))
            });
        let Some((document, end)) = document else {
            return Err(de::Error::custom(
                "a JSONL entry names a document its line does not hold",
            ));
        };
        *value = document;
        self.at = Some(end);
        Ok(())
    }

    fn end(&self) -> Result<(), serde_json::Error> {
        match self.at {
            Some(at) if self.line[at..].trim_ascii() != b"]" => Err(de::Error::custom(
                "a JSONL frame holds more than its entry's documents",
            )),
            _ => Ok(()),
        }
    }
}

struct Claim<'a> {
    state_id: &'a str,
    owner: &'a OwnerToken,
    kind: Claimed<'a>,
}

#[derive(Clone, Copy)]
enum Claimed<'a> {
    Acquire,
    Replace(u64),
    Import {
        after: Option<&'a OwnerToken>,
        revision: u64,
    },
    Records,
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
        match claim.kind {
            Claimed::Acquire => {
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
            Claimed::Replace(revision) => match owner {
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
            Claimed::Import { after, revision } => {
                if owner != after {
                    return Ok(Verdict::Stale);
                }
                let next = owner
                    .map_or(Some(1), |owner| owner.fence().checked_add(1))
                    .ok_or_else(|| backend("JSONL owner fence overflow"))?;
                match claim.owner.fence().cmp(&next) {
                    Ordering::Equal => {
                        owners.insert(claim.state_id, claim.owner);
                        revisions.insert(claim.state_id, revision);
                    }
                    Ordering::Less => return Ok(Verdict::Stale),
                    Ordering::Greater => return Err(backend("invalid JSONL owner fence sequence")),
                }
            }
            Claimed::Records => {
                if owner != Some(claim.owner) {
                    return Ok(Verdict::Stale);
                }
            }
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
        let parsed = Indexed::of(&line);
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
                nanocode_trace::warn!(
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
                                Indexed::of(bytes).map(|entry| (*location, entry, bytes.as_slice()))
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

fn cores() -> usize {
    static CORES: OnceLock<usize> = OnceLock::new();
    *CORES.get_or_init(|| thread::available_parallelism().map_or(1, std::num::NonZero::get))
}

type Recovered<'a> = (Location, Indexed<'a>, &'a [u8], Vec<[u8; 32]>);

type Parsed<'a> = Result<(Indexed<'a>, Vec<[u8; 32]>), serde_json::Error>;

fn parsed<'a>((_, line): &(Location, &'a [u8])) -> Parsed<'a> {
    let entry = Indexed::of(line)?;
    let digests = match (&entry.state_id, &entry.records) {
        (Some(id), Some(records)) => records
            .iter()
            .map(|record| value_digest(id, &record.key, record.value))
            .collect(),
        _ => Vec::new(),
    };
    Ok((entry, digests))
}

fn recovered<'a>(lines: &[(Location, &'a [u8])]) -> Result<Vec<Parsed<'a>>, StoreError> {
    let bytes = lines.iter().map(|(_, line)| line.len()).sum::<usize>();
    let workers = cores().min(bytes / JOURNAL_READ_BUFFER_BYTES);
    if workers < 2 {
        return Ok(lines.iter().map(parsed).collect());
    }
    let share = bytes.div_ceil(workers);
    let mut parts = Vec::with_capacity(workers);
    let (mut from, mut held) = (0, 0);
    for (index, (_, line)) in lines.iter().enumerate() {
        held += line.len();
        if held >= share {
            parts.push(&lines[from..=index]);
            (from, held) = (index + 1, 0);
        }
    }
    parts.push(&lines[from..]);
    thread::scope(|scope| {
        let running = parts
            .into_iter()
            .map(|part| scope.spawn(move || part.iter().map(parsed).collect::<Vec<_>>()))
            .collect::<Vec<_>>();
        let mut all = Vec::with_capacity(lines.len());
        for part in running {
            all.extend(
                part.join()
                    .map_err(|_| backend("a JSONL recovery worker stopped"))?,
            );
        }
        Ok(all)
    })
}

fn stored_records(records: &[IndexedRecord<'_>]) -> Result<Vec<StoreRecord>, StoreError> {
    records
        .iter()
        .map(|record| {
            Ok(StoreRecord {
                key: record.key.to_string(),
                value: text(record.value, true)?,
            })
        })
        .collect()
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
    let revision = || {
        entry
            .revision
            .ok_or_else(|| backend("invalid JSONL replacement entry"))
    };
    let kind = match entry.kind {
        Kind::Acquire => Claimed::Acquire,
        Kind::Replace => Claimed::Replace(revision()?),
        Kind::Import => Claimed::Import {
            after: entry.after.as_ref(),
            revision: revision()?,
        },
        Kind::Records => Claimed::Records,
        Kind::Seal | Kind::Unseal => return Err(backend("JSONL seal inside an entry batch")),
    };
    Ok(Claim {
        state_id,
        owner,
        kind,
    })
}

#[cfg(target_vendor = "apple")]
fn sync_journal(file: &File) -> std::io::Result<()> {
    nanocode_platform::unix::barrier_sync_file(file).or_else(|_| file.sync_all())
}

#[cfg(not(target_vendor = "apple"))]
fn sync_journal(file: &File) -> std::io::Result<()> {
    file.sync_all()
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    nanocode_platform::unix::probe_process(pid)
        .err()
        .and_then(|error| error.raw_os_error())
        != Some(libc::ESRCH)
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
        let written = match self.value.strip_prefix('=').and_then(document) {
            Some(document) => serializer.serialize_u64(document.len() as u64)?,
            None => serializer.serialize_str(self.value)?,
        };
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
    Import,
    Records,
    Seal,
    Unseal,
}

#[derive(Deserialize)]
struct Indexed<'a> {
    #[serde(rename = "type")]
    kind: Kind,
    #[serde(borrow)]
    state_id: Option<Cow<'a, str>>,
    owner: Option<OwnerToken>,
    after: Option<OwnerToken>,
    revision: Option<u64>,
    #[serde(borrow, default, deserialize_with = "stored_payload")]
    payload: Option<&'a [u8]>,
    #[serde(borrow)]
    records: Option<Vec<IndexedRecord<'a>>>,
    batch: Option<Batch>,
    #[serde(borrow)]
    seal: Option<Cow<'a, str>>,
    pid: Option<u32>,
}

impl<'a> Indexed<'a> {
    fn of(line: &'a [u8]) -> Result<Self, serde_json::Error> {
        let (mut entry, mut documents) = Documents::of::<Self>(line)?;
        for value in entry.payload.iter_mut().chain(
            entry
                .records
                .iter_mut()
                .flatten()
                .map(|record| &mut record.value),
        ) {
            documents.resolve(value)?;
        }
        documents.end()?;
        Ok(entry)
    }
}

#[derive(Deserialize)]
struct IndexedRecord<'a> {
    #[serde(borrow)]
    key: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "stored")]
    value: &'a [u8],
}

struct Text;

fn stored_payload<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<&'de [u8]>, D::Error> {
    stored(deserializer).map(Some)
}

fn stored<'de, D: Deserializer<'de>>(deserializer: D) -> Result<&'de [u8], D::Error> {
    let raw = <&serde_json::value::RawValue>::deserialize(deserializer)?;
    if raw.get().starts_with(|first: char| first.is_ascii_digit()) {
        return Ok(raw.get().as_bytes());
    }
    if !raw.get().starts_with('"') {
        return Err(de::Error::custom("expected a string or a document"));
    }
    if raw.get().contains("\\u") {
        let mut decoder = serde_json::Deserializer::from_str(raw.get());
        decoder
            .deserialize_str(TextVisitor)
            .map_err(de::Error::custom)?;
    }
    Ok(raw.get().as_bytes())
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

    fn located(id: &str, key: &str, line: &[u8], position: u64, value: &[u8]) -> Self {
        let start = value.as_ptr().addr() - line.as_ptr().addr();
        Self::value(id, key, position + start as u64, value)
    }

    fn placed(line: &[u8], position: u64, value: &[u8], digest: [u8; 32]) -> Self {
        let start = value.as_ptr().addr() - line.as_ptr().addr();
        Self::Value {
            location: Location {
                position: position + start as u64,
                length: value.len(),
            },
            digest,
        }
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
        digest.update(&(part.len() as u64).to_le_bytes());
        digest.update(part);
    }
    digest.update(bytes);
    digest.finalize()
}

struct State {
    revision: u64,
    location: Location,
    records: records::RecordMap,
    created: usize,
}

fn key_hash(key: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
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
    #[serde(borrow, deserialize_with = "stored")]
    payload: &'a [u8],
    #[serde(borrow)]
    records: Vec<IndexedRecord<'a>>,
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
        let (mut entry, mut documents) = Documents::of::<StoredEntry>(&bytes).map_err(backend)?;
        if !matches!(entry.kind, Kind::Replace | Kind::Import) {
            return Err(backend("JSONL index does not reference a replacement"));
        }
        for value in std::iter::once(&mut entry.payload)
            .chain(entry.records.iter_mut().map(|record| &mut record.value))
        {
            documents.resolve(value).map_err(backend)?;
        }
        documents.end().map_err(backend)?;
        let range = |value: &[u8]| {
            let start = value.as_ptr().addr() - bytes.as_ptr().addr();
            start..start + value.len()
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

    fn text(&self, range: Range<usize>, inline: bool) -> Result<String, StoreError> {
        text(&self.bytes[range], inline)
    }
}

struct Journal {
    file: File,
    synced: Arc<File>,
    reads: File,
    path: PathBuf,
    lineage: String,
    index: index::Index,
    persisted_offset: Option<u64>,
    ready: bool,
    invalid_index: Option<String>,
    cached: Option<CachedEntry>,
    sealed: Option<Sealed>,
    sidecar: bool,
    unsynced: bool,
    current: bool,
    shared: Option<shared::Shared>,
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
    fn open(path: &Path, sidecar: bool) -> Result<Self, StoreError> {
        let _span = nanocode_trace::info_span!("durability.jsonl.open").entered();
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
            synced: Arc::new(file.try_clone().map_err(backend)?),
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
            sidecar,
            unsynced: false,
            current: false,
            shared: None,
        };
        if sidecar {
            store.restore_index();
        }
        store.sync()?;
        if sidecar {
            store.clear_temporaries().map_err(backend)?;
            if store.persisted_offset.is_none() {
                store.persist_index();
            }
        }
        store.ready = true;
        Ok(store)
    }

    fn temporary(&self, suffix: &str) -> std::io::Result<nanocode_platform::TemporaryFile> {
        let (directory, prefix) = self.temporary_prefix(suffix)?;
        nanocode_platform::TemporaryFile::create_in(directory, prefix, None)
    }

    fn temporary_copy(
        &self,
        suffix: &str,
        source: &Path,
    ) -> std::io::Result<nanocode_platform::TemporaryFile> {
        let (directory, prefix) = self.temporary_prefix(suffix)?;
        nanocode_platform::TemporaryFile::make_in(directory, prefix, |path| {
            std::fs::copy(source, path)?;
            OpenOptions::new().read(true).append(true).open(path)
        })
    }

    fn temporary_prefix(&self, suffix: &str) -> std::io::Result<(&Path, std::ffi::OsString)> {
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
        Ok((directory, prefix))
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

    fn sync(&mut self) -> Result<(), StoreError> {
        loop {
            let current = identity(&std::fs::metadata(&self.path).map_err(backend)?)?;
            if current == self.index.source {
                break;
            }
            self.reopen_compacted()?;
        }
        self.adopt_saved_index();
        self.refresh_index()?;
        self.current = true;
        Ok(())
    }

    fn read(&mut self) -> Result<(), StoreError> {
        if self.current {
            return Ok(());
        }
        self.sync()
    }

    const fn stale(&mut self) {
        self.current = false;
    }

    fn harden(&mut self) -> Result<(), StoreError> {
        if !std::mem::take(&mut self.unsynced) {
            return Ok(());
        }
        let _span = nanocode_trace::info_span!("durability.jsonl.sync").entered();
        sync_journal(&self.file).map_err(backend)
    }

    fn unsynced(&mut self) -> Option<Arc<File>> {
        std::mem::take(&mut self.unsynced).then(|| Arc::clone(&self.synced))
    }

    fn writable(&mut self) -> Result<(), StoreError> {
        let mut delay = SEAL_POLL_FIRST;
        loop {
            self.sync()?;
            let Some(sealed) = self.sealed.clone() else {
                return Ok(());
            };
            if !process_alive(sealed.pid) {
                nanocode_trace::warn!(
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
        let _span = nanocode_trace::info_span!(
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
        let mut position = self.index.offset;
        let mut carried = (Vec::new(), Vec::new());
        loop {
            let (mut block, mut lengths) = std::mem::take(&mut carried);
            let first = position - block.len() as u64;
            let wanted = block.len().saturating_mul(2).max(RECOVERY_BLOCK_BYTES);
            let mut torn = false;
            while position < end && block.len() < wanted {
                let before = block.len();
                reader.read_until(b'\n', &mut block).map_err(backend)?;
                if block.last() != Some(&b'\n') || block.len() == before {
                    block.truncate(before);
                    torn = true;
                    break;
                }
                lengths.push(block.len() - before);
                position = position
                    .checked_add((block.len() - before) as u64)
                    .ok_or_else(|| backend("JSONL offset overflow"))?;
                if position > end {
                    return Err(backend("JSONL entry crossed an appended boundary"));
                }
            }
            let mut lines = Vec::with_capacity(lengths.len());
            let mut start = 0;
            let mut scanned = 0;
            for length in &lengths {
                lines.push((
                    Location {
                        position: first + scanned as u64,
                        length: *length,
                    },
                    &block[scanned..scanned + length],
                ));
                scanned += length;
            }
            let last_block = torn || position >= end;
            let mut group: Vec<Recovered<'_>> = Vec::new();
            let mut open: Option<Batch> = None;
            for (index, parsed) in recovered(&lines)?.into_iter().enumerate() {
                let (location, line) = lines[index];
                let batch = parsed
                    .as_ref()
                    .ok()
                    .and_then(|(entry, _)| entry.batch.clone())
                    .filter(|batch| batch.count > 1);
                if open.is_some() && open != batch {
                    let (last, ..) = group.pop().ok_or_else(|| backend("empty JSONL batch"))?;
                    group.clear();
                    open = None;
                    self.recovered(None, last)?;
                    start = index;
                }
                match parsed {
                    Err(error) => {
                        nanocode_trace::warn!(
                            position = location.position,
                            %error,
                            "skipping an unparseable JSONL line"
                        );
                        self.recovered(None, location)?;
                        start = index + 1;
                    }
                    Ok((entry, digests)) => match batch {
                        Some(batch) => {
                            group.push((location, entry, line, digests));
                            let complete = group.len() == batch.count;
                            open = Some(batch);
                            if complete {
                                self.recovered(Some(&group), location)?;
                                group.clear();
                                open = None;
                                start = index + 1;
                            }
                        }
                        None => {
                            self.recovered(Some(&[(location, entry, line, digests)]), location)?;
                            start = index + 1;
                        }
                    },
                }
            }
            if last_block {
                if until.is_some()
                    && position == end
                    && let Some((last, ..)) = group.pop()
                {
                    self.recovered(None, last)?;
                }
                return Ok(());
            }
            let kept = lines.get(start).map_or(block.len(), |(location, _)| {
                (location.position - first) as usize
            });
            drop(group);
            drop(lines);
            carried = (block.split_off(kept), lengths.split_off(start));
        }
    }

    fn recovered(
        &mut self,
        lines: Option<&[Recovered<'_>]>,
        last: Location,
    ) -> Result<(), StoreError> {
        if let Some(lines) = lines {
            self.settle(lines)?;
        }
        self.index.last = Some(last);
        self.index.offset = last
            .position
            .checked_add(last.length as u64)
            .ok_or_else(|| backend("JSONL offset overflow"))?;
        Ok(())
    }

    fn settle(&mut self, lines: &[Recovered<'_>]) -> Result<(), StoreError> {
        if let [(_, entry, ..)] = lines
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
            .map(|(_, entry, ..)| claim(entry))
            .collect::<Result<Vec<_>, _>>()?;
        if matches!(judge(&self.index, &claims)?, Verdict::Stale) {
            nanocode_trace::debug!(
                position = lines.first().map(|(location, ..)| location.position),
                "skipping a JSONL entry superseded by a concurrent writer"
            );
            return Ok(());
        }
        for (location, entry, line, digests) in lines {
            self.replay(*location, entry, line, digests)?;
        }
        Ok(())
    }

    fn replay(
        &mut self,
        location: Location,
        indexed: &Indexed<'_>,
        line: &[u8],
        digests: &[[u8; 32]],
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
                let (Some(revision), Some(_), Some(records)) =
                    (indexed.revision, indexed.payload, &indexed.records)
                else {
                    return Err(backend("invalid JSONL replacement entry"));
                };
                self.check_replacement(id, &owner, revision)?;
                let known = self.index.states.get(id).map(|state| &state.records);
                let mut keys = HashSet::with_capacity(records.len());
                let repeated = records.iter().any(|record| {
                    !keys.insert(&*record.key)
                        || known.is_some_and(|known| known.contains_key(&record.key))
                });
                if repeated {
                    self.validate_records(id, &stored_records(records)?)?;
                }
                self.replaced(
                    id,
                    revision,
                    location,
                    records.iter().zip(digests).map(|(record, digest)| {
                        (
                            &*record.key,
                            Record::placed(line, location.position, record.value, *digest),
                        )
                    }),
                );
                Ok(())
            }
            Kind::Import => {
                let (Some(revision), Some(_)) = (indexed.revision, indexed.payload) else {
                    return Err(backend("invalid JSONL import entry"));
                };
                self.imported(id, owner, revision, location)
            }
            Kind::Records => {
                let Some(records) = &indexed.records else {
                    return Err(backend("invalid JSONL records entry"));
                };
                if self.index.owners.get(id) != Some(&owner) || self.revision(id) == 0 {
                    return Err(backend("invalid JSONL records owner"));
                }
                let known = self.index.states.get(id).map(|state| &state.records);
                let mut keys = HashSet::with_capacity(records.len());
                if records.iter().any(|record| {
                    !keys.insert(&*record.key)
                        || known.is_some_and(|known| known.contains_key(&record.key))
                }) {
                    self.validate_records(id, &stored_records(records)?)?;
                }
                self.added(
                    id,
                    records.iter().zip(digests).map(|(record, digest)| {
                        (
                            &*record.key,
                            Record::placed(line, location.position, record.value, *digest),
                        )
                    }),
                );
                Ok(())
            }
            Kind::Seal | Kind::Unseal => Err(backend("JSONL seal inside an entry batch")),
        }
    }

    fn imported(
        &mut self,
        id: &str,
        owner: OwnerToken,
        revision: u64,
        location: Location,
    ) -> Result<(), StoreError> {
        if revision == 0 {
            return Err(backend("invalid JSONL import revision"));
        }
        self.acquired(id, owner)?;
        self.replaced(id, revision, location, std::iter::empty());
        Ok(())
    }

    fn added<'k>(&mut self, id: &str, records: impl Iterator<Item = (&'k str, Record)>) {
        if let Some(state) = self.index.states.get_mut(id) {
            for (key, record) in records {
                if state.records.add(key, record) {
                    self.index
                        .holders
                        .entry(key_hash(key))
                        .or_insert(state.created);
                }
            }
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
            self.index.states.insert(
                id.into(),
                State {
                    revision,
                    location,
                    records: records::RecordMap::default(),
                    created: self.index.created.len(),
                },
            );
            self.index.created.push(id.into());
        }
        let state = self
            .index
            .states
            .get_mut(id)
            .expect("a replaced state is indexed");
        state.revision = revision;
        state.location = location;
        for (key, record) in records {
            if state.records.add(key, record) {
                self.index
                    .holders
                    .entry(key_hash(key))
                    .or_insert(state.created);
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
            payload: Some(entry.text(entry.payload.clone(), false)?),
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
            .map(|state| state.records.keys().collect())
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

    fn shares(
        &mut self,
        candidate: &str,
        record: &StoreRecord,
    ) -> Result<Option<String>, StoreError> {
        if record.value.len() <= SHARED_RECORD.len_utf8() + candidate.len()
            || self.location(candidate, &record.key)?.is_none()
        {
            return Ok(None);
        }
        Ok(self
            .resolved(candidate, &record.key)?
            .filter(|(found, value)| {
                *value == record.value
                    && record.value.len() > SHARED_RECORD.len_utf8() + found.len()
            })
            .map(|(found, _)| found))
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
        for (record, holder) in &mut records {
            let Some(candidate) = self
                .index
                .holders
                .get(&key_hash(&record.key))
                .and_then(|created| self.index.created.get(*created))
                .filter(|candidate| candidate.as_str() != id)
                .cloned()
            else {
                continue;
            };
            *holder = self.shares(&candidate, record)?;
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
        text(&bytes, true).map_err(|error| self.invalidated(error))
    }

    #[cfg(unix)]
    fn read_spread(&mut self, id: &str, keys: &[String]) -> Option<Vec<Option<String>>> {
        use std::os::unix::fs::FileExt;
        let mut located = Vec::with_capacity(keys.len());
        let mut bytes = 0;
        for key in keys {
            match self.location(id, key) {
                Ok(Some(record @ Record::Value { location, .. })) => {
                    bytes += location.length;
                    located.push(Some(record));
                }
                Ok(None) => located.push(None),
                Ok(Some(Record::Entry(_))) | Err(_) => return None,
            }
        }
        let workers = cores().min(bytes / JOURNAL_READ_BUFFER_BYTES);
        if workers < 2 {
            return None;
        }
        let wanted = keys.iter().zip(located).collect::<Vec<_>>();
        let reads = &self.reads;
        let read = |part: &[(&String, Option<Record>)]| {
            part.iter()
                .map(|(key, located)| {
                    let Some(Record::Value { location, digest }) = located else {
                        return Some(None);
                    };
                    let mut stored = vec![0; location.length];
                    reads.read_exact_at(&mut stored, location.position).ok()?;
                    if value_digest(id, key, &stored) != *digest {
                        return None;
                    }
                    text(&stored, true).ok().map(Some)
                })
                .collect::<Option<Vec<_>>>()
        };
        thread::scope(|scope| {
            let running = wanted
                .chunks(wanted.len().div_ceil(workers))
                .map(|part| scope.spawn(move || read(part)))
                .collect::<Vec<_>>();
            let mut values = Vec::with_capacity(wanted.len());
            for part in running {
                values.extend(part.join().ok()??);
            }
            Some(values)
        })
    }

    #[cfg(not(unix))]
    fn read_spread(&mut self, _: &str, _: &[String]) -> Option<Vec<Option<String>>> {
        None
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
        let value = match entry.text(range, true) {
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
            if record.value.is_empty() {
                fresh.push(false);
                continue;
            }
            let retained = match self.matches(id, record, &mut encoded, &mut stored)? {
                Some(false) => Some(self.same_content(id, record)?),
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

    fn same_content(&mut self, id: &str, record: &StoreRecord) -> Result<bool, StoreError> {
        let Some(shared) = self.shared.clone() else {
            return Ok(false);
        };
        let Some(existing) = self.record(id, &record.key)? else {
            return Ok(false);
        };
        Ok(shared.load(existing)? == shared.load(record.value.clone())?)
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
            match record
                .value
                .strip_prefix('=')
                .and_then(document)
                .filter(|document| document.len() == location.length)
            {
                Some(document) => encoded.extend_from_slice(document.as_bytes()),
                None => serde_json::to_writer(&mut *encoded, &record.value).map_err(backend)?,
            }
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
        self.unsynced = true;
        end.checked_sub(bytes.len() as u64)
            .ok_or_else(|| backend("JSONL append position underflow"))
    }

    fn append_marker(&mut self, line: &Line<'_>) -> Result<(), StoreError> {
        let mut bytes = serde_json::to_vec(line).map_err(backend)?;
        bytes.push(b'\n');
        let start = self.append_raw(&bytes)?;
        self.harden()?;
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
        self.harden()?;
        self.consume(Some(start))?;
        if self.index.offset != start {
            return Err(backend("JSONL journal did not reach its appended seal"));
        }
        let won = self.sealed.is_none();
        self.consume(None)?;
        Ok(won.then_some(start))
    }

    fn commit(&mut self, entries: Vec<Entry>) -> Result<Outcome, StoreError> {
        let span = nanocode_trace::info_span!(
            "durability.jsonl.append",
            entry.count = entries.len(),
            log.bytes = nanocode_trace::field::Empty,
            encode.duration_ms = nanocode_trace::field::Empty,
            index.duration_ms = nanocode_trace::field::Empty,
            outcome = nanocode_trace::field::Empty
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
        let held = self.unsynced;
        let origin = self.append_raw(&bytes)?;
        self.unsynced = held
            || entries
                .iter()
                .any(|entry| !matches!(entry, Entry::Acquire { .. }));
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
            let spans = values.by_ref().take(entry.records().len()).collect();
            self.apply(entry, location, spans, origin, &bytes)?;
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
        let span = nanocode_trace::info_span!(
            "durability.jsonl.append",
            entry.count = entries.len(),
            log.bytes = nanocode_trace::field::Empty,
            encode.duration_ms = nanocode_trace::field::Empty,
            index.duration_ms = nanocode_trace::field::Empty,
            outcome = nanocode_trace::field::Empty
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
            let spans: Vec<Range<usize>> = values.by_ref().take(entry.records().len()).collect();
            let verdict = if sealed {
                Verdict::Stale
            } else {
                judge(&self.index, &[entry.claim()])?
            };
            judged.push(match (verdict, sealed) {
                (Verdict::Stale, true) => Judged::Sealed(entry),
                (Verdict::Stale, false) => Judged::Stale(entry),
                (Verdict::Valid, _) => {
                    self.apply(entry, location, spans, origin, &bytes)?;
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
    fn apply(
        &mut self,
        entry: Entry,
        location: Location,
        spans: Vec<Range<usize>>,
        origin: u64,
        bytes: &[u8],
    ) -> Result<(), StoreError> {
        let located = |state_id: &str, records: &[StoreRecord]| {
            records
                .iter()
                .zip(spans.iter())
                .map(|(record, span)| {
                    (
                        record.key.clone(),
                        Record::value(
                            state_id,
                            &record.key,
                            origin + span.start as u64,
                            &bytes[span.clone()],
                        ),
                    )
                })
                .collect::<Vec<_>>()
        };
        match entry {
            Entry::Acquire { state_id, owner } => self.acquired(&state_id, owner),
            Entry::Replace {
                state_id,
                owner,
                revision,
                records,
                ..
            } => {
                self.check_replacement(&state_id, &owner, revision)?;
                let records = located(&state_id, &records);
                self.replaced(
                    &state_id,
                    revision,
                    location,
                    records.iter().map(|(key, record)| (key.as_str(), *record)),
                );
                Ok(())
            }
            Entry::Import {
                state_id,
                owner,
                revision,
                ..
            } => self.imported(&state_id, owner, revision, location),
            Entry::Records {
                state_id, records, ..
            } => {
                let records = located(&state_id, &records);
                self.added(
                    &state_id,
                    records.iter().map(|(key, record)| (key.as_str(), *record)),
                );
                Ok(())
            }
        }
    }

    fn import(
        &mut self,
        id: &str,
        after: Option<&OwnerToken>,
        owner_id: OwnerId,
        revision: u64,
        payload: String,
        records: Vec<Vec<StoreRecord>>,
    ) -> Result<bool, StoreError> {
        loop {
            self.writable()?;
            if self.index.owners.get(id) != after {
                return Ok(false);
            }
            let fence = self
                .next_fence(id)
                .ok_or_else(|| StoreError::NotCommitted("JSONL owner fence overflow".into()))?;
            let owner = OwnerToken::new(owner_id.clone(), fence);
            let mut entries = vec![Entry::Import {
                state_id: id.into(),
                owner: owner.clone(),
                after: after.cloned(),
                revision,
                payload: payload.clone(),
            }];
            entries.extend(records.iter().map(|records| Entry::Records {
                state_id: id.into(),
                owner: owner.clone(),
                records: records.clone(),
            }));
            match self.commit(entries)? {
                Outcome::Committed => return Ok(true),
                Outcome::Stale(_) => return Ok(false),
                Outcome::Sealed(_) => {}
            }
        }
    }

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
        self.read()?;
        self.rebuilt(|store| store.record(id, key))
    }

    fn read_records(
        &mut self,
        id: &str,
        keys: &[String],
    ) -> Result<Vec<Option<String>>, StoreError> {
        self.read()?;
        if let Some(mut values) = self.read_spread(id, keys) {
            for (key, value) in keys.iter().zip(&mut values) {
                if value
                    .as_deref()
                    .is_some_and(|value| value.starts_with(SHARED_RECORD))
                {
                    *value = self.rebuilt(|store| store.record(id, key))?;
                }
            }
            return Ok(values);
        }
        self.rebuilt(|store| keys.iter().map(|key| store.record(id, key)).collect())
    }

    fn peek(&mut self, id: &str) -> Result<StoredState, StoreError> {
        self.read()?;
        self.state(id)
    }

    fn owner(&mut self, id: &str) -> Result<Option<OwnerToken>, StoreError> {
        self.read()?;
        Ok(self.index.owners.get(id).cloned())
    }

    fn list_records(&mut self, id: &str) -> Result<Vec<String>, StoreError> {
        self.read()?;
        self.rebuilt(|store| store.record_keys(id))
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

    fn acquire_many(
        &mut self,
        acquisitions: &[(String, OwnerId)],
    ) -> Result<Vec<OwnedState>, StoreError> {
        let distinct = acquisitions
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<HashSet<_>>();
        if distinct.len() != acquisitions.len() {
            return acquisitions
                .iter()
                .map(|(id, owner_id)| self.acquire(id, owner_id.clone()))
                .collect();
        }
        loop {
            self.writable()?;
            let owners = acquisitions
                .iter()
                .map(|(id, owner_id)| {
                    self.next_fence(id)
                        .map(|fence| OwnerToken::new(owner_id.clone(), fence))
                        .ok_or_else(|| {
                            StoreError::NotCommitted("JSONL owner fence overflow".into())
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let entries = acquisitions
                .iter()
                .zip(&owners)
                .map(|((id, _), owner)| Entry::Acquire {
                    state_id: id.clone(),
                    owner: owner.clone(),
                })
                .collect();
            if let Outcome::Committed = self.commit(entries)? {
                return acquisitions
                    .iter()
                    .zip(owners)
                    .map(|((id, _), owner)| {
                        Ok(OwnedState {
                            owner,
                            state: self.state(id)?,
                        })
                    })
                    .collect();
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
                    Judged::Sealed(
                        Entry::Acquire { .. } | Entry::Import { .. } | Entry::Records { .. },
                    )
                    | Judged::Stale(
                        Entry::Acquire { .. } | Entry::Import { .. } | Entry::Records { .. },
                    ) => {
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
enum Reply {
    Wanted,
    Applied,
    Settled,
}

pub struct JsonlStore {
    jobs: mpsc::UnboundedSender<Job>,
    closed: Option<oneshot::Receiver<()>>,
    failed: Arc<OnceLock<StoreError>>,
    colocated: Arc<Colocated>,
}

#[derive(Clone)]
pub struct JsonlArchiver {
    jobs: mpsc::UnboundedSender<Job>,
    failed: Arc<OnceLock<StoreError>>,
    colocated: Arc<Colocated>,
}

impl JsonlArchiver {
    pub async fn archive(&self) -> Result<usize, StoreError> {
        let mut store = JsonlStore {
            jobs: self.jobs.clone(),
            closed: None,
            failed: Arc::clone(&self.failed),
            colocated: Arc::clone(&self.colocated),
        };
        store.write(Sessions::archive).await
    }

    pub async fn written(&self) -> Result<Vec<String>, StoreError> {
        let mut store = JsonlStore {
            jobs: self.jobs.clone(),
            closed: None,
            failed: Arc::clone(&self.failed),
            colocated: Arc::clone(&self.colocated),
        };
        store.read(|sessions| Ok(sessions.written())).await
    }

    pub async fn live_owner(&self, state_id: &str) -> Result<Option<u32>, StoreError> {
        let mut store = JsonlStore {
            jobs: self.jobs.clone(),
            closed: None,
            failed: Arc::clone(&self.failed),
            colocated: Arc::clone(&self.colocated),
        };
        let state_id = state_id.to_owned();
        store
            .read(move |sessions| sessions.live_owner(&state_id))
            .await
    }
}

struct Colocated(std::sync::mpsc::Sender<(String, String)>);

impl Colocation for Colocated {
    fn colocate(&self, state_id: &str, beside: &str) {
        let _ = self.0.send((state_id.to_owned(), beside.to_owned()));
    }
}

impl JsonlStore {
    pub fn open(directory: impl AsRef<Path>) -> Result<Self, StoreError> {
        let sessions = Sessions::open(directory.as_ref().to_owned())?;
        Self::start(move || Ok(sessions)).map(|(store, _)| store)
    }

    pub fn open_home(home: &Path) -> Result<Self, StoreError> {
        let sessions = Sessions::open_home(home)?;
        Self::start(move || Ok(sessions)).map(|(store, _)| store)
    }

    pub async fn open_async(directory: impl AsRef<Path>) -> Result<Self, StoreError> {
        let directory = directory.as_ref().to_owned();
        Self::opened(Self::start(move || Sessions::open(directory))?).await
    }

    pub async fn open_home_async(home: &Path) -> Result<Self, StoreError> {
        let home = home.to_owned();
        Self::opened(Self::start(move || Sessions::open_home(&home))?).await
    }

    pub fn open_home_in_background(home: &Path) -> Result<Self, StoreError> {
        let home = home.to_owned();
        Self::start(move || Sessions::open_home(&home)).map(|(store, _)| store)
    }

    pub async fn compact(&mut self) -> Result<CompactionStats, StoreError> {
        self.write(Sessions::compact).await
    }

    #[must_use]
    pub fn archiver(&self) -> JsonlArchiver {
        JsonlArchiver {
            jobs: self.jobs.clone(),
            failed: Arc::clone(&self.failed),
            colocated: Arc::clone(&self.colocated),
        }
    }

    pub async fn close(self) -> Result<(), StoreError> {
        let Self {
            jobs,
            closed,
            failed,
            colocated: _,
        } = self;
        drop(jobs);
        match closed {
            Some(closed) => closed
                .await
                .map_err(|_| failed.get().cloned().unwrap_or_else(stopped)),
            None => Ok(()),
        }
    }

    fn start(
        open: impl FnOnce() -> Result<Sessions, StoreError> + Send + 'static,
    ) -> Result<(Self, oneshot::Receiver<Result<(), StoreError>>), StoreError> {
        let (jobs, mut receiver) = mpsc::unbounded_channel::<Job>();
        let (finished, closed) = oneshot::channel();
        let (ready, opened) = oneshot::channel();
        let (colocated, hints) = std::sync::mpsc::channel::<(String, String)>();
        let failed = Arc::new(OnceLock::new());
        let failure = Arc::clone(&failed);
        let span = nanocode_trace::Span::current();
        thread::Builder::new()
            .name("nanocode-jsonl".into())
            .spawn(move || {
                let mut sessions = match span.in_scope(open) {
                    Ok(sessions) => sessions,
                    Err(error) => {
                        drop(failure.set(error.clone()));
                        return drop(ready.send(Err(error)));
                    }
                };
                drop(ready.send(Ok(())));
                let (syncs, unsynced) =
                    std::sync::mpsc::channel::<(Vec<Arc<File>>, Vec<Settled>)>();
                let hardened = Arc::clone(&failure);
                let syncer = thread::Builder::new()
                    .name("nanocode-jsonl-sync".into())
                    .spawn(move || {
                        while let Ok(first) = unsynced.recv() {
                            let mut files = Vec::<Arc<File>>::new();
                            let mut written = Vec::new();
                            for (unsynced, replies) in
                                std::iter::once(first).chain(unsynced.try_iter())
                            {
                                for file in unsynced {
                                    if !files.iter().any(|known| Arc::ptr_eq(known, &file)) {
                                        files.push(file);
                                    }
                                }
                                written.extend(replies);
                            }
                            let settled = match hardened.get() {
                                Some(error) => Err(error.clone()),
                                None => {
                                    let _span = nanocode_trace::info_span!(
                                        "durability.jsonl.sync",
                                        reply.count = written.len()
                                    )
                                    .entered();
                                    files
                                        .iter()
                                        .try_for_each(|file| sync_journal(file))
                                        .map_err(backend)
                                }
                            };
                            if let Err(error) = &settled {
                                drop(hardened.set(error.clone()));
                            }
                            for reply in written {
                                reply(settled.clone());
                            }
                        }
                    });
                let syncer = match syncer {
                    Ok(syncer) => syncer,
                    Err(error) => return drop(failure.set(backend(error))),
                };
                while let Some(first) = receiver.blocking_recv() {
                    if failure.get().is_some() {
                        break;
                    }
                    sessions.stale();
                    let mut queued = receiver.len();
                    let mut written = Vec::new();
                    let mut job = Some(first);
                    while let Some(run) = job.take() {
                        for (state_id, beside) in hints.try_iter() {
                            sessions.colocate(&state_id, &beside);
                        }
                        written.extend(run(&mut sessions));
                        if queued > 0 {
                            queued -= 1;
                            job = receiver.try_recv().ok();
                        }
                    }
                    let unsynced = sessions.unsynced();
                    if (!written.is_empty() || !unsynced.is_empty())
                        && syncs.send((unsynced, written)).is_err()
                    {
                        break;
                    }
                }
                drop(syncs);
                drop(syncer.join());
                drop(sessions);
                let _ = finished.send(());
            })
            .map_err(backend)?;
        Ok((
            Self {
                jobs,
                closed: Some(closed),
                failed,
                colocated: Arc::new(Colocated(colocated)),
            },
            opened,
        ))
    }

    async fn opened(
        (store, opened): (Self, oneshot::Receiver<Result<(), StoreError>>),
    ) -> Result<Self, StoreError> {
        opened.await.map_err(|_| stopped())??;
        Ok(store)
    }

    fn write<T: Send + 'static>(
        &mut self,
        action: impl FnOnce(&mut Sessions) -> Result<T, StoreError> + Send + 'static,
    ) -> StoreFuture<'_, Result<T, StoreError>> {
        self.dispatch(Reply::Settled, action)
    }

    fn mark<T: Send + 'static>(
        &mut self,
        action: impl FnOnce(&mut Sessions) -> Result<T, StoreError> + Send + 'static,
    ) -> StoreFuture<'_, Result<T, StoreError>> {
        self.dispatch(Reply::Applied, action)
    }

    fn read<T: Send + 'static>(
        &mut self,
        action: impl FnOnce(&mut Sessions) -> Result<T, StoreError> + Send + 'static,
    ) -> StoreFuture<'_, Result<T, StoreError>> {
        self.dispatch(Reply::Wanted, action)
    }

    fn dispatch<T: Send + 'static>(
        &mut self,
        answer: Reply,
        action: impl FnOnce(&mut Sessions) -> Result<T, StoreError> + Send + 'static,
    ) -> StoreFuture<'_, Result<T, StoreError>> {
        let parent = nanocode_trace::Span::current();
        let queued = Instant::now();
        let (reply, result) = oneshot::channel();
        let job: Job = Box::new(move |sessions| {
            if matches!(answer, Reply::Wanted) && reply.is_closed() {
                return None;
            }
            let span = nanocode_trace::info_span!(
                parent: &parent,
                "durability.jsonl.worker",
                queue.duration_ms = queued.elapsed().as_secs_f64() * 1_000.0
            );
            let _entered = span.enter();
            let result = action(sessions);
            if matches!(answer, Reply::Settled) {
                return Some(Box::new(move |settled: Result<(), StoreError>| {
                    drop(reply.send(settled.and(result)));
                }) as Settled);
            }
            drop(reply.send(result));
            None
        });
        Box::pin(async move {
            let stopped = || self.failed.get().cloned().unwrap_or_else(stopped);
            self.jobs.send(job).map_err(|_| stopped())?;
            result.await.map_err(|_| stopped())?
        })
    }
}

impl StoreHandles for JsonlStore {
    fn handle(&self) -> Box<dyn StateStore> {
        Box::new(Self {
            jobs: self.jobs.clone(),
            closed: None,
            failed: Arc::clone(&self.failed),
            colocated: Arc::clone(&self.colocated),
        })
    }
}

impl StateStore for JsonlStore {
    fn colocation(&self) -> Option<Arc<dyn Colocation>> {
        Some(Arc::clone(&self.colocated) as Arc<dyn Colocation>)
    }

    fn pipeline_handle(&self) -> Option<Box<dyn StateStore>> {
        Some(self.handle())
    }

    fn handles(&self) -> Option<Arc<dyn StoreHandles>> {
        Some(Arc::new(Self {
            jobs: self.jobs.clone(),
            closed: None,
            failed: Arc::clone(&self.failed),
            colocated: Arc::clone(&self.colocated),
        }))
    }

    fn acquire_with<'a>(
        &'a mut self,
        id: &'a str,
        owner_id: OwnerId,
        update: StoreUpdate,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        let id = id.to_owned();
        self.write(move |sessions| sessions.acquire_with(&id, owner_id, update))
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
        self.write(move |sessions| {
            sessions.acquire_with_companion(&id, owner_id, update, &companion_id, companion_owner)
        })
    }

    fn read_record<'a>(
        &'a mut self,
        id: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Result<Option<String>, StoreError>> {
        let (id, key) = (id.to_owned(), key.to_owned());
        self.read(move |sessions| sessions.read_record(&id, &key))
    }
    fn read_records<'a>(
        &'a mut self,
        id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>> {
        let (id, keys) = (id.to_owned(), keys.to_vec());
        self.read(move |sessions| sessions.read_records(&id, &keys))
    }
    fn peek<'a>(&'a mut self, id: &'a str) -> StoreFuture<'a, Result<StoredState, StoreError>> {
        let id = id.to_owned();
        self.read(move |sessions| sessions.peek(&id))
    }
    fn owner<'a>(
        &'a mut self,
        id: &'a str,
    ) -> StoreFuture<'a, Result<Option<OwnerToken>, StoreError>> {
        let id = id.to_owned();
        self.read(move |sessions| sessions.owner(&id))
    }
    fn list_states<'a>(
        &'a mut self,
        limit: usize,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        self.read(move |sessions| sessions.list_states(limit))
    }
    fn list_records<'a>(
        &'a mut self,
        id: &'a str,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        let id = id.to_owned();
        self.read(move |sessions| sessions.list_records(&id))
    }
    fn list_summaries<'a>(
        &'a mut self,
        limit: usize,
    ) -> StoreFuture<'a, Result<Option<Vec<ListedState>>, StoreError>> {
        self.read(move |sessions| sessions.list_summaries(limit).map(Some))
    }
    fn remember_summaries<'a>(
        &'a mut self,
        summaries: Vec<KnownSummary>,
    ) -> StoreFuture<'a, Result<(), StoreError>> {
        self.write(move |sessions| sessions.remember_summaries(summaries))
    }
    fn acquire<'a>(
        &'a mut self,
        id: &'a str,
        owner_id: OwnerId,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        let id = id.to_owned();
        self.mark(move |sessions| sessions.acquire(&id, owner_id))
    }
    fn acquire_many(
        &mut self,
        acquisitions: Vec<(String, OwnerId)>,
    ) -> StoreFuture<'_, Vec<Result<OwnedState, StoreError>>> {
        let count = acquisitions.len();
        let acquired = self.mark(move |sessions| Ok(sessions.acquire_many(acquisitions)));
        Box::pin(async move {
            match acquired.await {
                Ok(results) => results,
                Err(error) => vec![Err(error); count],
            }
        })
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
        self.write(move |sessions| sessions.replace(&id, &owner, expected, payload, records))
    }

    fn commit<'a>(
        &'a mut self,
        id: &'a str,
        owner: &'a OwnerToken,
        expected: u64,
        commit: StateCommit<'a>,
    ) -> StoreFuture<'a, Result<Committed, StoreError>> {
        if !commit.events.is_empty() {
            return Box::pin(async {
                Err(StoreError::NotCommitted(
                    "this store does not record state events".to_owned(),
                ))
            });
        }
        let (id, owner, payload, records) = (
            id.to_owned(),
            owner.clone(),
            commit.payload.to_owned(),
            commit.records.to_vec(),
        );
        let replace = move |sessions: &mut Sessions| {
            sessions.replace(&id, &owner, expected, payload, records)
        };
        let replaced = match commit.settle {
            Settle::Durable => self.write(replace),
            Settle::Ordered => self.mark(replace),
        };
        Box::pin(async move {
            Ok(Committed {
                revision: replaced.await?,
                first_event: None,
            })
        })
    }

    fn commit_many(
        &mut self,
        commits: Vec<OwnedCommit>,
    ) -> StoreFuture<'_, Vec<Result<Committed, StoreError>>> {
        let count = commits.len();
        let ordered = commits
            .iter()
            .all(|commit| commit.settle == Settle::Ordered);
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
        let replace = move |sessions: &mut Sessions| Ok(sessions.replace_many(replacements));
        let replaced = if ordered {
            self.mark(replace)
        } else {
            self.write(replace)
        };
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
        self.write(|sessions| {
            sessions.flush();
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
