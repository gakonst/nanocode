use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use nanocode_durability_core::{StateSummary, StoreError};
use serde::{Deserialize, Serialize};

use super::{backend, identity};

const FILE: &str = ".summaries.json";

pub(super) struct Summaries {
    path: PathBuf,
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
            loaded: None,
            entries: HashMap::new(),
        }
    }

    pub(super) fn refresh(&mut self) {
        let current = fs::metadata(&self.path)
            .ok()
            .and_then(|metadata| identity(&metadata).ok());
        if current.is_none() || current == self.loaded {
            return;
        }
        match File::open(&self.path)
            .map_err(backend)
            .and_then(|file| serde_json::from_reader(BufReader::new(file)).map_err(backend))
        {
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

    pub(super) fn remember(
        &mut self,
        remembered: Vec<(String, Summary)>,
    ) -> Result<(), StoreError> {
        if remembered.is_empty() {
            return Ok(());
        }
        self.refresh();
        self.entries.extend(remembered);
        let directory = self
            .path
            .parent()
            .ok_or_else(|| backend("JSONL session summaries have no directory"))?;
        let mut temporary = tempfile::Builder::new()
            .prefix(FILE)
            .tempfile_in(directory)
            .map_err(backend)?;
        let mut output = BufWriter::new(temporary.as_file_mut());
        serde_json::to_writer(&mut output, &self.entries).map_err(backend)?;
        output.flush().map_err(backend)?;
        drop(output);
        let file = temporary
            .persist(&self.path)
            .map_err(|error| backend(error.error))?;
        self.loaded = Some(identity(&file.metadata().map_err(backend)?)?);
        Ok(())
    }
}
