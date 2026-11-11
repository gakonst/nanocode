use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufReader, Write},
    path::{Path, PathBuf},
};

use nanocode_durability_core::{StateSummary, StoreError};
use serde::{Deserialize, Serialize};

use super::{archive, backend, identity};

const FILE: &str = ".summaries.json.zst";
const LEGACY: &str = ".summaries.json";

pub(super) struct Summaries {
    path: PathBuf,
    legacy: PathBuf,
    loaded: Option<String>,
    entries: HashMap<String, Summary>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Summary {
    pub(super) file: String,
    pub(super) length: u64,
    pub(super) revision: u64,
    pub(super) summary: Option<StateSummary>,
}

impl Summaries {
    pub(super) fn new(directory: &Path) -> Self {
        Self {
            path: directory.join(FILE),
            legacy: directory.join(LEGACY),
            loaded: None,
            entries: HashMap::new(),
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

    pub(super) fn colocated(&self, state_id: &str, file: &str, revision: u64) -> Option<Summary> {
        self.entries
            .get(state_id)
            .filter(|known| known.file == file && known.revision == revision)
            .cloned()
    }

    pub(super) fn moved(&mut self, from: &str, to: &str, length: u64) -> Result<(), StoreError> {
        self.refresh();
        let mut moved = false;
        for entry in self.entries.values_mut().filter(|entry| entry.file == from) {
            entry.file = to.to_owned();
            entry.length = length;
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
