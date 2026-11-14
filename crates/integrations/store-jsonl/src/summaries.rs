use std::{
    borrow::Cow,
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use nanocode_digest::Sha256;
use nanocode_durability_core::{StateSummary, StoreError};
use serde::{Deserialize, Serialize};

use super::{
    Documents, JOURNAL_READ_BUFFER_BYTES, Kind, Location, archive, backend, identity,
    index::digest_anchor,
};

const FILE: &str = ".summaries";
const LEGACY: [&str; 2] = [".summaries.json.zst", ".summaries.json"];
const LENGTH_BYTES: usize = 4;
const PLAIN: u8 = b'{';

pub(super) struct Summaries {
    path: PathBuf,
    legacy: [PathBuf; 2],
    loaded: Loaded,
    entries: HashMap<String, Summary>,
    copied: Vec<(String, Summary)>,
}

#[derive(PartialEq)]
enum Loaded {
    Nothing,
    Legacy(String),
    Journal { identity: String, end: u64 },
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
    #[serde(with = "hex_digest")]
    pub(super) digest: [u8; 32],
}

mod hex_digest {
    use std::fmt::{self, Write as _};

    use serde::{Deserializer, Serializer, de};

    pub(super) fn serialize<S: Serializer>(
        digest: &[u8; 32],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut text = String::with_capacity(digest.len() * 2);
        for byte in digest {
            write!(text, "{byte:02x}").map_err(serde::ser::Error::custom)?;
        }
        serializer.serialize_str(&text)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<[u8; 32], D::Error> {
        deserializer.deserialize_any(Digest)
    }

    struct Digest;

    impl<'de> de::Visitor<'de> for Digest {
        type Value = [u8; 32];

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a SHA-256 digest as hex or as bytes")
        }

        fn visit_str<E: de::Error>(self, text: &str) -> Result<Self::Value, E> {
            let mut digest = [0; 32];
            if text.len() != digest.len() * 2 {
                return Err(E::invalid_length(text.len(), &self));
            }
            for (byte, pair) in digest.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
                let nibble = |at: usize| char::from(pair[at]).to_digit(16);
                *byte = nibble(0)
                    .zip(nibble(1))
                    .and_then(|(high, low)| u8::try_from(high << 4 | low).ok())
                    .ok_or_else(|| E::invalid_value(de::Unexpected::Str(text), &self))?;
            }
            Ok(digest)
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut bytes: A) -> Result<Self::Value, A::Error> {
            let mut digest = [0; 32];
            for (index, byte) in digest.iter_mut().enumerate() {
                *byte = bytes
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(index, &self))?;
            }
            if bytes.next_element::<u8>()?.is_some() {
                return Err(de::Error::invalid_length(digest.len() + 1, &self));
            }
            Ok(digest)
        }
    }
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
            digest: Sha256::digest(&stored),
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
                digest: Sha256::digest(&bytes[whole + last..]),
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
            legacy: LEGACY.map(|name| directory.join(name)),
            loaded: Loaded::Nothing,
            entries: HashMap::new(),
            copied: Vec::new(),
        }
    }

    pub(super) fn refresh(&mut self) {
        match fs::metadata(&self.path) {
            Ok(metadata) => self.refresh_journal(&metadata),
            Err(_) => self.refresh_legacy(),
        }
    }

    fn refresh_journal(&mut self, metadata: &fs::Metadata) {
        let Ok(identity) = identity(metadata) else {
            return;
        };
        let start = match &self.loaded {
            Loaded::Journal {
                identity: loaded,
                end,
            } if *loaded == identity && *end <= metadata.len() => Some(*end),
            _ => None,
        };
        if start == Some(metadata.len()) {
            return;
        }
        let read = File::open(&self.path).and_then(|mut file| {
            file.seek(SeekFrom::Start(start.unwrap_or(0)))?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Ok(bytes)
        });
        let bytes = match read {
            Ok(bytes) => bytes,
            Err(error) => {
                nanocode_trace::warn!(%error, "ignoring unreadable JSONL session summaries");
                return;
            }
        };
        let (changes, consumed) = match start {
            Some(end) => (&bytes[..], end),
            None => match base(&bytes) {
                Ok((entries, rest)) => {
                    self.entries = entries;
                    (rest, (bytes.len() - rest.len()) as u64)
                }
                Err(error) => {
                    nanocode_trace::warn!(%error, "ignoring unreadable JSONL session summaries");
                    self.entries.clear();
                    self.loaded = Loaded::Nothing;
                    return;
                }
            },
        };
        let mut applied = 0;
        let mut files = None;
        for line in changes.split_inclusive(|byte| *byte == b'\n') {
            if !line.ends_with(b"\n") {
                break;
            }
            match serde_json::from_slice::<Change>(line) {
                Ok(change) => change.replay(&mut self.entries, &mut files),
                Err(error) => {
                    nanocode_trace::warn!(%error, "ignoring unreadable JSONL session summaries");
                    break;
                }
            }
            applied += line.len();
        }
        self.loaded = Loaded::Journal {
            identity,
            end: consumed + applied as u64,
        };
    }

    fn refresh_legacy(&mut self) {
        let Some((source, compressed)) = self
            .legacy
            .iter()
            .zip([true, false])
            .find(|(path, _)| path.is_file())
        else {
            return;
        };
        let Some(current) = fs::metadata(source)
            .ok()
            .and_then(|metadata| identity(&metadata).ok())
        else {
            return;
        };
        if self.loaded == Loaded::Legacy(current.clone()) {
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
            Err(error) => nanocode_trace::warn!(%error, "ignoring unreadable JSONL session summaries"),
        }
        self.loaded = Loaded::Legacy(current);
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
        self.record(Change::Moved {
            from: from.to_owned(),
            to: to.to_owned(),
            length,
            anchor,
        })
    }

    pub(super) fn remember(
        &mut self,
        remembered: Vec<(String, Summary)>,
    ) -> Result<(), StoreError> {
        if remembered.is_empty() {
            return Ok(());
        }
        self.record(Change::Remember(remembered.into_iter().collect()))
    }

    fn record(&mut self, change: Change) -> Result<(), StoreError> {
        let mut line = serde_json::to_vec(&change).map_err(backend)?;
        line.push(b'\n');
        if self.append(&line)? {
            if matches!(self.loaded, Loaded::Journal { .. }) {
                change.apply(&mut self.entries);
            }
            return Ok(());
        }
        self.refresh();
        change.apply(&mut self.entries);
        self.rewrite()
    }

    fn append(&mut self, line: &[u8]) -> Result<bool, StoreError> {
        let mut file = match OpenOptions::new().read(true).append(true).open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(backend(error)),
        };
        let length = file.metadata().map_err(backend)?.len();
        let mut prefix = [0; LENGTH_BYTES];
        let mut format = [0];
        if file
            .read_exact(&mut prefix)
            .and_then(|()| file.read_exact(&mut format))
            .is_err()
            || format != [PLAIN]
        {
            return Ok(false);
        }
        let base = LENGTH_BYTES as u64 + u64::from(u32::from_le_bytes(prefix));
        if length < base || (length - base) + line.len() as u64 > base {
            return Ok(false);
        }
        if length > base {
            let mut last = [0];
            file.seek(SeekFrom::Start(length - 1)).map_err(backend)?;
            file.read_exact(&mut last).map_err(backend)?;
            if last != *b"\n" {
                return Ok(false);
            }
        }
        file.write_all(line).map_err(backend)?;
        let identity = identity(&file.metadata().map_err(backend)?)?;
        if let Loaded::Journal {
            identity: loaded,
            end,
        } = &mut self.loaded
            && *loaded == identity
            && *end == length
        {
            *end += line.len() as u64;
        }
        Ok(true)
    }

    fn rewrite(&mut self) -> Result<(), StoreError> {
        let directory = self
            .path
            .parent()
            .ok_or_else(|| backend("JSONL session summaries have no directory"))?;
        let mut temporary =
            nanocode_platform::TemporaryFile::create_in(directory, FILE, None).map_err(backend)?;
        let frame = serde_json::to_vec(&self.entries).map_err(backend)?;
        let length = u32::try_from(frame.len())
            .map_err(|_| backend("JSONL session summaries are too large"))?;
        temporary
            .write_all(&length.to_le_bytes())
            .and_then(|()| temporary.write_all(&frame))
            .map_err(backend)?;
        let file = temporary
            .persist(&self.path)
            .map_err(|error| backend(error.error))?;
        self.loaded = Loaded::Journal {
            identity: identity(&file.metadata().map_err(backend)?)?,
            end: (LENGTH_BYTES + frame.len()) as u64,
        };
        for legacy in &self.legacy {
            match fs::remove_file(legacy) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(backend(error)),
            }
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Change {
    Remember(HashMap<String, Summary>),
    Moved {
        from: String,
        to: String,
        length: u64,
        anchor: Option<Anchor>,
    },
}

impl Change {
    fn apply(&self, entries: &mut HashMap<String, Summary>) {
        match self {
            Self::Remember(remembered) => entries.extend(
                remembered
                    .iter()
                    .map(|(state_id, summary)| (state_id.clone(), summary.clone())),
            ),
            Self::Moved {
                from,
                to,
                length,
                anchor,
            } => {
                for entry in entries.values_mut().filter(|entry| entry.file == *from) {
                    entry.file.clone_from(to);
                    entry.length = *length;
                    entry.anchor = *anchor;
                }
            }
        }
    }
}

impl Change {
    fn replay(
        self,
        entries: &mut HashMap<String, Summary>,
        files: &mut Option<HashMap<String, Vec<String>>>,
    ) {
        match self {
            Self::Remember(remembered) => {
                for (state_id, summary) in remembered {
                    if let Some(files) = files {
                        if let Some(ids) = entries
                            .get(&state_id)
                            .and_then(|known| files.get_mut(&known.file))
                        {
                            ids.retain(|id| *id != state_id);
                        }
                        files
                            .entry(summary.file.clone())
                            .or_default()
                            .push(state_id.clone());
                    }
                    entries.insert(state_id, summary);
                }
            }
            Self::Moved {
                from,
                to,
                length,
                anchor,
            } => {
                let files = files.get_or_insert_with(|| {
                    let mut files = HashMap::<String, Vec<String>>::new();
                    for (state_id, entry) in entries.iter() {
                        files
                            .entry(entry.file.clone())
                            .or_default()
                            .push(state_id.clone());
                    }
                    files
                });
                let Some(moved) = files.remove(&from) else {
                    return;
                };
                for state_id in &moved {
                    if let Some(entry) = entries.get_mut(state_id) {
                        entry.file.clone_from(&to);
                        entry.length = length;
                        entry.anchor = anchor;
                    }
                }
                files.entry(to).or_default().extend(moved);
            }
        }
    }
}

fn base(bytes: &[u8]) -> Result<(HashMap<String, Summary>, &[u8]), StoreError> {
    let unreadable = || backend("JSONL session summaries are truncated");
    let (length, rest) = bytes
        .split_first_chunk::<LENGTH_BYTES>()
        .ok_or_else(unreadable)?;
    let length = usize::try_from(u32::from_le_bytes(*length)).map_err(|_| unreadable())?;
    let frame = rest.get(..length).ok_or_else(unreadable)?;
    let entries = match frame.first() {
        Some(&PLAIN) => serde_json::from_slice(frame),
        _ => serde_json::from_slice(&archive::decompress(frame).map_err(backend)?),
    }
    .map_err(backend)?;
    Ok((entries, &rest[length..]))
}
