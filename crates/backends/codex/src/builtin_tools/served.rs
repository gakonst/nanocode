use std::{
    cmp::Reverse,
    collections::BTreeMap,
    fmt::Display,
    fs::{self, OpenOptions},
    io::{ErrorKind, Read, Seek, SeekFrom},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use nanocode_sync::Atom;
use nanocode_tools::ToolDefinition;
use serde::Deserialize;
use serde_json::{
    Value, json,
    value::{RawValue, to_raw_value},
};

use super::{
    StandardTool, Tool, ToolContext, ToolInput, ToolOutput, ToolOutputBody, ToolOutputContent,
    ToolResult, workspace_runtime::WorkspaceToolRuntime,
};

pub type ServedReport = fn(&'static str, &dyn Display);

pub type Redactions = Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

const PROXY_VARIABLES: [&str; 6] = [
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
];
const MIN_PROXY_PASSWORD: usize = 8;

pub struct ServedDirectories {
    file: PathBuf,
    home: PathBuf,
    refusal: &'static str,
    report: ServedReport,
    read: Atom<Option<Arc<Listed>>>,
}

struct Listed {
    file: fs::File,
    stamp: FileStamp,
    directories: Arc<Vec<PathBuf>>,
    origin: Instant,
    checked: AtomicU64,
    checking: AtomicBool,
}

type FileStamp = (u64, Option<SystemTime>);

fn stamp(metadata: &fs::Metadata) -> FileStamp {
    (metadata.len(), metadata.modified().ok())
}

impl ServedDirectories {
    #[must_use]
    pub fn new(file: PathBuf, home: PathBuf, refusal: &'static str, report: ServedReport) -> Self {
        Self {
            file,
            home,
            refusal,
            report,
            read: Atom::default(),
        }
    }

    fn logged<E: Display>(&self, message: &'static str) -> impl FnOnce(E) -> &'static str + '_ {
        move |error| {
            (self.report)(message, &error);
            message
        }
    }

    fn directories(&self) -> Result<Arc<Vec<PathBuf>>, &'static str> {
        const UNAVAILABLE: &str = "served directories are unavailable";
        if let Some(listed) = self.read.get().as_ref()
            && self.current(listed)
        {
            return Ok(Arc::clone(&listed.directories));
        }
        let mut file = fs::File::open(&self.file).map_err(self.logged(UNAVAILABLE))?;
        let metadata = file.metadata().map_err(self.logged(UNAVAILABLE))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(self.logged(UNAVAILABLE))?;
        let directories: Arc<Vec<PathBuf>> = Arc::new(
            serde_json::from_slice(&bytes)
                .map_err(self.logged("served directories are unreadable"))?,
        );
        self.read.set(Some(Arc::new(Listed {
            file,
            stamp: stamp(&metadata),
            directories: Arc::clone(&directories),
            origin: Instant::now(),
            checked: AtomicU64::new(0),
            checking: AtomicBool::new(false),
        })));
        Ok(directories)
    }

    fn current(&self, listed: &Listed) -> bool {
        let Ok(open) = listed.file.metadata() else {
            return false;
        };
        if open.nlink() == 0 || stamp(&open) != listed.stamp {
            return false;
        }
        if listed.checking.swap(true, Ordering::Acquire) {
            return true;
        }
        let now = u64::try_from(listed.origin.elapsed().as_millis()).unwrap_or(u64::MAX);
        let same = now.saturating_sub(listed.checked.load(Ordering::Relaxed)) < 1000
            || fs::metadata(&self.file)
                .is_ok_and(|path| (path.dev(), path.ino()) == (open.dev(), open.ino()))
                && {
                    listed.checked.store(now, Ordering::Relaxed);
                    true
                };
        listed.checking.store(false, Ordering::Release);
        same
    }

    fn resolve(&self, workdir: Option<&Value>) -> Result<PathBuf, &'static str> {
        let requested = workdir
            .and_then(Value::as_str)
            .map(Path::new)
            .filter(|path| path.is_absolute())
            .ok_or(self.refusal)?;
        let served = self.directories()?;
        if served.iter().any(|dir| dir == requested)
            && fs::symlink_metadata(requested).is_ok_and(|metadata| metadata.is_dir())
        {
            return Ok(requested.to_owned());
        }
        let canonical = requested
            .canonicalize()
            .map_err(self.logged(self.refusal))?;
        let mut dirs = served.iter().chain([&self.home]);
        (dirs.clone().any(|dir| *dir == canonical)
            || dirs.any(|dir| dir.canonicalize().is_ok_and(|dir| dir == canonical)))
        .then_some(canonical)
        .filter(|dir| dir.is_dir())
        .ok_or(self.refusal)
    }
}

pub struct ServedTools {
    directories: Arc<ServedDirectories>,
    runtime: Arc<WorkspaceToolRuntime>,
    read_file: ToolDefinition,
    chunk: u64,
    redactions: Option<Redactions>,
}

impl ServedTools {
    #[must_use]
    pub fn new(
        directories: ServedDirectories,
        runtime: Arc<WorkspaceToolRuntime>,
        read_file: ToolDefinition,
        chunk: u64,
    ) -> Self {
        Self {
            directories: Arc::new(directories),
            runtime,
            read_file,
            chunk,
            redactions: None,
        }
    }

    #[must_use]
    pub fn redacting(mut self, redactions: Redactions) -> Self {
        self.redactions = Some(redactions);
        self
    }
}

impl nanocode_tool_host::ToolSource for ServedTools {
    fn install(
        self,
        builder: nanocode_tool_host::ToolsBuilder,
    ) -> nanocode_tool_host::ToolsBuilder {
        let mut builder = builder;
        for kind in [StandardTool::ExecCommand, StandardTool::WriteStdin] {
            builder = builder.add(ServedShell {
                kind,
                directories: Arc::clone(&self.directories),
                runtime: Arc::clone(&self.runtime),
                redactions: self.redactions.clone(),
            });
        }
        builder.add(ReadFile {
            directories: self.directories,
            definition: self.read_file,
            chunk: self.chunk,
        })
    }
}

struct ServedShell {
    kind: StandardTool,
    directories: Arc<ServedDirectories>,
    runtime: Arc<WorkspaceToolRuntime>,
    redactions: Option<Redactions>,
}

#[async_trait::async_trait]
impl Tool for ServedShell {
    fn definition(&self) -> ToolDefinition {
        self.kind.definition()
    }

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let mut args: Value = input.decode_json()?;
        let Some(object) = args.as_object_mut() else {
            return Ok(ToolOutput::error("expected shell arguments"));
        };
        if self.kind == StandardTool::ExecCommand {
            let workdir = match self.directories.resolve(object.get("workdir")) {
                Ok(workdir) => workdir,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            object.insert(
                "workdir".into(),
                Value::String(workdir.to_string_lossy().into_owned()),
            );
            for field in ["sandbox_permissions", "justification", "prefix_rule"] {
                object.remove(field);
            }
        }
        let output = self
            .runtime
            .execute_tool(
                self.kind.name(),
                ToolInput::Function(to_raw_value(&args)?),
                context,
            )
            .await;
        Ok(match &self.redactions {
            Some(redactions) => redact(output, &redactions()),
            None => output,
        })
    }
}

#[must_use]
pub fn proxy_redactions(secrets: Option<PathBuf>) -> Redactions {
    Arc::new(move || {
        let stored: BTreeMap<String, String> = secrets
            .as_ref()
            .and_then(|file| fs::read(file).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let mut forms = Vec::new();
        for name in PROXY_VARIABLES {
            let values = std::env::var(name)
                .ok()
                .into_iter()
                .chain(stored.get(name).cloned());
            for url in values {
                credential_forms(name, &url, &mut forms);
            }
        }
        forms.sort_by_key(|(form, _)| Reverse(form.len()));
        forms.dedup_by(|a, b| a.0 == b.0);
        forms
    })
}

fn credential_forms(name: &str, url: &str, forms: &mut Vec<(String, String)>) {
    let Some((_, rest)) = url.split_once("://") else {
        return;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let Some((user, password)) = authority
        .rsplit_once('@')
        .and_then(|(userinfo, _)| userinfo.split_once(':'))
    else {
        return;
    };
    if password.len() < MIN_PROXY_PASSWORD {
        return;
    }
    let placeholder = format!("[secret {name}]");
    let (decoded_user, decoded_password) = (percent_decode(user), percent_decode(password));
    for form in [
        password.to_owned(),
        decoded_password.clone(),
        BASE64.encode(format!("{user}:{password}")),
        BASE64.encode(format!("{decoded_user}:{decoded_password}")),
    ] {
        forms.push((form, placeholder.clone()));
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let hex = bytes
            .get(index + 1..index + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (byte, hex) {
            (b'%', Some(decoded)) => {
                out.push(decoded);
                index += 3;
            }
            (byte, _) => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn scrub(text: &mut String, forms: &[(String, String)]) -> bool {
    let mut found = false;
    for (form, placeholder) in forms {
        if text.contains(form.as_str()) {
            *text = text.replace(form.as_str(), placeholder);
            found = true;
        }
    }
    found
}

fn scrub_json(value: &mut Value, forms: &[(String, String)]) -> bool {
    match value {
        Value::String(text) => scrub(text, forms),
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |found, item| scrub_json(item, forms) | found),
        Value::Object(fields) => fields
            .values_mut()
            .fold(false, |found, item| scrub_json(item, forms) | found),
        _ => false,
    }
}

fn redact(mut output: ToolOutput, forms: &[(String, String)]) -> ToolOutput {
    if forms.is_empty() {
        return output;
    }
    match &mut output.output {
        ToolOutputBody::Text(text) => {
            scrub(text, forms);
        }
        ToolOutputBody::Content(items) => {
            for item in items {
                if let ToolOutputContent::InputText { text } = item {
                    scrub(text, forms);
                }
            }
        }
    }
    if let Some(metadata) = &output.metadata {
        let mut text = metadata.get().to_owned();
        if scrub(&mut text, forms) {
            output.metadata = RawValue::from_string(text).ok();
        }
    }
    let mut structured = output.structured_result();
    if scrub_json(&mut structured, forms) {
        output = output.with_structured_result(structured);
    }
    output
}

struct ReadFile {
    directories: Arc<ServedDirectories>,
    definition: ToolDefinition,
    chunk: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    workdir: Value,
    path: String,
    #[serde(default)]
    offset: u64,
    length: Option<u64>,
}

enum Outcome {
    Missing,
    Link(String),
    Chunk {
        size: u64,
        modified_ns: u64,
        data: Vec<u8>,
    },
}

impl ReadFile {
    fn read(&self, args: &ReadArgs) -> Result<Outcome, &'static str> {
        let served = &self.directories;
        let root = served.resolve(Some(&args.workdir))?;
        let parts: Vec<&str> = args.path.split('/').collect();
        if args.path.is_empty()
            || args.path.chars().any(char::is_control)
            || parts.iter().any(|p| matches!(*p, "" | "." | ".."))
        {
            return Err("path must be relative to workdir, without . or .. components");
        }
        let mut path = root;
        let mut info = None;
        for (index, part) in parts.iter().enumerate() {
            path.push(part);
            let meta = match fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_symlink() && index + 1 == parts.len() => {
                    let target = fs::read_link(&path).map_err(served.logged("file unreadable"))?;
                    return target
                        .to_str()
                        .map(|target| Outcome::Link(target.to_owned()))
                        .ok_or("symbolic link target is not UTF-8");
                }
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err("symbolic links are not followed");
                }
                Ok(meta) => meta,
                Err(error)
                    if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
                {
                    return Ok(Outcome::Missing);
                }
                Err(_) => return Err("file unreadable"),
            };
            if index + 1 < parts.len() && !meta.is_dir() {
                return Ok(Outcome::Missing);
            }
            info = Some(meta);
        }
        let expected = info.ok_or("file unreadable")?;
        if !expected.is_file() {
            return Err("not a regular file");
        }
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .map_err(served.logged("file unreadable"))?;
        let opened = file.metadata().map_err(served.logged("file unreadable"))?;
        if (opened.dev(), opened.ino()) != (expected.dev(), expected.ino()) {
            return Err("file changed while reading");
        }
        let modified_ns = opened
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .and_then(|elapsed| u64::try_from(elapsed.as_nanos()).ok())
            .unwrap_or_default();
        let mut data = Vec::new();
        let length = args.length.unwrap_or(self.chunk).min(self.chunk);
        file.seek(SeekFrom::Start(args.offset))
            .and_then(|_| Read::by_ref(&mut file).take(length).read_to_end(&mut data))
            .map_err(served.logged("file unreadable"))?;
        Ok(Outcome::Chunk {
            size: opened.len(),
            modified_ns,
            data,
        })
    }
}

#[async_trait::async_trait]
impl Tool for ReadFile {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    async fn execute(&self, input: ToolInput, _context: ToolContext<'_>) -> ToolResult {
        let args: ReadArgs = input.decode_json()?;
        Ok(match self.read(&args) {
            Ok(Outcome::Missing) => {
                ToolOutput::text("file not found").with_structured_result(json!({"exists":false}))
            }
            Ok(Outcome::Link(target)) => ToolOutput::text("symbolic link, not followed")
                .with_structured_result(json!({"exists":true,"link":target})),
            Ok(Outcome::Chunk {
                size,
                modified_ns,
                data,
            }) => ToolOutput::text(format!("read {} of {size} bytes", data.len()))
                .with_structured_result(json!({
                    "exists": true,
                    "size": size,
                    "modified_ns": modified_ns,
                    "offset": args.offset,
                    "data": BASE64.encode(&data),
                })),
            Err(message) => ToolOutput::error(message),
        })
    }
}
