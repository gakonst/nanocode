use std::{
    borrow::Cow,
    collections::HashMap,
    fs::{self, File},
    io::{BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use nanocode_durability_core::{StateSummary, StoreError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    Documents, JOURNAL_READ_BUFFER_BYTES, Kind, Location, archive, backend, identity,
    index::digest_anchor,
};

const FILE: &str = ".summaries.json.zst";
const LEGACY: &str = ".summaries.json";

pub(super) struct Summaries {
    path: PathBuf,
    legacy: PathBuf,
    loaded: Option<String>,
    entries: HashMap<String, Summary>,
    copied: Vec<(String, Summary)>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Summary {
    pub(super) file: String,
    pub(super) length: u64,
    pub(super) revision: u64,
    pub(super) summary: Option<StateSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) anchor: Option<Anchor>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
pub(super) struct Anchor {
    pub(super) position: u64,
    pub(super) digest: [u8; 32],
}

impl Anchor {
    fn holds(self, path: &Path, length: u64) -> bool {
        let digest = || {
            let length = usize::try_from(length.checked_sub(self.position)?).ok()?;
            let location = Location {
                position: self.position,
                length,
            };
            digest_anchor(&mut File::open(path).ok()?, Some(location)).ok()
        };
        digest() == Some(self.digest)
    }
}

#[derive(Deserialize)]
struct Replaced<'a> {
    #[serde(rename = "type")]
    kind: Kind,
    #[serde(borrow)]
    state_id: Option<Cow<'a, str>>,
    revision: Option<u64>,
}

fn replaced(lines: &[u8], state_id: &str) -> Option<(u64, usize)> {
    let mut end = lines.strip_suffix(b"\n")?.len();
    let mut last = None;
    loop {
        let start = lines[..end]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |at| at + 1);
        let last = *last.get_or_insert(start);
        if let Ok((entry, _)) = Documents::of::<Replaced>(&lines[start..end])
            && matches!(entry.kind, Kind::Replace | Kind::Import)
            && entry.state_id.as_deref() == Some(state_id)
        {
            return Some((entry.revision?, last));
        }
        end = start.checked_sub(1)?;
    }
}

fn ending(path: &Path, length: u64, state_id: &str) -> Option<(u64, Anchor)> {
    if archive::unarchived(path.file_name()?.to_str()?).is_some() {
        let stored = fs::read(path).ok()?;
        let (revision, _) = replaced(&archive::decompress(&stored).ok()?, state_id)?;
        let anchor = Anchor {
            position: 0,
            digest: Sha256::digest(&stored).into(),
        };
        return Some((revision, anchor));
    }
    let mut file = File::open(path).ok()?;
    let mut wanted = JOURNAL_READ_BUFFER_BYTES as u64;
    loop {
        let start = length.saturating_sub(wanted);
        let mut bytes = vec![0; usize::try_from(length - start).ok()?];
        file.seek(SeekFrom::Start(start)).ok()?;
        file.read_exact(&mut bytes).ok()?;
        if !bytes.ends_with(b"\n") {
            return None;
        }
        let whole = match start {
            0 => Some(0),
            _ => bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|at| at + 1),
        };
        if let Some(whole) = whole
            && let Some((revision, last)) = replaced(&bytes[whole..], state_id)
        {
            let anchor = Anchor {
                position: start + (whole + last) as u64,
                digest: Sha256::digest(&bytes[whole + last..]).into(),
            };
            return Some((revision, anchor));
        }
        if start == 0 {
            return None;
        }
        wanted = wanted.saturating_mul(2);
    }
}

impl Summaries {
    pub(super) fn new(directory: &Path) -> Self {
        Self {
            path: directory.join(FILE),
            legacy: directory.join(LEGACY),
            loaded: None,
            entries: HashMap::new(),
            copied: Vec::new(),
        }
    }

    pub(super) fn refresh(&mut self) {
        let compressed = self.path.is_file();
        let source = if compressed { &self.path } else { &self.legacy };
        let current = fs::metadata(source)
            .ok()
            .and_then(|metadata| identity(&metadata).ok());
        if current.is_none() || current == self.loaded {
            return;
        }
        let read = if compressed {
            fs::read(source)
                .and_then(|bytes| archive::decompress(&bytes))
                .map_err(backend)
                .and_then(|bytes| serde_json::from_slice(&bytes).map_err(backend))
        } else {
            File::open(source)
                .map_err(backend)
                .and_then(|file| serde_json::from_reader(BufReader::new(file)).map_err(backend))
        };
        match read {
            Ok(entries) => self.entries = entries,
            Err(error) => tracing::warn!(%error, "ignoring unreadable JSONL session summaries"),
        }
        self.loaded = current;
    }

    pub(super) fn listed(&self, state_id: &str, file: &str, length: u64) -> Option<Summary> {
        self.entries
            .get(state_id)
            .filter(|known| known.file == file && known.length == length)
            .cloned()
    }

    pub(super) fn current(&self, state_id: &str, file: &str, length: u64) -> bool {
        self.entries
            .get(state_id)
            .is_some_and(|known| known.file == file && known.length == length)
    }

    pub(super) fn colocated(
        &mut self,
        state_id: &str,
        file: &str,
        length: u64,
        revision: u64,
    ) -> Option<Summary> {
        let known = self
            .entries
            .get_mut(state_id)
            .filter(|known| known.file == file && known.revision == revision)?;
        known.length = length;
        Some(known.clone())
    }

    pub(super) fn copied(
        &mut self,
        state_id: &str,
        file: &str,
        path: &Path,
        length: u64,
        revision: Option<u64>,
    ) -> Option<Summary> {
        let known = self
            .entries
            .get_mut(state_id)
            .filter(|known| known.length == length)?;
        let same = match (revision, known.anchor) {
            (Some(revision), _) => known.revision == revision,
            (None, Some(anchor)) => anchor.holds(path, length),
            (None, None) => match ending(path, length, state_id) {
                Some((revision, anchor)) if revision == known.revision => {
                    known.anchor = Some(anchor);
                    true
                }
                _ => false,
            },
        };
        if !same {
            return None;
        }
        known.file = file.to_owned();
        self.copied.push((state_id.to_owned(), known.clone()));
        Some(known.clone())
    }

    pub(super) fn settle(&mut self) -> Result<(), StoreError> {
        let copied = std::mem::take(&mut self.copied);
        self.remember(copied)
    }

    pub(super) fn moved(
        &mut self,
        from: &str,
        to: &str,
        length: u64,
        anchor: Option<Anchor>,
    ) -> Result<(), StoreError> {
        self.refresh();
        let mut moved = false;
        for entry in self.entries.values_mut().filter(|entry| entry.file == from) {
            entry.file = to.to_owned();
            entry.length = length;
            entry.anchor = anchor;
            moved = true;
        }
        if moved { self.persist() } else { Ok(()) }
    }

    pub(super) fn remember(
        &mut self,
        remembered: Vec<(String, Summary)>,
    ) -> Result<(), StoreError> {
        if remembered.is_empty() {
            return Ok(());
        }
        self.refresh();
        self.entries.extend(remembered);
        self.persist()
    }

    fn persist(&mut self) -> Result<(), StoreError> {
        let directory = self
            .path
            .parent()
            .ok_or_else(|| backend("JSONL session summaries have no directory"))?;
        let mut temporary = tempfile::Builder::new()
            .prefix(FILE)
            .tempfile_in(directory)
            .map_err(backend)?;
        let encoded = serde_json::to_vec(&self.entries).map_err(backend)?;
        temporary
            .write_all(&archive::compress(&encoded).map_err(backend)?)
            .map_err(backend)?;
        let file = temporary
            .persist(&self.path)
            .map_err(|error| backend(error.error))?;
        self.loaded = Some(identity(&file.metadata().map_err(backend)?)?);
        match fs::remove_file(&self.legacy) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(backend(error)),
        }
        Ok(())
    }
}
