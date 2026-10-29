use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

use nanocode_durability_core::{OwnerToken, StateSummary};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Journal, Location, Record, State, identity};

const MAGIC: &[u8] = b"nanocode-jsonl-index-v3\n";
const ENTRY_MAGIC: &[u8] = b"nanocode-jsonl-index-v2\n";
const LEGACY_MAGIC: &[u8] = b"nanocode-jsonl-index-v1\n";
const INDEX_COPY_BUFFER_BYTES: usize = 128 * 1024;
const ENTRY_RECORD: u8 = 0;
const VALUE_RECORD: u8 = 1;

pub(super) struct Index {
    pub(super) source: String,
    pub(super) offset: u64,
    pub(super) owners: HashMap<String, OwnerToken>,
    pub(super) states: HashMap<String, State>,
    pub(super) created: Vec<String>,
    pub(super) last: Option<Location>,
    pub(super) migrated: bool,
    anchor: [u8; 32],
    deferred: HashMap<String, RecordBlock>,
    summaries: Option<HashMap<String, (u64, Option<StateSummary>)>>,
    deferred_summaries: Option<RecordBlock>,
    file: Option<File>,
    saved: Option<String>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct RecordBlock {
    location: Location,
    digest: [u8; 32],
    format: Option<BlockFormat>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BlockFormat {
    Entries,
    Records,
}

#[derive(Serialize, Deserialize)]
struct SavedState {
    revision: u64,
    location: Location,
    records: RecordBlock,
}

#[derive(Serialize, Deserialize)]
struct SavedIndex {
    source: String,
    offset: u64,
    owners: HashMap<String, OwnerToken>,
    states: HashMap<String, SavedState>,
    created: Vec<String>,
    last: Option<Location>,
    anchor: [u8; 32],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    summaries: Option<RecordBlock>,
}

#[derive(Deserialize)]
struct LegacyState {
    revision: u64,
    location: Location,
    records: HashMap<String, Location>,
}

#[derive(Deserialize)]
struct LegacyIndex {
    source: String,
    offset: u64,
    owners: HashMap<String, OwnerToken>,
    states: HashMap<String, LegacyState>,
    created: Vec<String>,
    last: Option<Location>,
    anchor: [u8; 32],
}

impl Index {
    pub(super) fn new(source: String) -> Self {
        Self {
            source,
            offset: 0,
            owners: HashMap::new(),
            states: HashMap::new(),
            created: Vec::new(),
            last: None,
            migrated: false,
            anchor: Sha256::digest([]).into(),
            deferred: HashMap::new(),
            summaries: None,
            deferred_summaries: None,
            file: None,
            saved: None,
        }
    }

    pub(super) fn load_summaries(&mut self) -> &mut HashMap<String, (u64, Option<StateSummary>)> {
        if self.summaries.is_none() {
            let loaded = self.deferred_summaries.take().map(|block| {
                let file = self.file.as_mut().ok_or_else(invalid)?;
                let bytes = read_block(file, block)?;
                serde_json::from_slice::<Vec<(String, u64, Option<StateSummary>)>>(&bytes)
                    .map_err(io::Error::other)
            });
            let summaries = match loaded {
                Some(Ok(summaries)) => summaries
                    .into_iter()
                    .map(|(id, revision, summary)| (id, (revision, summary)))
                    .collect(),
                Some(Err(error)) => {
                    tracing::warn!(%error, "dropping unreadable JSONL session summaries");
                    HashMap::new()
                }
                None => HashMap::new(),
            };
            self.summaries = Some(summaries);
        }
        self.summaries.get_or_insert_with(HashMap::new)
    }

    pub(super) fn relocate(
        &mut self,
        offset: u64,
        last: Option<Location>,
        entries: &HashMap<u64, Location>,
        mut relocated: impl FnMut(&str, Record) -> Option<Record>,
    ) -> io::Result<()> {
        let deferred: Vec<String> = self.deferred.keys().cloned().collect();
        for id in deferred {
            self.load_records(&id)?;
        }
        self.load_summaries();
        self.file = None;
        self.migrated = false;
        self.offset = offset;
        self.last = last;
        for state in self.states.values_mut() {
            state.location = entries
                .get(&state.location.position)
                .copied()
                .ok_or_else(invalid)?;
            for (key, record) in &mut state.records {
                *record = relocated(key, *record).ok_or_else(invalid)?;
            }
        }
        Ok(())
    }

    pub(super) fn deferred_bytes(&self, id: &str) -> usize {
        self.deferred
            .get(id)
            .map_or(0, |block| block.location.length)
    }

    pub(super) fn load_records(&mut self, id: &str) -> io::Result<()> {
        let Some(block) = self.deferred.get(id).copied() else {
            return Ok(());
        };
        let _span = tracing::info_span!("durability.jsonl.index.records").entered();
        let file = self.file.as_mut().ok_or_else(invalid)?;
        let bytes = read_block(file, block)?;
        let records = match block.format {
            Some(BlockFormat::Entries) => {
                serde_json::from_slice::<HashMap<String, Location>>(&bytes)
                    .map_err(io::Error::other)?
                    .into_iter()
                    .map(|(key, location)| (key, Record::Entry(location)))
                    .collect()
            }
            Some(BlockFormat::Records) => decode_records(&bytes)?,
            None => return Err(invalid()),
        };
        if !records
            .values()
            .all(|record| valid_location(record.location(), self.offset))
        {
            return Err(invalid());
        }
        self.states.get_mut(id).ok_or_else(invalid)?.records = records;
        self.deferred.remove(id);
        Ok(())
    }

    fn valid(&self, source: &str, length: u64) -> bool {
        let location_valid = |location| valid_location(location, self.offset);
        self.source == source
            && self.offset <= length
            && self.last.map_or(self.offset == 0, |last| {
                location_valid(last)
                    && last.position.checked_add(last.length as u64) == Some(self.offset)
            })
            && self.created.len() == self.states.len()
            && self.created.iter().collect::<HashSet<_>>().len() == self.created.len()
            && self.created.iter().all(|id| self.states.contains_key(id))
            && self.states.iter().all(|(id, state)| {
                self.owners.contains_key(id)
                    && state.revision > 0
                    && location_valid(state.location)
                    && state
                        .records
                        .values()
                        .all(|record| location_valid(record.location()))
            })
            && self.owners.values().all(|owner| owner.fence() > 0)
    }
}

impl Journal {
    fn index_path(&self) -> PathBuf {
        self.path.with_added_extension("index")
    }

    pub(super) fn restore_index(&mut self) {
        let _span = tracing::info_span!("durability.jsonl.index.load").entered();
        match self.read_index() {
            Ok(Some(index)) => {
                tracing::info!(
                    indexed_bytes = index.offset,
                    states = index.states.len(),
                    "reused JSONL index"
                );
                self.persisted_offset = index.file.as_ref().map(|_| index.offset);
                self.index = index;
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "rebuilding invalid JSONL index from journal"),
        }
    }

    fn read_index(&mut self) -> io::Result<Option<Index>> {
        let mut file = match File::open(self.index_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        let file_identity = identity(&metadata).map_err(io::Error::other)?;
        let length = metadata.len();
        let trailer = Sha256::output_size() + size_of::<u64>();
        let mut magic = vec![0; MAGIC.len()];
        file.read_exact(&mut magic)?;
        if magic == LEGACY_MAGIC {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            let (checksum, payload) = bytes
                .split_at_checked(Sha256::output_size())
                .ok_or_else(invalid)?;
            if checksum != &Sha256::digest(payload)[..] {
                return Err(invalid());
            }
            let saved: LegacyIndex = serde_json::from_slice(payload).map_err(io::Error::other)?;
            let mut index = Index::new(saved.source);
            index.offset = saved.offset;
            index.owners = saved.owners;
            index.states = saved
                .states
                .into_iter()
                .map(|(id, state)| {
                    let records = state
                        .records
                        .into_iter()
                        .map(|(key, location)| (key, Record::Entry(location)))
                        .collect();
                    (
                        id,
                        State {
                            revision: state.revision,
                            location: state.location,
                            records,
                        },
                    )
                })
                .collect();
            index.created = saved.created;
            index.last = saved.last;
            index.anchor = saved.anchor;
            return self.checked_index(index, file_identity);
        }
        let format = match &*magic {
            MAGIC => None,
            ENTRY_MAGIC => Some(BlockFormat::Entries),
            _ => return Err(invalid()),
        };
        if length < (MAGIC.len() + trailer) as u64 {
            return Err(invalid());
        }
        file.seek(SeekFrom::End(-(trailer as i64)))?;
        let mut checksum = [0; 32];
        let mut encoded_length = [0; 8];
        file.read_exact(&mut checksum)?;
        file.read_exact(&mut encoded_length)?;
        let header_length = u64::from_le_bytes(encoded_length);
        let header_start = length
            .checked_sub(trailer as u64)
            .and_then(|end| end.checked_sub(header_length))
            .filter(|start| *start >= MAGIC.len() as u64)
            .ok_or_else(invalid)?;
        file.seek(SeekFrom::Start(header_start))?;
        let mut payload = vec![0; usize::try_from(header_length).map_err(io::Error::other)?];
        file.read_exact(&mut payload)?;
        if checksum != Sha256::digest(&payload)[..] {
            return Err(invalid());
        }
        let saved: SavedIndex = serde_json::from_slice(&payload).map_err(io::Error::other)?;
        let mut index = Index::new(saved.source);
        index.offset = saved.offset;
        index.owners = saved.owners;
        index.created = saved.created;
        index.last = saved.last;
        index.anchor = saved.anchor;
        for (id, mut state) in saved.states {
            if state.records.location.position < MAGIC.len() as u64
                || state
                    .records
                    .location
                    .position
                    .checked_add(state.records.location.length as u64)
                    .is_none_or(|end| end > header_start)
            {
                return Err(invalid());
            }
            state.records.format = match (format, state.records.format) {
                (None, Some(format)) | (Some(format), None) => Some(format),
                _ => return Err(invalid()),
            };
            index.deferred.insert(id.clone(), state.records);
            index.states.insert(
                id,
                State {
                    revision: state.revision,
                    location: state.location,
                    records: HashMap::new(),
                },
            );
        }
        if let Some(block) = saved.summaries {
            if block.location.position < MAGIC.len() as u64
                || block
                    .location
                    .position
                    .checked_add(block.location.length as u64)
                    .is_none_or(|end| end > header_start)
            {
                return Err(invalid());
            }
            index.deferred_summaries = Some(block);
        }
        index.file = Some(file);
        self.checked_index(index, file_identity)
    }

    fn checked_index(&mut self, mut index: Index, saved: String) -> io::Result<Option<Index>> {
        if !index.valid(&self.index.source, self.file.metadata()?.len())
            || digest_anchor(&mut self.reads, index.last)? != index.anchor
        {
            return Err(invalid());
        }
        index.saved = Some(saved);
        Ok(Some(index))
    }

    pub(super) fn persist_index(&mut self) {
        if self.sealed.is_some() {
            return;
        }
        if std::fs::metadata(&self.path)
            .ok()
            .and_then(|metadata| identity(&metadata).ok())
            .as_deref()
            != Some(self.index.source.as_str())
        {
            return;
        }
        if self.persisted_offset == Some(self.index.offset) && !self.index.migrated {
            return;
        }
        let _span = tracing::info_span!(
            "durability.jsonl.index.save",
            indexed_bytes = self.index.offset
        )
        .entered();
        if self.persisted_offset.is_some()
            && !self.index_file_unchanged()
            && let Ok(Some(index)) = self.read_index()
            && index.offset >= self.index.offset
        {
            tracing::debug!("retaining a current JSONL index saved by another store");
            self.persisted_offset = Some(index.offset);
            self.index.saved = index.saved;
            return;
        }
        match self.write_index() {
            Ok(()) => {
                self.persisted_offset = Some(self.index.offset);
                self.index.migrated = false;
            }
            Err(error) => tracing::warn!(%error, "could not save rebuildable JSONL index"),
        }
    }

    fn index_file_unchanged(&self) -> bool {
        self.index.saved.is_some()
            && std::fs::metadata(self.index_path())
                .ok()
                .and_then(|metadata| identity(&metadata).ok())
                == self.index.saved
    }

    fn write_index(&mut self) -> io::Result<()> {
        self.index.anchor = digest_anchor(&mut self.reads, self.index.last)?;
        let path = self.index_path();
        let mut temporary = self.temporary(".index-")?;
        temporary.write_all(MAGIC)?;
        let mut states = HashMap::with_capacity(self.index.states.len());
        for (id, state) in &self.index.states {
            let position = temporary.stream_position()?;
            let mut output = DigestWriter {
                output: &mut temporary,
                digest: Sha256::new(),
                length: 0,
            };
            let format = if let Some(block) = self.index.deferred.get(id) {
                let file = self.index.file.as_mut().ok_or_else(invalid)?;
                file.seek(SeekFrom::Start(block.location.position))?;
                let mut source = BufReader::with_capacity(
                    INDEX_COPY_BUFFER_BYTES,
                    file.take(block.location.length as u64),
                );
                io::copy(&mut source, &mut output)?;
                if output.length != block.location.length
                    || output.digest.clone().finalize()[..] != block.digest
                {
                    return Err(invalid());
                }
                block.format
            } else {
                let mut buffered =
                    io::BufWriter::with_capacity(INDEX_COPY_BUFFER_BYTES, &mut output);
                encode_records(&state.records, &mut buffered)?;
                buffered.flush()?;
                Some(BlockFormat::Records)
            };
            let records = RecordBlock {
                location: Location {
                    position,
                    length: output.length,
                },
                digest: output.digest.finalize().into(),
                format,
            };
            states.insert(
                id.clone(),
                SavedState {
                    revision: state.revision,
                    location: state.location,
                    records,
                },
            );
        }
        let summaries = if self.index.summaries.is_some() || self.index.deferred_summaries.is_some()
        {
            let position = temporary.stream_position()?;
            let mut output = DigestWriter {
                output: &mut temporary,
                digest: Sha256::new(),
                length: 0,
            };
            if let Some(summaries) = &self.index.summaries {
                let mut buffered =
                    io::BufWriter::with_capacity(INDEX_COPY_BUFFER_BYTES, &mut output);
                serde_json::to_writer(
                    &mut buffered,
                    &summaries
                        .iter()
                        .map(|(id, (revision, summary))| (id, revision, summary))
                        .collect::<Vec<_>>(),
                )
                .map_err(io::Error::other)?;
                buffered.flush()?;
            } else if let Some(block) = self.index.deferred_summaries {
                let file = self.index.file.as_mut().ok_or_else(invalid)?;
                file.seek(SeekFrom::Start(block.location.position))?;
                let mut source = BufReader::with_capacity(
                    INDEX_COPY_BUFFER_BYTES,
                    file.take(block.location.length as u64),
                );
                io::copy(&mut source, &mut output)?;
                if output.length != block.location.length
                    || output.digest.clone().finalize()[..] != block.digest
                {
                    return Err(invalid());
                }
            }
            Some(RecordBlock {
                location: Location {
                    position,
                    length: output.length,
                },
                digest: output.digest.finalize().into(),
                format: None,
            })
        } else {
            None
        };
        let saved = SavedIndex {
            source: self.index.source.clone(),
            offset: self.index.offset,
            owners: self.index.owners.clone(),
            states,
            created: self.index.created.clone(),
            last: self.index.last,
            anchor: self.index.anchor,
            summaries,
        };
        let payload = serde_json::to_vec(&saved).map_err(io::Error::other)?;
        temporary.write_all(&payload)?;
        temporary.write_all(&Sha256::digest(&payload))?;
        temporary.write_all(&(payload.len() as u64).to_le_bytes())?;
        let file = temporary.persist(path).map_err(|error| error.error)?;
        self.index.saved = Some(identity(&file.metadata()?).map_err(io::Error::other)?);
        Ok(())
    }
}

struct DigestWriter<'a, W> {
    output: &'a mut W,
    digest: Sha256,
    length: usize,
}

impl<W: Write> Write for DigestWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = self.output.write(bytes)?;
        self.digest.update(&bytes[..length]);
        self.length += length;
        Ok(length)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "JSONL index does not match its journal",
    )
}

fn valid_location(location: Location, end: u64) -> bool {
    location.length > 0
        && location
            .position
            .checked_add(location.length as u64)
            .is_some_and(|position| position <= end)
}

fn encode_records(records: &HashMap<String, Record>, output: &mut impl Write) -> io::Result<()> {
    for (key, record) in records {
        output.write_all(&(key.len() as u64).to_le_bytes())?;
        output.write_all(key.as_bytes())?;
        let (tag, location, digest) = match record {
            Record::Entry(location) => (ENTRY_RECORD, location, None),
            Record::Value { location, digest } => (VALUE_RECORD, location, Some(digest)),
        };
        output.write_all(&[tag])?;
        output.write_all(&location.position.to_le_bytes())?;
        output.write_all(&(location.length as u64).to_le_bytes())?;
        if let Some(digest) = digest {
            output.write_all(digest)?;
        }
    }
    Ok(())
}

fn decode_records(mut bytes: &[u8]) -> io::Result<HashMap<String, Record>> {
    let mut records = HashMap::new();
    while !bytes.is_empty() {
        let length = usize::try_from(take_u64(&mut bytes)?).map_err(io::Error::other)?;
        let key = std::str::from_utf8(take(&mut bytes, length)?).map_err(io::Error::other)?;
        let tag = take(&mut bytes, 1)?[0];
        let location = Location {
            position: take_u64(&mut bytes)?,
            length: usize::try_from(take_u64(&mut bytes)?).map_err(io::Error::other)?,
        };
        let record = match tag {
            ENTRY_RECORD => Record::Entry(location),
            VALUE_RECORD => Record::Value {
                location,
                digest: take(&mut bytes, Sha256::output_size())?
                    .try_into()
                    .map_err(io::Error::other)?,
            },
            _ => return Err(invalid()),
        };
        if records.insert(key.to_owned(), record).is_some() {
            return Err(invalid());
        }
    }
    Ok(records)
}

fn take<'a>(bytes: &mut &'a [u8], length: usize) -> io::Result<&'a [u8]> {
    let (taken, rest) = bytes.split_at_checked(length).ok_or_else(invalid)?;
    *bytes = rest;
    Ok(taken)
}

fn take_u64(bytes: &mut &[u8]) -> io::Result<u64> {
    Ok(u64::from_le_bytes(
        take(bytes, size_of::<u64>())?
            .try_into()
            .map_err(io::Error::other)?,
    ))
}

fn read_block(file: &mut File, block: RecordBlock) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(block.location.position))?;
    let mut bytes = vec![0; block.location.length];
    file.read_exact(&mut bytes)?;
    if Sha256::digest(&bytes)[..] != block.digest {
        return Err(invalid());
    }
    Ok(bytes)
}

impl Journal {
    pub(super) fn discard_index(&mut self, error: impl std::fmt::Display) {
        tracing::warn!(%error, "rebuilding invalid JSONL index from journal");
        self.index = Index::new(self.index.source.clone());
        self.persisted_offset = None;
        self.cached = None;
        self.sealed = None;
    }

    pub(super) fn refresh_index(&mut self) -> Result<(), nanocode_durability_core::StoreError> {
        let result = self.refresh();
        if let Some(error) = self.invalid_index.take() {
            self.discard_index(error);
            self.refresh()
        } else {
            result
        }
    }

    pub(super) fn adopt_saved_index(&mut self) {
        if !self.ready || self.index_file_unchanged() {
            return;
        }
        let (Ok(journal), Ok(saved)) =
            (self.reads.metadata(), std::fs::metadata(self.index_path()))
        else {
            return;
        };
        let unindexed = journal.len().saturating_sub(self.index.offset);
        if unindexed <= saved.len() {
            return;
        }
        let _span = tracing::info_span!(
            "durability.jsonl.index.adopt",
            log.unindexed_bytes = unindexed
        )
        .entered();
        match self.read_index() {
            Ok(Some(index)) if index.offset > self.index.offset => {
                self.persisted_offset = index.file.as_ref().map(|_| index.offset);
                self.index = index;
                self.cached = None;
                self.sealed = None;
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, "replaying the journal past an invalid JSONL index")
            }
        }
    }
}

impl Journal {
    pub(super) fn flush_index(&mut self) {
        if self.ready && (self.persisted_offset != Some(self.index.offset) || self.index.migrated) {
            self.persist_index();
        }
    }

    pub(super) fn flush_lagging_index(&mut self) {
        let lag = self
            .index
            .offset
            .saturating_sub(self.persisted_offset.unwrap_or(0));
        let saved = std::fs::metadata(self.index_path()).map_or(0, |metadata| metadata.len());
        if lag > saved {
            self.flush_index();
        }
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        self.flush_index();
    }
}

fn digest_anchor(file: &mut File, location: Option<Location>) -> io::Result<[u8; 32]> {
    let mut digest = Sha256::new();
    if let Some(location) = location {
        file.seek(SeekFrom::Start(location.position))?;
        let mut reader = BufReader::new(file.take(location.length as u64));
        let mut length = 0;
        loop {
            let bytes = reader.fill_buf()?;
            if bytes.is_empty() {
                break;
            }
            digest.update(bytes);
            let count = bytes.len();
            length += count;
            reader.consume(count);
        }
        if length != location.length {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "JSONL index anchor was truncated",
            ));
        }
    }
    Ok(digest.finalize().into())
}
