use crate::tools::{
    SessionEnvironment, ToolContent, ToolOutput, media::MediaReadOptions, text_files::Known,
};
use nanocode_sync::Atom;
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    hash::Hasher as _,
    io::{BufRead as _, BufReader, Read as _, Seek as _, SeekFrom, Write as _},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

#[derive(Clone, Copy, Debug)]
enum View {
    Part,
    Whole(u64),
    Current(u64),
}

#[derive(Clone, Copy, Debug)]
struct Seen {
    modified: SystemTime,
    view: View,
}

#[derive(Clone, Debug)]
pub struct ClaudeWorkspaceFiles {
    root: PathBuf,
    base: PathBuf,
    confined: bool,
    media: MediaReadOptions,
    read_token_budget: usize,
    reads: Arc<Atom<HashMap<PathBuf, Seen>>>,
}

impl ClaudeWorkspaceFiles {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, String> {
        let given = root.as_ref();
        let root = fs::canonicalize(given).map_err(|e| format!("workspace root: {e}"))?;
        if !root.is_dir() {
            return Err("workspace root is not a directory".into());
        }
        Ok(Self {
            base: root.clone(),
            confined: false,
            root,
            media: MediaReadOptions::default(),
            read_token_budget: usize::MAX,
            reads: Arc::default(),
        })
    }

    pub fn machine(directory: impl AsRef<Path>) -> Result<Self, String> {
        let base = fs::canonicalize(directory.as_ref()).map_err(|e| format!("directory: {e}"))?;
        if !base.is_dir() {
            return Err("directory is not a directory".into());
        }
        let root = base
            .ancestors()
            .last()
            .ok_or("directory has no filesystem root")?
            .to_path_buf();
        Ok(Self {
            root,
            base,
            confined: false,
            media: MediaReadOptions::default(),
            read_token_budget: usize::MAX,
            reads: Arc::default(),
        })
    }

    pub fn confined(directory: impl AsRef<Path>) -> Result<Self, String> {
        let base = fs::canonicalize(directory.as_ref()).map_err(|e| format!("directory: {e}"))?;
        if !base.is_dir() {
            return Err("directory is not a directory".into());
        }
        Ok(Self {
            root: base.clone(),
            base,
            confined: true,
            media: MediaReadOptions::default(),
            read_token_budget: usize::MAX,
            reads: Arc::default(),
        })
    }

    fn inside(&self, real: PathBuf) -> Result<PathBuf, String> {
        if self.confined && !real.starts_with(&self.root) {
            return Err(format!(
                "{} is outside {}, the only directory these file tools may use",
                real.display(),
                self.root.display()
            ));
        }
        Ok(real)
    }

    fn creatable(&self, parent: &Path) -> Result<(), String> {
        if !self.confined {
            return Ok(());
        }
        let mut existing = parent;
        let real = loop {
            match fs::canonicalize(existing) {
                Ok(real) => break real,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    existing = existing.parent().ok_or("file has no parent")?;
                }
                Err(e) => return Err(format!("parent: {e}")),
            }
        };
        self.inside(real)?;
        let missing = parent
            .strip_prefix(existing)
            .map_err(|e| format!("parent: {e}"))?;
        if missing
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        {
            Ok(())
        } else {
            Err(format!(
                "{} leaves {} through directories that do not exist yet",
                parent.display(),
                self.root.display()
            ))
        }
    }

    #[must_use]
    pub fn with_media_options(mut self, media: MediaReadOptions) -> Self {
        self.media = media;
        self
    }

    #[must_use]
    pub const fn with_read_token_budget(mut self, tokens: usize) -> Self {
        self.read_token_budget = tokens;
        self
    }

    #[must_use]
    pub fn definitions() -> Vec<Value> {
        super::text_files::definitions()
    }

    #[must_use]
    pub fn tool_schemas() -> Vec<Value> {
        Self::definitions()
    }

    pub async fn execute(&self, name: &str, input: Value) -> Result<String, String> {
        let output = self.execute_output(name, input).await?;
        match output.content {
            ToolContent::Text(text) if output.is_error => Err(text),
            ToolContent::Text(text) => Ok(text),
            ToolContent::Blocks(blocks) => {
                let mut text = String::new();
                for block in blocks {
                    match block {
                        crate::tools::ToolResultBlock::Text { text: part } => text.push_str(&part),
                        _ => return Err("Read returned media; use execute_output to preserve native Claude content blocks".into()),
                    }
                }
                Ok(text)
            }
        }
    }

    pub async fn execute_output(&self, name: &str, input: Value) -> Result<ToolOutput, String> {
        self.execute_output_in_session(name, input, None).await
    }

    pub async fn execute_output_in_session(
        &self,
        name: &str,
        input: Value,
        session: Option<SessionEnvironment>,
    ) -> Result<ToolOutput, String> {
        let this = self.clone();
        let name = name.to_owned();
        nanocode_platform::spawn_blocking(move || {
            let mut media = None;
            if name == "Read" {
                let fields = input.as_object().ok_or("Read input must be an object")?;
                if let Some(key) = fields.keys().find(|k| {
                    !["file_path", "offset", "limit", "pages", "allow_large"].contains(&k.as_str())
                }) {
                    return Err(format!("unsupported Read option: {key}"));
                }
                if input.get("allow_large").is_some_and(|v| !v.is_boolean()) {
                    return Err("allow_large must be a boolean".into());
                }
                let file_path = Self::field(&input, "file_path")?;
                if let Some(error) = super::text_files::binary(file_path) {
                    return Err(error);
                }
                if let Some(error) = this.unreadable(file_path)? {
                    return Ok(ToolOutput::error(error));
                }
                let (_, path) = this.file(file_path)?;
                media = crate::tools::media::read(&path, &input, &this.media, session.as_ref())?;
            }
            Ok(match media {
                Some(output) => output,
                None if name == "Read" => match this.read(&input) {
                    Ok(text) => ToolOutput::text(text),
                    Err(error) => ToolOutput::error(error),
                },
                None => ToolOutput::text(this.execute_sync(&name, &input)?),
            })
        })
        .await
        .map_err(|e| format!("workspace task: {e}"))?
    }

    fn execute_sync(&self, name: &str, input: &Value) -> Result<String, String> {
        super::text_files::check(name, input)?;
        match name {
            "Read" => self.read(input),
            "Write" => self.write(input),
            "Edit" => self.edit(input),
            _ => Err(format!("unknown workspace tool: {name}")),
        }
    }

    fn field<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
        input
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("missing or invalid {key}"))
    }

    fn resolve(&self, text: &str) -> Result<PathBuf, String> {
        if text.is_empty() {
            return Err("file path is empty".into());
        }
        Ok(self.base.join(text))
    }

    fn relative_path(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.root).unwrap_or(path).to_path_buf()
    }

    fn existing(&self, text: &str) -> Result<(PathBuf, PathBuf), String> {
        let real = fs::canonicalize(self.resolve(text)?).map_err(|e| format!("path: {e}"))?;
        let real = self.inside(real)?;
        Ok((self.relative_path(&real), real))
    }

    fn file(&self, text: &str) -> Result<(PathBuf, PathBuf), String> {
        let (rel, real) = self.existing(text)?;
        if !real.is_file() {
            return Err("path is not a regular file".into());
        }
        Ok((rel, real))
    }

    fn unreadable(&self, text: &str) -> Result<Option<String>, String> {
        let path = self.resolve(text)?;
        match fs::canonicalize(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Some(self.missing(&path))),
            Ok(real) => Ok(self.inside(real)?.is_dir().then(|| {
                format!(
                    "EISDIR: illegal operation on a directory, read '{}'",
                    path.display()
                )
            })),
            Err(_) => Ok(None),
        }
    }

    fn missing(&self, path: &Path) -> String {
        let mut text = format!(
            "File does not exist. Note: your current working directory is {}.",
            self.base.display()
        );
        if let Some(similar) = self.beside(path).or_else(|| similar(path)) {
            text.push_str(&format!(" Did you mean {similar}?"));
        }
        text
    }

    fn beside(&self, path: &Path) -> Option<String> {
        let parent = self.base.parent()?;
        let path = path
            .parent()
            .and_then(|directory| fs::canonicalize(directory).ok())
            .and_then(|directory| Some(directory.join(path.file_name()?)))
            .unwrap_or_else(|| path.to_path_buf());
        let rest = path.strip_prefix(parent).ok()?;
        if rest.as_os_str().is_empty() || path.starts_with(&self.base) {
            return None;
        }
        let candidate = self.base.join(rest);
        candidate.exists().then(|| candidate.display().to_string())
    }

    fn seen(&self, path: &Path, view: View) {
        if let Some(modified) = modified(path) {
            self.reads.update(|reads| {
                reads.insert(path.to_path_buf(), Seen { modified, view });
            });
        }
    }

    fn read_text(path: &Path) -> Result<String, String> {
        fs::read_to_string(path).map_err(|e| format!("read: {e}"))
    }

    fn read(&self, input: &Value) -> Result<String, String> {
        if input.get("pages").is_some() {
            return Err("pages is only applicable to PDF files".into());
        }
        let file_path = Self::field(input, "file_path")?;
        let (_, path) = self.file(file_path)?;
        let metadata = fs::metadata(&path).map_err(|e| format!("read: {e}"))?;
        let size = metadata.len();
        let read =
            super::text_files::read_view(file_path, size, input, self.read_token_budget, || {
                let mut file = fs::File::open(&path).map_err(|e| format!("read: {e}"))?;
                let mut last = [0u8];
                let trailing = size > 0
                    && file.seek(SeekFrom::End(-1)).is_ok()
                    && file.read_exact(&mut last).is_ok()
                    && last[0] == b'\n';
                file.seek(SeekFrom::Start(0))
                    .map_err(|e| format!("read: {e}"))?;
                Ok(BufReader::new(file)
                    .lines()
                    .map(|line| line.map_err(|e| format!("read: {e}")))
                    .chain(trailing.then(|| Ok(String::new()))))
            });
        let (text, whole) = read?;
        if let Ok(modified) = metadata.modified() {
            let view = match whole {
                true => View::Current(digest(&fs::read(&path).map_err(|e| format!("read: {e}"))?)),
                false => View::Part,
            };
            self.reads.update(|reads| {
                reads.insert(path.clone(), Seen { modified, view });
            });
        }
        Ok(text)
    }

    fn write_target(&self, text: &str) -> Result<(PathBuf, PathBuf), String> {
        let mut path = self.resolve(text)?;
        loop {
            match fs::canonicalize(&path) {
                Ok(real) => {
                    let real = self.inside(real)?;
                    if !real.is_file() {
                        return Err("target is not a regular file".into());
                    }
                    return Ok((self.relative_path(&real), real));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    match fs::symlink_metadata(&path) {
                        Ok(meta) if meta.file_type().is_symlink() => {
                            let target =
                                fs::read_link(&path).map_err(|e| format!("read link: {e}"))?;
                            path = path.parent().ok_or("file has no parent")?.join(target);
                            continue;
                        }
                        Ok(_) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(format!("check target: {e}")),
                    }
                    let parent = path.parent().ok_or("file has no parent")?;
                    self.creatable(parent)?;
                    fs::create_dir_all(parent).map_err(|e| format!("create directory: {e}"))?;
                    let parent = fs::canonicalize(parent).map_err(|e| format!("parent: {e}"))?;
                    let parent = self.inside(parent)?;
                    let path = parent.join(path.file_name().ok_or("file has no name")?);
                    return Ok((self.relative_path(&path), path));
                }
                Err(e) => return Err(format!("path: {e}")),
            }
        }
    }

    fn write(&self, input: &Value) -> Result<String, String> {
        let content = Self::field(input, "content")?;
        let (rel, path) = self.write_target(Self::field(input, "file_path")?)?;
        let existed = path.exists();
        atomic_write(&path, content)?;
        self.seen(&path, View::Current(digest(content.as_bytes())));
        let shown = self.root.join(&rel).display().to_string();
        Ok(super::text_files::written(&shown, existed))
    }

    fn edit(&self, input: &Value) -> Result<String, String> {
        let text = Self::field(input, "file_path")?;
        let edit = super::text_files::Edit::parse(input)?;
        let target = self.resolve(text)?;
        match fs::canonicalize(&target) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if !edit.creates() {
                    return Err(self.missing(&target));
                }
                let (updated, _) = edit.apply("")?;
                let (rel, path) = self.write_target(text)?;
                atomic_write(&path, &updated)?;
                self.seen(&path, View::Current(digest(updated.as_bytes())));
                return Ok(super::text_files::edited(
                    &self.root.join(&rel).display().to_string(),
                    edit.replace_all(),
                ));
            }
            Ok(real) => {
                if self.inside(real)?.is_dir() {
                    return Err(
                        "Error calling tool (Edit): EISDIR: illegal operation on a directory, read"
                            .into(),
                    );
                }
            }
            Err(_) => {}
        }
        let (rel, existing) = self.file(text)?;
        let content = Self::read_text(&existing)?;
        let current = digest(content.as_bytes());
        let seen = self.reads.read(|reads| reads.get(&existing).copied());
        let stale = seen.is_some_and(|seen| {
            modified(&existing).is_some_and(|modified| modified > seen.modified)
                && !matches!(seen.view, View::Whole(known) | View::Current(known) if known == current)
        });
        let (updated, _) = match edit.apply(&content) {
            Err(_) if stale && edit.changes() && !edit.creates() => {
                return Err(super::text_files::MODIFIED_SINCE_READ.into());
            }
            applied => applied?,
        };
        let known = match seen.map(|seen| seen.view) {
            _ if stale => Known::Stale,
            Some(View::Current(known)) if known == current => Known::Current,
            _ => Known::Unread,
        };
        let (_, path) = self.write_target(text)?;

        if Self::read_text(&path)? != content {
            return Err("file changed during edit".into());
        }
        atomic_write(&path, &updated)?;
        let updated = digest(updated.as_bytes());
        self.seen(
            &path,
            match known {
                Known::Current => View::Current(updated),
                Known::Unread | Known::Stale => View::Whole(updated),
            },
        );
        Ok(super::text_files::edited_with(
            &self.root.join(&rel).display().to_string(),
            edit.replace_all(),
            known,
        ))
    }
}

fn digest(bytes: &[u8]) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn similar(path: &Path) -> Option<String> {
    let name = path.file_name()?;
    let stem = path.file_stem()?;
    fs::read_dir(path.parent()?)
        .ok()?
        .flatten()
        .map(|entry| entry.file_name())
        .find(|entry| {
            let entry = Path::new(entry);
            entry.as_os_str() != name
                && entry.file_stem() == Some(stem)
                && entry.extension().is_none_or(|extension| {
                    extension.to_str().is_some_and(|extension| {
                        (1..=32).contains(&extension.chars().count())
                            && extension
                                .chars()
                                .all(|c| c.is_alphanumeric() || "_+~#-".contains(c))
                    })
                })
        })?
        .into_string()
        .ok()
}

fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let parent = path.parent().ok_or("file has no parent")?;
    let existing = fs::metadata(path).ok();
    #[cfg(unix)]
    let permissions = existing.is_none().then(|| {
        use std::os::unix::fs::PermissionsExt;
        fs::Permissions::from_mode(0o666)
    });
    #[cfg(not(unix))]
    let permissions: Option<fs::Permissions> = None;
    let mut file =
        nanocode_platform::TemporaryFile::with_permissions_in(parent, permissions.as_ref())
            .map_err(|e| format!("create temp file: {e}"))?;
    file.write_all(content.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    flush_to_disk(file.as_file()).map_err(|e| format!("sync: {e}"))?;
    if let Some(meta) = existing {
        file.as_file()
            .set_permissions(meta.permissions())
            .map_err(|e| format!("permissions: {e}"))?;
    }
    file.persist(path).map_err(|e| format!("rename: {e}"))?;
    Ok(())
}

#[cfg(unix)]
fn flush_to_disk(file: &fs::File) -> std::io::Result<()> {
    nanocode_platform::unix::sync_file(file)
}

#[cfg(not(unix))]
fn flush_to_disk(file: &fs::File) -> std::io::Result<()> {
    file.sync_all()
}
