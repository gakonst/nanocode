use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File},
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};

use nanocode_durability_core::{OwnerId, OwnerToken, StateSummary, StoreError, StoreRecord};
use serde::{Deserialize, Serialize};

use super::{
    Entry, JOURNAL_READ_BUFFER_BYTES, Journal, Record, SHARED_RECORD, Spans, backend, encode,
    identity,
    sessions::{Sessions, session_key},
    summaries::Summary,
    sync_journal, value_digest,
};

const MARKER: &str = ".legacy.json";
const IMPORT_OWNER: &str = "legacy-import-";
const LINE_BYTES: usize = 4 * 1024 * 1024;
const MARKER_INTERVAL: usize = 32;
const HEADROOM_BYTES: u64 = 1 << 30;
const READ_GAP_BYTES: u64 = 1 << 20;
const READ_BATCH_BYTES: u64 = 16 << 20;

#[derive(Default, Deserialize, Serialize)]
struct Marker {
    source: String,
    offset: u64,
    states: HashMap<String, u64>,
}

struct Fresh {
    states: Vec<(String, u64)>,
    file: String,
    length: u64,
}

fn import_owner() -> OwnerId {
    OwnerId::from(format!("{IMPORT_OWNER}{}", OwnerId::new().as_str()))
}

fn imported(owner: &OwnerToken) -> bool {
    owner.owner_id().as_str().starts_with(IMPORT_OWNER)
}

fn stored(value: String) -> String {
    if value.starts_with(SHARED_RECORD) {
        format!("{SHARED_RECORD}{value}")
    } else {
        value
    }
}

fn read_marker(path: &Path) -> Result<Marker, StoreError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(backend),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Marker::default()),
        Err(error) => Err(backend(error)),
    }
}

fn write_marker(directory: &Path, marker: &Marker) -> Result<(), StoreError> {
    #[cfg(unix)]
    File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(backend)?;
    let mut temporary =
        nanocode_platform::TemporaryFile::create_in(directory, MARKER, None).map_err(backend)?;
    serde_json::to_writer(BufWriter::new(temporary.as_file_mut()), marker).map_err(backend)?;
    sync_journal(temporary.as_file()).map_err(backend)?;
    temporary
        .persist(directory.join(MARKER))
        .map_err(|error| backend(error.error))?;
    Ok(())
}

#[cfg(unix)]
fn available(directory: &Path) -> Option<u64> {
    nanocode_platform::unix::available_bytes(directory).ok()
}

#[cfg(not(unix))]
const fn available(_: &Path) -> Option<u64> {
    None
}

fn legacy_bytes(journal: &mut Journal, ids: &[String]) -> Result<u64, StoreError> {
    let mut bytes = 0_u64;
    for id in ids {
        journal.rebuilt(|journal| journal.record_keys(id).map(drop))?;
        bytes = bytes.saturating_add(journal.index.states.get(id).map_or(0, |state| {
            state
                .records
                .values()
                .map(|record| record.location().length as u64)
                .sum::<u64>()
                .saturating_add(state.location.length as u64)
        }));
    }
    Ok(bytes)
}

fn sorted_keys(journal: &mut Journal, id: &str) -> Result<Vec<String>, StoreError> {
    let mut keys = journal.rebuilt(|journal| journal.record_keys(id))?;
    if let Some(state) = journal.index.states.get(id) {
        keys.sort_by_cached_key(|key| {
            state
                .records
                .get(key)
                .map_or(u64::MAX, |record| record.location().position)
        });
    }
    Ok(keys)
}

fn legacy_records(
    journal: &mut Journal,
    id: &str,
    keys: Vec<String>,
    mut each: impl FnMut(StoreRecord) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let located: Vec<(String, Option<Record>)> = {
        let records = journal.index.states.get(id).map(|state| &state.records);
        keys.into_iter()
            .map(|key| {
                let record = records.and_then(|records| records.get(&key)).copied();
                (key, record)
            })
            .collect()
    };
    let mut buffer = Vec::new();
    let mut index = 0;
    while index < located.len() {
        let Some(Record::Value {
            location: first, ..
        }) = located[index].1
        else {
            let key = located[index].0.clone();
            let value = legacy_record(journal, id, &key)?;
            each(StoreRecord { key, value })?;
            index += 1;
            continue;
        };
        let start = first.position;
        let mut end = start + first.length as u64;
        let mut last = index + 1;
        while let Some((_, Some(Record::Value { location, .. }))) = located.get(last)
            && location.position >= start
            && location.position <= end.saturating_add(READ_GAP_BYTES)
            && location.position + location.length as u64 - start <= READ_BATCH_BYTES
        {
            end = end.max(location.position + location.length as u64);
            last += 1;
        }
        buffer.resize(usize::try_from(end - start).map_err(backend)?, 0);
        journal
            .reads
            .seek(SeekFrom::Start(start))
            .and_then(|_| journal.reads.read_exact(&mut buffer))
            .map_err(backend)?;
        for (key, record) in &located[index..last] {
            let Some(Record::Value { location, digest }) = record else {
                continue;
            };
            let offset = usize::try_from(location.position - start).map_err(backend)?;
            let bytes = &buffer[offset..offset + location.length];
            if value_digest(id, key, bytes) != *digest {
                return Err(backend(format!(
                    "legacy JSONL record {key} of {id} does not match its journal"
                )));
            }
            let value: String = serde_json::from_slice(bytes).map_err(backend)?;
            let value = if value.starts_with(SHARED_RECORD) {
                legacy_record(journal, id, key)?
            } else {
                value
            };
            each(StoreRecord {
                key: key.clone(),
                value,
            })?;
        }
        index = last;
    }
    Ok(())
}

fn write_line(output: &mut impl Write, entry: Entry) -> Result<(), StoreError> {
    let (bytes, _) = encode(&[entry], None, &Spans::default())?;
    output.write_all(&bytes).map_err(backend)
}

pub(super) fn migrate(sessions: &mut Sessions, legacy: &Path) -> Result<(), StoreError> {
    let metadata = fs::metadata(legacy).map_err(backend)?;
    let source = identity(&metadata)?;
    let directory = sessions.directory().to_owned();
    let mut marker = read_marker(&directory.join(MARKER))?;
    if marker.source == source && marker.offset == metadata.len() {
        return Ok(());
    }
    let span = nanocode_trace::info_span!(
        "durability.jsonl.migrate",
        legacy.bytes = metadata.len(),
        sessions = nanocode_trace::field::Empty,
        states = nanocode_trace::field::Empty
    );
    let _entered = span.enter();
    let mut journal = Journal::open(legacy, true)?;
    let offset = journal.index.offset;
    let summaries = journal.index.load_summaries().clone();
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for id in &journal.index.created {
        groups
            .entry(session_key(id).to_owned())
            .or_default()
            .push(id.clone());
    }
    let migrated: HashSet<String> = marker
        .states
        .keys()
        .map(|id| session_key(id).to_owned())
        .collect();
    let (mut moved_sessions, mut moved_states) = (0_usize, 0_usize);
    let mut remembered = Vec::new();
    for (key, ids) in groups {
        let changed: Vec<String> = ids
            .iter()
            .filter(|id| {
                let revision = journal.revision(id);
                revision > 0 && marker.states.get(*id) != Some(&revision)
            })
            .cloned()
            .collect();
        if changed.is_empty() {
            continue;
        }
        let path = sessions.path(&key);
        if !path.is_file()
            && !migrated.contains(&key)
            && let Some(free) = available(&directory)
        {
            let needed = legacy_bytes(&mut journal, &ids)?;
            if free < needed.saturating_add(HEADROOM_BYTES) {
                write_marker(&directory, &marker)?;
                sessions.remember(remembered)?;
                return Err(backend(format!(
                    "moving session {key} out of {} into its own file needs {needed} bytes and leaves less than {HEADROOM_BYTES} free ({free} free); free some disk space and start again",
                    legacy.display()
                )));
            }
        }
        if path.is_file() {
            remembered.extend(catch_up(
                sessions,
                &mut journal,
                &changed,
                &mut marker,
                &summaries,
            )?);
        } else if migrated.contains(&key) {
            continue;
        } else {
            match fresh(&directory, &path, &mut journal, &ids)? {
                Some(Fresh {
                    states,
                    file,
                    length,
                }) => {
                    for (id, revision) in states {
                        if let Some(summary) = summary(&summaries, &id, revision, &file, length) {
                            remembered.push((id.clone(), summary));
                        }
                        marker.states.insert(id, revision);
                    }
                }
                None => remembered.extend(catch_up(
                    sessions,
                    &mut journal,
                    &changed,
                    &mut marker,
                    &summaries,
                )?),
            }
        }
        moved_sessions += 1;
        moved_states += changed.len();
        if moved_sessions.is_multiple_of(MARKER_INTERVAL) {
            write_marker(&directory, &marker)?;
        }
    }
    marker.source = source;
    marker.offset = offset;
    write_marker(&directory, &marker)?;
    sessions.remember(remembered)?;
    span.record("sessions", moved_sessions);
    span.record("states", moved_states);
    if moved_sessions > 0 {
        nanocode_trace::info!(
            sessions = moved_sessions,
            states = moved_states,
            legacy = %legacy.display(),
            directory = %directory.display(),
            "moved sessions from the shared JSONL journal into their own files"
        );
    }
    Ok(())
}

fn summary(
    summaries: &HashMap<String, (u64, Option<StateSummary>)>,
    id: &str,
    revision: u64,
    file: &str,
    length: u64,
) -> Option<Summary> {
    summaries
        .get(id)
        .filter(|(known, _)| *known == revision)
        .map(|(_, summary)| Summary {
            file: file.to_owned(),
            length,
            revision,
            summary: summary.clone(),
            anchor: None,
        })
}

fn legacy_payload(journal: &mut Journal, id: &str) -> Result<String, StoreError> {
    journal
        .rebuilt(|journal| journal.state(id))?
        .payload
        .ok_or_else(|| backend(format!("legacy JSONL state {id} has no payload")))
}

fn legacy_record(journal: &mut Journal, id: &str, key: &str) -> Result<String, StoreError> {
    journal
        .rebuilt(|journal| journal.record(id, key))?
        .map(stored)
        .ok_or_else(|| backend(format!("legacy JSONL record {key} of {id} is missing")))
}

fn fresh(
    directory: &Path,
    path: &Path,
    journal: &mut Journal,
    ids: &[String],
) -> Result<Option<Fresh>, StoreError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| backend("JSONL session file has no name"))?;
    let mut temporary = nanocode_platform::TemporaryFile::create_in(
        directory,
        format!("{name}.migrate-{}-", std::process::id()),
        None,
    )
    .map_err(backend)?;
    let mut output = BufWriter::with_capacity(JOURNAL_READ_BUFFER_BYTES, temporary.as_file_mut());
    let mut states = Vec::with_capacity(ids.len());
    for id in ids {
        let revision = journal.revision(id);
        if revision == 0 {
            continue;
        }
        let owner = OwnerToken::new(import_owner(), 1);
        write_line(
            &mut output,
            Entry::Import {
                state_id: id.clone(),
                owner: owner.clone(),
                after: None,
                revision,
                payload: legacy_payload(journal, id)?,
            },
        )?;
        let mut records = Vec::new();
        let mut bytes = 0;
        let keys = sorted_keys(journal, id)?;
        legacy_records(journal, id, keys, |record| {
            bytes += record.key.len() + record.value.len();
            records.push(record);
            if bytes < LINE_BYTES {
                return Ok(());
            }
            bytes = 0;
            write_line(
                &mut output,
                Entry::Records {
                    state_id: id.clone(),
                    owner: owner.clone(),
                    records: std::mem::take(&mut records),
                },
            )
        })?;
        if !records.is_empty() {
            write_line(
                &mut output,
                Entry::Records {
                    state_id: id.clone(),
                    owner,
                    records,
                },
            )?;
        }
        states.push((id.clone(), revision));
    }
    output.flush().map_err(backend)?;
    drop(output);
    sync_journal(temporary.as_file()).map_err(backend)?;
    let file = match temporary.persist_noclobber(path) {
        Ok(file) => file,
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
        Err(error) => return Err(backend(error.error)),
    };
    let metadata = file.metadata().map_err(backend)?;
    Ok(Some(Fresh {
        states,
        file: identity(&metadata)?,
        length: metadata.len(),
    }))
}

fn catch_up(
    sessions: &mut Sessions,
    journal: &mut Journal,
    changed: &[String],
    marker: &mut Marker,
    summaries: &HashMap<String, (u64, Option<StateSummary>)>,
) -> Result<Vec<(String, Summary)>, StoreError> {
    let Some(first) = changed.first() else {
        return Ok(Vec::new());
    };
    let target = sessions.opened(first)?;
    let mut caught = Vec::with_capacity(changed.len());
    for id in changed {
        let revision = journal.revision(id);
        target.peek(id)?;
        let owner = target.index.owners.get(id).cloned();
        if target.revision(id) == revision {
            caught.push((id.clone(), revision));
            continue;
        }
        if owner.as_ref().is_some_and(|owner| !imported(owner)) {
            nanocode_trace::warn!(
                state = id.as_str(),
                "keeping a session that changed in its own file since it left the shared JSONL journal"
            );
            continue;
        }
        let known = target.list_records(id)?.into_iter().collect::<HashSet<_>>();
        let mut chunks = Vec::new();
        let mut records = Vec::new();
        let mut bytes = 0;
        let keys = sorted_keys(journal, id)?
            .into_iter()
            .filter(|key| !known.contains(key))
            .collect();
        legacy_records(journal, id, keys, |record| {
            bytes += record.key.len() + record.value.len();
            records.push(record);
            if bytes >= LINE_BYTES {
                bytes = 0;
                chunks.push(std::mem::take(&mut records));
            }
            Ok(())
        })?;
        if !records.is_empty() {
            chunks.push(records);
        }
        let payload = legacy_payload(journal, id)?;
        if target.import(
            id,
            owner.as_ref(),
            import_owner(),
            revision,
            payload,
            chunks,
        )? {
            caught.push((id.clone(), revision));
        }
    }
    let (file, length) = (target.index.source.clone(), target.index.offset);
    let mut remembered = Vec::new();
    for (id, revision) in caught {
        if let Some(summary) = summary(summaries, &id, revision, &file, length) {
            remembered.push((id.clone(), summary));
        }
        marker.states.insert(id, revision);
    }
    Ok(remembered)
}
