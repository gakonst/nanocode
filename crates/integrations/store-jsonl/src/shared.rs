use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
};

use nanocode_durability_core::{BlobKey, StoreError, StoreRecord};

use super::{backend, sync_journal};

const DIRECTORY: &str = "records";
const MARK: char = '\u{2}';

#[derive(Clone)]
pub(super) struct Shared {
    directory: Arc<Path>,
    threshold: usize,
}

impl Shared {
    pub(super) fn open(home: &Path) -> Result<Option<Self>, StoreError> {
        let directory = home.join(DIRECTORY);
        fs::create_dir_all(&directory).map_err(backend)?;
        Ok(block_size(&directory)
            .map_err(backend)?
            .map(|threshold| Self {
                directory: directory.into(),
                threshold,
            }))
    }

    pub(super) fn store(&self, records: Vec<StoreRecord>) -> Result<Vec<StoreRecord>, StoreError> {
        let mut added = false;
        let records = records
            .into_iter()
            .map(|StoreRecord { key, value }| {
                let value = if value.len() < self.threshold {
                    if value.starts_with(MARK) {
                        format!("{MARK}{value}")
                    } else {
                        value
                    }
                } else {
                    let blob = BlobKey::of(value.as_bytes());
                    added |= self
                        .put(&blob, value.as_bytes())
                        .map_err(|error| backend(format!("shared record {blob}: {error}")))?;
                    format!("{MARK}{blob}")
                };
                Ok(StoreRecord { key, value })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        if added {
            sync_directory(&self.directory).map_err(backend)?;
        }
        Ok(records)
    }

    pub(super) fn load(&self, value: String) -> Result<String, StoreError> {
        let Some(rest) = value.strip_prefix(MARK) else {
            return Ok(value);
        };
        if rest.starts_with(MARK) {
            return Ok(rest.to_owned());
        }
        let blob =
            BlobKey::parse(rest).ok_or_else(|| backend("a JSONL record names no shared record"))?;
        let bytes = fs::read(self.path(&blob))
            .map_err(|error| backend(format!("shared record {blob}: {error}")))?;
        if !blob.matches(&bytes) {
            return Err(backend(format!(
                "shared record {blob} does not hold the content its name hashes"
            )));
        }
        String::from_utf8(bytes).map_err(backend)
    }

    fn put(&self, blob: &BlobKey, bytes: &[u8]) -> std::io::Result<bool> {
        let path = self.path(blob);
        if path.is_file() {
            return Ok(false);
        }
        let mut file = nanocode_platform::TemporaryFile::new_in(&self.directory)?;
        file.write_all(bytes)?;
        sync_journal(file.as_file())?;
        match file.persist_noclobber(&path) {
            Ok(_) => Ok(true),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error.error),
        }
    }

    fn path(&self, blob: &BlobKey) -> PathBuf {
        let name = blob.as_str();
        self.directory
            .join(name.split_once(':').map_or(name, |(_, hash)| hash))
    }
}

#[cfg(unix)]
fn block_size(directory: &Path) -> std::io::Result<Option<usize>> {
    use std::os::unix::fs::MetadataExt as _;
    Ok(usize::try_from(fs::metadata(directory)?.blksize()).ok())
}

#[cfg(not(unix))]
fn block_size(_: &Path) -> std::io::Result<Option<usize>> {
    Ok(None)
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> std::io::Result<()> {
    sync_journal(&fs::File::open(directory)?)
}

#[cfg(not(unix))]
fn sync_directory(_: &Path) -> std::io::Result<()> {
    Ok(())
}
