use std::{
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
use serde_json::{Value, json, value::to_raw_value};

use super::{
    StandardTool, Tool, ToolContext, ToolInput, ToolOutput, ToolResult,
    workspace_runtime::WorkspaceToolRuntime,
};

pub type ServedReport = fn(&'static str, &dyn Display);

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
        }
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
        Ok(self
            .runtime
            .execute_tool(
                self.kind.name(),
                ToolInput::Function(to_raw_value(&args)?),
                context,
            )
            .await)
    }
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
