use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    fs::{self, Metadata},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use nanocode_durability_core::{
    KnownSummary, ListedState, ListedSummary, OwnedState, OwnerId, OwnerToken, StoreError,
    StoreRecord, StoreReplacement, StoreUpdate, StoredState,
};

use super::{
    CompactionStats, Journal, Replacement, archive, backend, identity, process_alive,
    shared::Shared,
    summaries::{Summaries, Summary},
};

const OPEN_JOURNALS: usize = 32;
const EXTENSION: &str = ".jsonl";
const HOMES: &str = ".homes";

pub(super) struct Sessions {
    directory: PathBuf,
    journals: HashMap<String, Opened>,
    clock: u64,
    summaries: Summaries,
    homes: HashMap<String, String>,
    colocations: Option<fs::File>,
    shared: Option<Shared>,
    touched: HashSet<String>,
}

struct Opened {
    journal: Journal,
    used: u64,
}

pub(super) fn session_key(state_id: &str) -> &str {
    match state_id.split_once(':') {
        Some((root, _)) if !root.is_empty() => root,
        _ => state_id,
    }
}

pub(super) fn file_name(key: &str) -> String {
    let mut name = String::with_capacity(key.len() + EXTENSION.len());
    for (index, byte) in key.bytes().enumerate() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_')
            || (byte == b'.' && index > 0)
        {
            name.push(char::from(byte));
        } else {
            name.push_str(&format!("%{byte:02X}"));
        }
    }
    name.push_str(EXTENSION);
    name
}

fn key_of(name: &str) -> Option<String> {
    let name = archive::unarchived(name).unwrap_or(name);
    let mut rest = name.strip_suffix(EXTENSION)?.as_bytes();
    let mut bytes = Vec::with_capacity(rest.len());
    while let Some((&byte, tail)) = rest.split_first() {
        if byte == b'%' {
            let hex = std::str::from_utf8(tail.get(..2)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            rest = &tail[2..];
        } else {
            bytes.push(byte);
            rest = tail;
        }
    }
    let key = String::from_utf8(bytes).ok()?;
    (!key.is_empty() && file_name(&key) == name).then_some(key)
}

fn clear_temporaries(directory: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let abandoned = [".compact-", ".migrate-"].into_iter().any(|marker| {
            name.split_once(marker)
                .and_then(|(_, rest)| rest.split_once('-'))
                .and_then(|(pid, _)| pid.parse::<u32>().ok())
                .is_some_and(|pid| !process_alive(pid))
        });
        if abandoned && entry.file_type()?.is_file() {
            match fs::remove_file(entry.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

fn created(metadata: &Metadata) -> std::time::SystemTime {
    metadata
        .created()
        .or_else(|_| metadata.modified())
        .unwrap_or(UNIX_EPOCH)
}

impl Sessions {
    pub(super) fn open(directory: PathBuf) -> Result<Self, StoreError> {
        if fs::metadata(&directory).is_ok_and(|metadata| !metadata.is_dir()) {
            return Err(backend(format!(
                "{} is not a directory: JSONL sessions are stored one file per session in a directory",
                directory.display()
            )));
        }
        fs::create_dir_all(&directory).map_err(backend)?;
        clear_temporaries(&directory).map_err(backend)?;
        let homes = match fs::read_to_string(directory.join(HOMES)) {
            Ok(lines) => lines
                .lines()
                .filter_map(|line| serde_json::from_str::<(String, String)>(line).ok())
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(error) => return Err(backend(error)),
        };
        Ok(Self {
            summaries: Summaries::new(&directory),
            directory,
            journals: HashMap::new(),
            clock: 0,
            homes,
            colocations: None,
            shared: None,
            touched: HashSet::new(),
        })
    }

    fn home(&self, state_id: &str) -> String {
        let key = session_key(state_id);
        self.homes.get(key).map_or(key, String::as_str).to_owned()
    }

    pub(super) fn colocate(&mut self, state_id: &str, beside: &str) {
        let key = session_key(state_id);
        let home = self.home(beside);
        if key == home || self.homes.contains_key(key) || archive::exists(&self.path(key)) {
            return;
        }
        let line = serde_json::to_string(&(key, &home)).map(|line| line + "\n");
        let appended = line.map_err(std::io::Error::other).and_then(|line| {
            use std::io::Write as _;
            let mut colocations = match self.colocations.take() {
                Some(colocations) => colocations,
                None => fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(self.directory.join(HOMES))?,
            };
            colocations.write_all(line.as_bytes())?;
            self.colocations = Some(colocations);
            Ok(())
        });
        if let Err(error) = appended {
            tracing::warn!(%error, state_id, "could not record where a JSONL session is stored");
        }
        if let Some(opened) = self.journals.get_mut(&home) {
            opened.journal.sidecar = true;
        }
        self.homes.insert(key.to_owned(), home);
    }

    pub(super) fn open_home(home: &Path) -> Result<Self, StoreError> {
        let mut sessions = Self::open(home.join("sessions"))?;
        sessions.shared = Shared::open(home)?;
        let legacy = home.join("sessions.jsonl");
        if legacy.is_file() {
            super::migrate::migrate(&mut sessions, &legacy)?;
        }
        Ok(sessions)
    }

    pub(super) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(super) fn path(&self, key: &str) -> PathBuf {
        self.directory.join(file_name(key))
    }

    fn journal(
        &mut self,
        state_id: &str,
        create: bool,
    ) -> Result<Option<&mut Journal>, StoreError> {
        let key = self.home(state_id);
        let key = key.as_str();
        self.clock += 1;
        let clock = self.clock;
        if !self.journals.contains_key(key) {
            let path = self.path(key);
            if !path.is_file() && archive::archived(&path).is_file() {
                archive::restore(&path).map_err(backend)?;
                self.touched.insert(key.to_owned());
            }
            if !create && !path.is_file() {
                return Ok(None);
            }
            if self.journals.len() >= OPEN_JOURNALS
                && let Some(oldest) = self
                    .journals
                    .iter()
                    .min_by_key(|(_, opened)| opened.used)
                    .map(|(key, _)| key.clone())
                && let Some(mut closed) = self.journals.remove(&oldest)
            {
                closed.journal.harden()?;
            }
            let mut journal = Journal::open(&path, self.homes.values().any(|home| home == key))?;
            journal.shared = self.shared.clone();
            for member in journal
                .index
                .states
                .keys()
                .chain(journal.index.owners.keys())
            {
                let member = session_key(member);
                if member != key && !self.homes.contains_key(member) {
                    self.homes.insert(member.to_owned(), key.to_owned());
                }
            }
            self.journals.insert(
                key.to_owned(),
                Opened {
                    journal,
                    used: clock,
                },
            );
        }
        Ok(self.journals.get_mut(key).map(|opened| {
            opened.used = clock;
            opened.journal.index_due();
            &mut opened.journal
        }))
    }

    pub(super) fn stale(&mut self) {
        for opened in self.journals.values_mut() {
            opened.journal.stale();
        }
    }

    pub(super) fn flush(&mut self) {
        for opened in self.journals.values_mut() {
            opened.journal.flush_index();
        }
    }

    pub(super) fn unsynced(&mut self) -> Vec<std::sync::Arc<fs::File>> {
        self.journals
            .values_mut()
            .filter_map(|opened| opened.journal.unsynced())
            .collect()
    }

    pub(super) fn existing(&mut self, state_id: &str) -> Result<Option<&mut Journal>, StoreError> {
        self.journal(state_id, false)
    }

    pub(super) fn opened(&mut self, state_id: &str) -> Result<&mut Journal, StoreError> {
        self.touched.insert(self.home(state_id));
        self.journal(state_id, true)?
            .ok_or_else(|| backend("JSONL session file was not created"))
    }

    fn files(&self) -> Result<Vec<(String, Metadata)>, StoreError> {
        let mut files = Vec::new();
        for entry in fs::read_dir(&self.directory).map_err(backend)? {
            let entry = entry.map_err(backend)?;
            let Some(key) = entry.file_name().to_str().and_then(key_of) else {
                continue;
            };
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(backend(error)),
            };
            if metadata.is_file() && !files.iter().any(|(listed, _)| *listed == key) {
                files.push((key, metadata));
            }
        }
        files.sort_by_key(|(_, metadata)| Reverse(created(metadata)));
        Ok(files)
    }

    pub(super) fn acquire_with(
        &mut self,
        id: &str,
        owner_id: OwnerId,
        update: StoreUpdate,
    ) -> Result<OwnedState, StoreError> {
        let update = self.sharing(update);
        self.opened(id)?
            .acquire_batch(id, owner_id, update, None)
            .map(|(owned, _)| owned)
    }

    pub(super) fn acquire_with_companion(
        &mut self,
        id: &str,
        owner_id: OwnerId,
        update: StoreUpdate,
        companion_id: &str,
        companion_owner: OwnerId,
    ) -> Result<(OwnedState, OwnedState), StoreError> {
        if self.home(id) != self.home(companion_id) {
            return Err(StoreError::NotCommitted(format!(
                "JSONL stores {companion_id} in another session file than {id}"
            )));
        }
        let update = self.sharing(update);
        let (owned, companion) = self.opened(id)?.acquire_batch(
            id,
            owner_id,
            update,
            Some((companion_id, companion_owner)),
        )?;
        companion
            .map(|companion| (owned, companion))
            .ok_or_else(|| backend("JSONL companion acquisition was not staged"))
    }

    pub(super) fn acquire(
        &mut self,
        id: &str,
        owner_id: OwnerId,
    ) -> Result<OwnedState, StoreError> {
        self.opened(id)?.acquire(id, owner_id)
    }

    pub(super) fn acquire_many(
        &mut self,
        acquisitions: Vec<(String, OwnerId)>,
    ) -> Vec<Result<OwnedState, StoreError>> {
        let mut results: Vec<Option<Result<OwnedState, StoreError>>> =
            (0..acquisitions.len()).map(|_| None).collect();
        let mut homes = Vec::<String>::new();
        let mut groups = Vec::<(Vec<usize>, Vec<(String, OwnerId)>)>::new();
        for (slot, acquisition) in acquisitions.into_iter().enumerate() {
            let home = self.home(&acquisition.0);
            match homes.iter().position(|key| *key == home) {
                Some(group) => {
                    groups[group].0.push(slot);
                    groups[group].1.push(acquisition);
                }
                None => {
                    homes.push(home);
                    groups.push((vec![slot], vec![acquisition]));
                }
            }
        }
        for (slots, group) in groups {
            let acquired = match group.first() {
                Some((first, _)) => self
                    .opened(first)
                    .and_then(|journal| journal.acquire_many(&group)),
                None => continue,
            };
            match acquired {
                Ok(states) => {
                    for (slot, state) in slots.into_iter().zip(states) {
                        results[slot] = Some(Ok(state));
                    }
                }
                Err(error) => {
                    for slot in slots {
                        results[slot] = Some(Err(error.clone()));
                    }
                }
            }
        }
        results
            .into_iter()
            .map(|result| {
                result.unwrap_or_else(|| Err(backend("JSONL acquisition was not attempted")))
            })
            .collect()
    }

    pub(super) fn replace(
        &mut self,
        id: &str,
        owner: &OwnerToken,
        expected: u64,
        payload: String,
        records: Vec<StoreRecord>,
    ) -> Result<u64, StoreError> {
        let records = self.stored(records)?;
        self.touched.insert(self.home(id));
        match self.existing(id)? {
            Some(journal) => journal.replace(id, owner, expected, payload, records),
            None => Err(StoreError::Fenced),
        }
    }

    pub(super) fn replace_many(
        &mut self,
        replacements: Vec<Result<Replacement, StoreError>>,
    ) -> Vec<Result<u64, StoreError>> {
        let mut results: Vec<Option<Result<u64, StoreError>>> =
            (0..replacements.len()).map(|_| None).collect();
        let mut groups: Vec<(Vec<usize>, Vec<Replacement>)> = Vec::new();
        let mut grouped: HashMap<String, usize> = HashMap::new();
        for (slot, replacement) in replacements.into_iter().enumerate() {
            match replacement {
                Err(error) => results[slot] = Some(Err(error)),
                Ok(mut replacement) => {
                    replacement.records = match self.stored(replacement.records) {
                        Ok(records) => records,
                        Err(error) => {
                            results[slot] = Some(Err(error));
                            continue;
                        }
                    };
                    let key = self.home(&replacement.state_id);
                    let group = *grouped.entry(key).or_insert_with(|| {
                        groups.push((Vec::new(), Vec::new()));
                        groups.len() - 1
                    });
                    groups[group].0.push(slot);
                    groups[group].1.push(replacement);
                }
            }
        }
        for (slots, replacements) in groups {
            let Some(first) = replacements.first().map(|first| first.state_id.clone()) else {
                continue;
            };
            let count = replacements.len();
            self.touched.insert(self.home(&first));
            let outcome = match self.existing(&first) {
                Ok(Some(journal)) => {
                    journal.replace_many(replacements.into_iter().map(Ok).collect())
                }
                Ok(None) => (0..count).map(|_| Err(StoreError::Fenced)).collect(),
                Err(error) => (0..count).map(|_| Err(error.clone())).collect(),
            };
            for (slot, result) in slots.into_iter().zip(outcome) {
                results[slot] = Some(result);
            }
        }
        results
            .into_iter()
            .map(|result| {
                result.unwrap_or_else(|| Err(backend("JSONL replacement was not attempted")))
            })
            .collect()
    }

    pub(super) fn read_record(
        &mut self,
        id: &str,
        key: &str,
    ) -> Result<Option<String>, StoreError> {
        let value = match self.existing(id)? {
            Some(journal) => journal.read_record(id, key)?,
            None => None,
        };
        value.map(|value| self.loaded(value)).transpose()
    }

    pub(super) fn read_records(
        &mut self,
        id: &str,
        keys: &[String],
    ) -> Result<Vec<Option<String>>, StoreError> {
        let values = match self.existing(id)? {
            Some(journal) => journal.read_records(id, keys)?,
            None => vec![None; keys.len()],
        };
        values
            .into_iter()
            .map(|value| value.map(|value| self.loaded(value)).transpose())
            .collect()
    }

    fn stored(&self, records: Vec<StoreRecord>) -> Result<Vec<StoreRecord>, StoreError> {
        match &self.shared {
            Some(shared) => shared.store(records),
            None => Ok(records),
        }
    }

    fn loaded(&self, value: String) -> Result<String, StoreError> {
        match &self.shared {
            Some(shared) => shared.load(value),
            None => Ok(value),
        }
    }

    fn sharing(&self, update: StoreUpdate) -> StoreUpdate {
        let Some(shared) = self.shared.clone() else {
            return update;
        };
        Box::new(move |state| {
            update(state)?
                .map(|replacement| {
                    Ok(StoreReplacement {
                        records: shared.store(replacement.records)?,
                        payload: replacement.payload,
                    })
                })
                .transpose()
        })
    }

    pub(super) fn peek(&mut self, id: &str) -> Result<StoredState, StoreError> {
        match self.existing(id)? {
            Some(journal) => journal.peek(id),
            None => Ok(StoredState::default()),
        }
    }

    pub(super) fn owner(&mut self, id: &str) -> Result<Option<OwnerToken>, StoreError> {
        match self.existing(id)? {
            Some(journal) => journal.owner(id),
            None => Ok(None),
        }
    }

    pub(super) fn list_records(&mut self, id: &str) -> Result<Vec<String>, StoreError> {
        match self.existing(id)? {
            Some(journal) => journal.list_records(id),
            None => Ok(Vec::new()),
        }
    }

    fn listed(&mut self) -> Result<Vec<(String, Metadata, Option<u64>)>, StoreError> {
        let mut members = HashMap::<String, Vec<String>>::new();
        for (member, home) in &self.homes {
            members
                .entry(home.clone())
                .or_default()
                .push(member.clone());
        }
        let mut listed = Vec::new();
        for (key, metadata) in self.files()? {
            let mut colocated = members.remove(&key).unwrap_or_default();
            colocated.sort_unstable_by(|a, b| b.cmp(a));
            if !colocated.is_empty()
                && let Some(journal) = self.existing(&key)?
            {
                listed.extend(colocated.into_iter().filter_map(|member| {
                    let revision = journal.index.states.get(&member)?.revision;
                    Some((member, metadata.clone(), Some(revision)))
                }));
            }
            listed.push((key, metadata, None));
        }
        Ok(listed)
    }

    pub(super) fn list_states(&mut self, limit: usize) -> Result<Vec<String>, StoreError> {
        Ok(self
            .listed()?
            .into_iter()
            .take(limit)
            .map(|(key, _, _)| key)
            .collect())
    }

    pub(super) fn list_summaries(&mut self, limit: usize) -> Result<Vec<ListedState>, StoreError> {
        let listed = self.listed()?;
        self.summaries.refresh();
        listed
            .into_iter()
            .take(limit)
            .map(|(state_id, metadata, colocated)| {
                let file = identity(&metadata)?;
                let known = match colocated {
                    Some(revision) => self.summaries.colocated(&state_id, &file, revision),
                    None => self.summaries.listed(&state_id, &file, metadata.len()),
                };
                Ok(ListedState {
                    revision: known.as_ref().map_or(0, |known| known.revision),
                    summary: match known {
                        Some(Summary {
                            summary: Some(summary),
                            ..
                        }) => ListedSummary::Listed(summary),
                        Some(_) => ListedSummary::Unlisted,
                        None => ListedSummary::Unknown,
                    },
                    state_id,
                })
            })
            .collect()
    }

    pub(super) fn remember_summaries(
        &mut self,
        known: Vec<KnownSummary>,
    ) -> Result<(), StoreError> {
        let mut remembered = Vec::with_capacity(known.len());
        for summary in known {
            let Some(journal) = self.existing(&summary.state_id)? else {
                continue;
            };
            let revision = journal.peek(&summary.state_id)?.revision;
            if summary.revision == revision && revision > 0 {
                remembered.push((
                    summary.state_id,
                    Summary {
                        file: journal.index.source.clone(),
                        length: journal.index.offset,
                        revision,
                        summary: summary.summary,
                    },
                ));
            }
        }
        self.remember(remembered)
    }

    pub(super) fn remember(
        &mut self,
        remembered: Vec<(String, Summary)>,
    ) -> Result<(), StoreError> {
        self.summaries.remember(remembered)
    }

    pub(super) fn archive(&mut self) -> Result<usize, StoreError> {
        let mut archived = 0;
        let mut touched = std::mem::take(&mut self.touched)
            .into_iter()
            .collect::<Vec<_>>();
        touched.sort_unstable();
        for key in touched {
            let Some(mut opened) = self.journals.remove(&key) else {
                continue;
            };
            if opened.journal.sidecar {
                self.journals.insert(key, opened);
                continue;
            }
            let path = self.path(&key);
            let from = identity(&fs::metadata(&path).map_err(backend)?)?;
            opened.journal.compact()?;
            opened.journal.harden()?;
            drop(opened);
            let compacted = identity(&fs::metadata(&path).map_err(backend)?)?;
            if compacted != from {
                self.summaries.moved(&from, &compacted, 0)?;
            }
            let from = compacted;
            if archive::archive(&path).map_err(backend)? {
                let stored = fs::metadata(archive::archived(&path)).map_err(backend)?;
                self.summaries
                    .moved(&from, &identity(&stored)?, stored.len())?;
                archived += 1;
            }
        }
        Ok(archived)
    }

    pub(super) fn compact(&mut self) -> Result<CompactionStats, StoreError> {
        let mut total = CompactionStats {
            before_bytes: 0,
            after_bytes: 0,
            records_removed: 0,
        };
        for (key, _) in self.files()? {
            let Some(journal) = self.existing(&key)? else {
                continue;
            };
            let stats = journal.compact()?;
            total.before_bytes += stats.before_bytes;
            total.after_bytes += stats.after_bytes;
            total.records_removed += stats.records_removed;
        }
        Ok(total)
    }
}
