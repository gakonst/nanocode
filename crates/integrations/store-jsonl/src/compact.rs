use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Seek, SeekFrom, Write},
};

use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};
use serde_json::value::RawValue;
use std::borrow::Cow;

use super::{
    Claim, Counted, IndexedRecord, JOURNAL_READ_BUFFER_BYTES, Journal, Kind, Location, OwnerToken,
    Owners, Record, Spanned, Spans, StoreError, Unit, Verdict,
};
use super::{backend, claim, identity, identity_changed, index::Index, judge, scan, value_digest};
use nanocode_durability_core::OwnerId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompactionStats {
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub records_removed: u64,
}

#[derive(Deserialize)]
struct Header {
    nanocode_journal: Option<String>,
}

#[derive(Deserialize)]
struct Payload<'a> {
    #[serde(borrow)]
    payload: &'a RawValue,
}

#[derive(Default)]
struct Arbiter {
    owners: HashMap<String, OwnerToken>,
    revisions: HashMap<String, u64>,
}

impl Owners for Arbiter {
    fn owner(&self, id: &str) -> Option<&OwnerToken> {
        self.owners.get(id)
    }

    fn revision(&self, id: &str) -> u64 {
        self.revisions.get(id).copied().unwrap_or_default()
    }
}

impl Arbiter {
    fn apply(&mut self, claims: &[Claim<'_>]) {
        for claim in claims {
            match claim.revision {
                None => {
                    self.owners
                        .insert(claim.state_id.to_owned(), claim.owner.clone());
                }
                Some(revision) => {
                    self.revisions.insert(claim.state_id.to_owned(), revision);
                }
            }
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Line<'a> {
    Acquire {
        state_id: &'a str,
        owner: &'a OwnerToken,
        #[serde(skip_serializing_if = "Option::is_none")]
        nanocode_journal: Option<&'a str>,
    },
    Replace {
        state_id: &'a str,
        owner: &'a OwnerToken,
        revision: u64,
        payload: &'a RawValue,
        records: Spanned<'a, [IndexedRecord<'a>]>,
    },
}

impl Serialize for Spanned<'_, [IndexedRecord<'_>]> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.value.iter().map(|value| Spanned {
            value,
            spans: self.spans,
        }))
    }
}

impl Serialize for Spanned<'_, IndexedRecord<'_>> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut record = serializer.serialize_struct("IndexedRecord", 2)?;
        record.serialize_field("key", &self.value.key)?;
        record.serialize_field(
            "value",
            &Spanned {
                value: self.value.value,
                spans: self.spans,
            },
        )?;
        record.end()
    }
}

impl Serialize for Spanned<'_, RawValue> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let start = self.spans.written.get();
        let written = self.value.serialize(serializer)?;
        self.spans
            .values
            .borrow_mut()
            .push(start..self.spans.written.get());
        Ok(written)
    }
}

pub(super) fn lineage(file: &mut File, source: &str) -> Result<String, StoreError> {
    file.seek(SeekFrom::Start(0)).map_err(backend)?;
    let mut line = Vec::new();
    BufReader::new(file)
        .read_until(b'\n', &mut line)
        .map_err(backend)?;
    if line.last() != Some(&b'\n') {
        return Ok(source.into());
    }
    let header: Header = serde_json::from_slice(&line).map_err(backend)?;
    Ok(header.nanocode_journal.unwrap_or_else(|| source.into()))
}

impl Journal {
    pub(super) fn reopen_compacted(&mut self) -> Result<(), StoreError> {
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&self.path)
            .map_err(backend)?;
        let mut reads = File::open(&self.path).map_err(backend)?;
        let source = identity(&file.metadata().map_err(backend)?)?;
        if identity(&reads.metadata().map_err(backend)?)? != source {
            return Ok(());
        }
        if lineage(&mut reads, &source)? != self.lineage {
            return Err(identity_changed());
        }
        self.file = file;
        self.reads = reads;
        self.index = Index::new(source);
        self.persisted_offset = None;
        self.invalid_index = None;
        self.cached = None;
        self.sealed = None;
        self.restore_index();
        Ok(())
    }

    pub(super) fn compact(&mut self) -> Result<CompactionStats, StoreError> {
        let (seal, end) = loop {
            self.writable()?;
            self.clear_temporaries().map_err(backend)?;
            if self.index.offset == 0 {
                return Ok(CompactionStats {
                    before_bytes: 0,
                    after_bytes: 0,
                    records_removed: 0,
                });
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if self.file.metadata().map_err(backend)?.nlink() != 1 {
                    return Err(StoreError::NotCommitted(
                        "cannot compact a journal with hard links".into(),
                    ));
                }
            }
            let seal = OwnerId::new().as_str().to_owned();
            if let Some(end) = self.seal(&seal)? {
                break (seal, end);
            }
        };
        let compacted = self.rebuilt(|store| store.compact_sealed(end));
        if !matches!(compacted, Ok(Some(_)))
            && let Err(error) = self.append_marker(&super::Line::Unseal { seal: &seal })
        {
            tracing::warn!(%error, "could not release the JSONL compaction seal");
            return Err(compacted.err().unwrap_or(error));
        }
        let Some(stats) = compacted? else {
            return Ok(CompactionStats {
                before_bytes: end,
                after_bytes: end,
                records_removed: 0,
            });
        };
        self.refresh_index()?;
        self.persist_index();
        Ok(stats)
    }

    fn compact_sealed(&mut self, end: u64) -> Result<Option<CompactionStats>, StoreError> {
        let span = tracing::info_span!(
            "durability.jsonl.compact",
            before_bytes = end,
            after_bytes = tracing::field::Empty,
            records_removed = tracing::field::Empty
        );
        let _entered = span.enter();
        let metadata = self.file.metadata().map_err(backend)?;
        let mut stats = CompactionStats {
            before_bytes: end,
            after_bytes: end,
            records_removed: 0,
        };
        let directory = self
            .path
            .parent()
            .ok_or_else(|| backend("journal has no directory"))?;
        let directory = directory.to_owned();
        let mut temporary = self.temporary(".compact-").map_err(backend)?;
        temporary
            .as_file()
            .set_permissions(metadata.permissions())
            .map_err(backend)?;
        let empty = RawValue::from_string("\"\"".into()).map_err(backend)?;
        let mut input = BufReader::with_capacity(
            JOURNAL_READ_BUFFER_BYTES,
            File::open(&self.path).map_err(backend)?,
        );
        let mut output =
            BufWriter::with_capacity(JOURNAL_READ_BUFFER_BYTES, temporary.as_file_mut());
        let mut moved = Vec::new();
        let mut converted = HashMap::new();
        let mut current = HashMap::with_capacity(self.index.states.len());
        let mut encoded = Vec::new();
        let mut written = 0_u64;
        let mut last = None;
        let mut arbiter = Arbiter::default();
        let mut sealed: Option<String> = None;
        scan(&mut input, 0, end, true, |unit, _| {
            let Unit::Entries(lines) = unit else {
                return Ok(());
            };
            if let [(_, entry, _)] = lines.as_slice()
                && matches!(entry.kind, Kind::Seal | Kind::Unseal)
            {
                let seal = entry.seal.as_deref().map(str::to_owned);
                match entry.kind {
                    Kind::Seal if sealed.is_none() => sealed = seal,
                    Kind::Unseal if sealed.is_some() && sealed == seal => sealed = None,
                    _ => {}
                }
                return Ok(());
            }
            if sealed.is_some() {
                return Ok(());
            }
            let claims = lines
                .iter()
                .map(|(_, entry, _)| claim(entry))
                .collect::<Result<Vec<_>, _>>()?;
            if matches!(judge(&arbiter, &claims)?, Verdict::Stale) {
                return Ok(());
            }
            arbiter.apply(&claims);
            for (location, entry, line) in &lines {
                let position = location.position;
                let id = entry
                    .state_id
                    .as_deref()
                    .ok_or_else(|| backend("missing state"))?;
                let owner = entry
                    .owner
                    .as_ref()
                    .ok_or_else(|| backend("missing owner"))?;
                let spans = Spans::default();
                encoded.clear();
                let counted = Counted {
                    bytes: &mut encoded,
                    spans: &spans,
                };
                match entry.kind {
                    Kind::Acquire => {
                        serde_json::to_writer(
                            counted,
                            &Line::Acquire {
                                state_id: id,
                                owner,
                                nanocode_journal: (written == 0).then_some(self.lineage.as_str()),
                            },
                        )
                        .map_err(backend)?;
                    }
                    Kind::Replace => {
                        if entry.payload.is_none() {
                            return Err(backend("missing payload"));
                        }
                        let revision = entry.revision.ok_or_else(|| backend("missing revision"))?;
                        let original = if self
                            .index
                            .states
                            .get(id)
                            .is_some_and(|state| state.location == *location)
                        {
                            Some(serde_json::from_slice::<Payload<'_>>(line).map_err(backend)?)
                        } else {
                            None
                        };
                        let mut keys = HashSet::new();
                        let mut records = Vec::new();
                        let mut sources = Vec::new();
                        for record in entry
                            .records
                            .as_ref()
                            .ok_or_else(|| backend("missing records"))?
                        {
                            let retained = match self.location(id, &record.key)? {
                                Some(Record::Entry(indexed)) => (indexed == *location
                                    && keys.insert(record.key.to_string()))
                                .then(|| {
                                    (
                                        Record::Entry(indexed),
                                        value_digest(
                                            id,
                                            &record.key,
                                            record.value.get().as_bytes(),
                                        ),
                                    )
                                }),
                                Some(Record::Value {
                                    location: value,
                                    digest,
                                }) => {
                                    let current = Record::located(
                                        id,
                                        &record.key,
                                        line,
                                        position,
                                        record.value,
                                    );
                                    if current.location() == value {
                                        if current
                                            != (Record::Value {
                                                location: value,
                                                digest,
                                            })
                                        {
                                            return Err(self
                                                .invalidated("record changed during compaction"));
                                        }
                                        Some((current, digest))
                                    } else {
                                        None
                                    }
                                }
                                None => {
                                    return Err(
                                        self.invalidated("record missing during compaction")
                                    );
                                }
                            };
                            match retained {
                                Some(source) => {
                                    records.push(IndexedRecord {
                                        key: Cow::Borrowed(&record.key),
                                        value: record.value,
                                    });
                                    sources.push(source);
                                }
                                None => stats.records_removed += 1,
                            }
                        }
                        serde_json::to_writer(
                            counted,
                            &Line::Replace {
                                state_id: id,
                                owner,
                                revision,
                                payload: original
                                    .as_ref()
                                    .map_or(empty.as_ref(), |value| value.payload),
                                records: Spanned {
                                    value: &records,
                                    spans: &spans,
                                },
                            },
                        )
                        .map_err(backend)?;
                        if original.is_some() {
                            current.insert(
                                position,
                                Location {
                                    position: written,
                                    length: encoded.len() + 1,
                                },
                            );
                        }
                        for ((record, (source, digest)), span) in
                            records.iter().zip(sources).zip(spans.values.take())
                        {
                            let target = Location {
                                position: written + span.start as u64,
                                length: span.len(),
                            };
                            match source {
                                Record::Value { location, .. } => {
                                    moved.push((location.position, target));
                                }
                                Record::Entry(entry) => {
                                    converted.insert(
                                        (entry.position, record.key.to_string()),
                                        Record::Value {
                                            location: target,
                                            digest,
                                        },
                                    );
                                }
                            }
                        }
                    }
                    Kind::Seal | Kind::Unseal => {
                        return Err(backend("JSONL seal inside an entry batch"));
                    }
                }
                encoded.push(b'\n');
                output.write_all(&encoded).map_err(backend)?;
                last = Some(Location {
                    position: written,
                    length: encoded.len(),
                });
                written += encoded.len() as u64;
            }
            Ok(())
        })?;
        output.flush().map_err(backend)?;
        drop(output);
        if written >= stats.before_bytes {
            return Ok(None);
        }
        temporary.as_file().sync_all().map_err(backend)?;
        let persisted = temporary
            .persist(&self.path)
            .map_err(|error| backend(error.error))?;
        #[cfg(unix)]
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(backend)?;
        let source = identity(&persisted.metadata().map_err(backend)?)?;
        drop(persisted);
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&self.path)
            .map_err(backend)?;
        let reads = File::open(&self.path).map_err(backend)?;
        if identity(&file.metadata().map_err(backend)?)? != source
            || identity(&reads.metadata().map_err(backend)?)? != source
        {
            return Err(identity_changed());
        }
        self.file = file;
        self.reads = reads;
        self.sealed = None;
        self.persisted_offset = None;
        self.invalid_index = None;
        self.cached = None;
        let relocated = self
            .index
            .relocate(written, last, &current, |key, record| match record {
                Record::Value { location, digest } => moved
                    .binary_search_by_key(&location.position, |(position, _)| *position)
                    .ok()
                    .map(|found| Record::Value {
                        location: moved[found].1,
                        digest,
                    }),
                Record::Entry(entry) => converted.remove(&(entry.position, key.to_owned())),
            });
        self.index.source = source;
        if let Err(error) = relocated {
            self.discard_index(error);
        }
        stats.after_bytes = written;
        span.record("after_bytes", written);
        span.record("records_removed", stats.records_removed);
        Ok(Some(stats))
    }
}
