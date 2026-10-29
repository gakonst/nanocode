use std::{
    collections::BTreeMap,
    hash::Hasher,
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::symlink,
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    time::SystemTime,
};

const RUST_KEY: &str = "nanocode-rustc-cache-5";
const WORKSPACE_KEY: &str = "nanocode-workspace-cache-1";
const C_KEY: &str = "nanocode-c-cache-3 -ffile-prefix-map=<out>=. -MD";
const MEMO: &str = ".nanocode-sha256";

type Environment = BTreeMap<OsString, OsString>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Registry,
    Workspace,
}

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let Some(tool) = args.first() else {
        return ExitCode::from(2);
    };
    let cache = cache_dir();
    let environment: Environment = env::vars_os().collect();
    if is_rustc(tool) {
        rust(&cache, args, environment, 1)
    } else if args.get(1).is_some_and(|compiler| is_rustc(compiler)) {
        rust(&cache, args, environment, 2)
    } else if environment.contains_key(OsStr::new("OUT_DIR")) {
        c(&cache, &args, environment)
    } else {
        exec(&args, &environment)
    }
}

fn is_rustc(path: &OsStr) -> bool {
    path.as_bytes().ends_with(b"rustc")
}

fn cache_dir() -> PathBuf {
    env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".cache"))
        .join("nanocode/build")
}

fn exec(args: &[OsString], environment: &Environment) -> ExitCode {
    let error = Command::new(&args[0])
        .args(&args[1..])
        .env_clear()
        .envs(environment)
        .exec();
    eprintln!("buildcache: could not run {}: {error}", args[0].to_string_lossy());
    ExitCode::from(1)
}

fn exit_code(status: std::process::ExitStatus) -> ExitCode {
    ExitCode::from(status.code().map_or(1, |code| code as u8))
}

fn rust(cache: &Path, mut args: Vec<OsString>, mut environment: Environment, compiler: usize) -> ExitCode {
    for name in ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR"] {
        environment.remove(OsStr::new(name));
    }
    if let Some(out_dir) = environment.get(OsStr::new("OUT_DIR")).cloned()
        && Path::new(&out_dir).is_dir()
        && let Some(stable) = stable(cache, Path::new(&out_dir))
    {
        environment.insert("OUT_DIR".into(), stable.into_os_string());
    }
    let registry = PathBuf::from(
        environment
            .get(OsStr::new("CARGO_HOME"))
            .cloned()
            .unwrap_or_else(|| {
                PathBuf::from(environment.get(OsStr::new("HOME")).cloned().unwrap_or_default())
                    .join(".cargo")
                    .into_os_string()
            }),
    )
    .join("registry/src/");
    let Ok(cwd) = env::current_dir() else {
        return exec(&args, &environment);
    };
    let mut workspace_root = cwd.as_os_str().as_bytes().to_vec();
    workspace_root.push(b'/');
    let mut keyed: Vec<Vec<u8>> = Vec::with_capacity(args.len());
    let mut externs: Vec<Vec<u8>> = Vec::new();
    let (mut out_dir, mut suffix, mut kind, mut eligible) = (None, None, None, true);
    for index in compiler..args.len() {
        let previous = args[index - 1].as_bytes().to_vec();
        let arg = args[index].clone();
        let bytes = arg.as_bytes();
        let mut key = bytes.to_vec();
        match previous.as_slice() {
            b"-L" => {
                if let Some(at) = bytes.iter().position(|byte| *byte == b'=') {
                    let (prefix, path) = (&bytes[..at], &bytes[at + 1..]);
                    if prefix == b"dependency" {
                        key = b"dependency=".to_vec();
                    } else if is_build_out(path) && Path::new(OsStr::from_bytes(path)).is_dir()
                        && let Some(stable) = stable(cache, Path::new(OsStr::from_bytes(path)))
                    {
                        let mut rewritten = prefix.to_vec();
                        rewritten.push(b'=');
                        rewritten.extend_from_slice(stable.as_os_str().as_bytes());
                        key.clone_from(&rewritten);
                        args[index] = OsString::from_vec(rewritten);
                    }
                }
            }
            b"--out-dir" => {
                out_dir = Some(PathBuf::from(&arg));
                key.clear();
            }
            b"--extern" => {
                if let Some(at) = bytes.iter().position(|byte| *byte == b'=') {
                    let path = Path::new(OsStr::from_bytes(&bytes[at + 1..]));
                    key = bytes[..at].to_vec();
                    key.push(b'=');
                    key.extend_from_slice(path.file_name().map_or(b"" as &[u8], OsStrExt::as_bytes));
                    match identity(path) {
                        Some(identity) => {
                            let mut line = key.clone();
                            line.push(b' ');
                            line.extend_from_slice(identity.as_bytes());
                            externs.push(line);
                        }
                        None => eligible = false,
                    }
                }
            }
            b"-C" => {
                if let Some(value) = bytes.strip_prefix(b"extra-filename=") {
                    suffix = Some(value.to_vec());
                } else if bytes.starts_with(b"incremental=") {
                    key = b"incremental=".to_vec();
                }
            }
            _ => {}
        }
        if bytes.starts_with(b"@") {
            eligible = false;
        }
        if bytes.ends_with(b".rs") {
            if bytes.starts_with(registry.as_os_str().as_bytes()) {
                kind = Some(Kind::Registry);
            } else if !bytes.starts_with(b"/") || bytes.starts_with(&workspace_root) {
                kind = Some(Kind::Workspace);
            }
        }
        keyed.push(key);
    }
    let (Some(out_dir), Some(suffix), Some(kind), true) = (out_dir, suffix, kind, eligible) else {
        return exec(&args, &environment);
    };
    let Some(version) = rustc_version(cache, Path::new(&args[compiler - 1])) else {
        return exec(&args, &environment);
    };
    let mut hasher = Sha256::new();
    hasher.update(if kind == Kind::Registry { RUST_KEY } else { WORKSPACE_KEY }.as_bytes());
    hasher.update(b"\n");
    hasher.update(version.as_bytes());
    hasher.update(b"\n");
    let mut clippy = false;
    for wrapper in &args[..compiler - 1] {
        let Some(digest) = tool_digest(cache, Path::new(wrapper)) else {
            return exec(&args, &environment);
        };
        clippy |= Path::new(wrapper)
            .file_name()
            .is_some_and(|name| name.as_bytes().windows(6).any(|window| window == b"clippy"));
        hasher.update(digest.as_bytes());
        hasher.update(b"\n");
    }
    if kind == Kind::Workspace {
        hasher.update(&workspace_root);
        hasher.update(b"\n");
    }
    for key in &keyed {
        hasher.update(key);
        hasher.update(b"\n");
    }
    if kind == Kind::Registry {
        for line in &externs {
            hasher.update(line);
            hasher.update(b"\n");
        }
    }
    for (name, value) in &environment {
        let name = name.as_bytes();
        let relevant = (name.starts_with(b"CARGO_") && name != b"CARGO_MAKEFLAGS" && (clippy || name != b"CARGO_PRIMARY_PACKAGE"))
            || (clippy && name.starts_with(b"CLIPPY_"))
            || matches!(name, b"OUT_DIR" | b"RUSTC_BOOTSTRAP" | b"MACOSX_DEPLOYMENT_TARGET" | b"SDKROOT");
        if relevant {
            hasher.update(name);
            hasher.update(b"=");
            hasher.update(value.as_bytes());
            hasher.update(b"\n");
        }
    }
    let key = hasher.finish();
    let inputs = match kind {
        Kind::Registry => Vec::new(),
        Kind::Workspace => {
            let manifest = environment
                .get(OsStr::new("CARGO_MANIFEST_PATH"))
                .map(PathBuf::from)
                .or_else(|| environment.get(OsStr::new("CARGO_MANIFEST_DIR")).map(|dir| Path::new(dir).join("Cargo.toml")));
            let mut inputs = Vec::new();
            for line in &externs {
                inputs.extend_from_slice(line);
                inputs.push(b'\n');
            }
            for path in manifest.iter().map(PathBuf::as_path).chain([cwd.join("Cargo.toml").as_path()]) {
                let Ok(digest) = digest_file(path) else {
                    return exec(&args, &environment);
                };
                inputs.extend_from_slice(digest.as_bytes());
                inputs.push(b' ');
                inputs.extend_from_slice(path.as_os_str().as_bytes());
                inputs.push(b'\n');
            }
            inputs
        }
    };
    let entry = match kind {
        Kind::Registry => cache.join("units").join(&key),
        Kind::Workspace => cache.join("workspace").join(&key),
    };
    if restore(&entry, &out_dir, &environment, kind, &inputs).is_ok() {
        return ExitCode::SUCCESS;
    }
    let started = SystemTime::now();
    let mut child = match Command::new(&args[0])
        .args(&args[1..])
        .env_clear()
        .envs(&environment)
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            eprintln!("buildcache: could not run {}: {error}", args[0].to_string_lossy());
            return ExitCode::from(1);
        }
    };
    let mut marker = Marker {
        kind,
        key: &key,
        metadata: None,
    };
    let mut captured = Vec::new();
    if let Some(mut pipe) = child.stderr.take() {
        let mut buffer = [0; 65536];
        let mut pending = Vec::new();
        let mut stderr = io::stderr();
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    pending.extend_from_slice(&buffer[..read]);
                    while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                        let line: Vec<u8> = pending.drain(..=end).collect();
                        if let Some((path, emit)) = artifact(&line) {
                            marker.mark(&path, &emit);
                        }
                        let _ = stderr.write_all(&line);
                        let _ = stderr.flush();
                        captured.extend_from_slice(&line);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = stderr.write_all(&pending);
        let _ = stderr.flush();
        captured.extend_from_slice(&pending);
    }
    let status = match child.wait() {
        Ok(status) => status,
        Err(_) => return ExitCode::from(1),
    };
    if !status.success() {
        return exit_code(status);
    }
    let outputs = outputs(&out_dir, &suffix);
    let mut identities = Vec::new();
    for output in &outputs {
        let name = output.file_name().map_or(b"" as &[u8], OsStrExt::as_bytes);
        if name.ends_with(b".d") {
            continue;
        }
        if fresh_memo(output).is_none() {
            marker.mark(output, if name.ends_with(b".rmeta") { b"metadata" } else { b"link" });
        }
        if let Some(identity) = fresh_memo(output) {
            identities.extend_from_slice(name);
            identities.push(b' ');
            identities.extend_from_slice(identity.as_bytes());
            identities.push(b'\n');
        }
    }
    let Some(depinfo) = outputs
        .iter()
        .find(|path| path.extension() == Some(OsStr::new("d")))
        .and_then(|path| fs::read(path).ok())
    else {
        return ExitCode::SUCCESS;
    };
    let (dependencies, names) = dependencies(&depinfo);
    let stable_root = cache.join("out");
    let inside = |path: &[u8]| {
        path.starts_with(registry.as_os_str().as_bytes()) || path.starts_with(stable_root.as_os_str().as_bytes())
    };
    let (identity, sources) = match kind {
        Kind::Registry => {
            if !dependencies.iter().all(|path| inside(path)) {
                return ExitCode::SUCCESS;
            }
            (key.clone(), Vec::new())
        }
        Kind::Workspace => {
            let mut sources = Vec::new();
            for path in &dependencies {
                let local = !path.starts_with(b"/") || path.starts_with(&workspace_root);
                if !local && !inside(path) {
                    return ExitCode::SUCCESS;
                }
                if unchanged_since(Path::new(OsStr::from_bytes(path)), started).is_none() {
                    return ExitCode::SUCCESS;
                }
                let Ok(digest) = digest_file(Path::new(OsStr::from_bytes(path))) else {
                    return ExitCode::SUCCESS;
                };
                sources.extend_from_slice(digest.as_bytes());
                sources.push(b' ');
                sources.extend_from_slice(path);
                sources.push(b'\n');
            }
            let mut hasher = Sha256::new();
            hasher.update(key.as_bytes());
            hasher.update(b"\n");
            hasher.update(&inputs);
            hasher.update(b"\n");
            hasher.update(&sources);
            (hasher.finish(), sources)
        }
    };
    let Some(recorded) = recorded_environment(&names, &environment) else {
        return ExitCode::SUCCESS;
    };
    let unit = Unit {
        out_dir: &out_dir,
        outputs: &outputs,
        captured: &captured,
        recorded: &recorded,
        inputs: &inputs,
        sources: &sources,
        identities: &identities,
        identity: &identity,
    };
    let _ = match kind {
        Kind::Registry => store(cache, &entry, &unit),
        Kind::Workspace => store_workspace(cache, &entry, &unit),
    };
    ExitCode::SUCCESS
}

struct Unit<'a> {
    out_dir: &'a Path,
    outputs: &'a [PathBuf],
    captured: &'a [u8],
    recorded: &'a [u8],
    inputs: &'a [u8],
    sources: &'a [u8],
    identities: &'a [u8],
    identity: &'a str,
}

struct Marker<'a> {
    kind: Kind,
    key: &'a str,
    metadata: Option<String>,
}

impl Marker<'_> {
    fn mark(&mut self, path: &Path, emit: &[u8]) {
        let name = path.file_name().map_or(b"" as &[u8], OsStrExt::as_bytes);
        let identity = match (self.kind, emit) {
            (_, b"dep-info") => return,
            (Kind::Registry, _) => self.key.to_owned(),
            (Kind::Workspace, b"metadata") => {
                let Ok(digest) = digest_file(path) else {
                    return;
                };
                self.metadata = Some(digest.clone());
                digest
            }
            (Kind::Workspace, _) if name.ends_with(b".rlib") && self.metadata.is_some() => {
                self.metadata.clone().unwrap_or_default()
            }
            (Kind::Workspace, _) if name.starts_with(b"lib") => {
                let Ok(digest) = digest_file(path) else {
                    return;
                };
                digest
            }
            (Kind::Workspace, _) => return,
        };
        write_memo(path, &identity);
    }
}

fn artifact(line: &[u8]) -> Option<(PathBuf, Vec<u8>)> {
    if !line.windows(29).any(|window| window == br#""$message_type":"artifact""#) {
        return None;
    }
    let path = json_string(line, br#""artifact":""#)?;
    let emit = json_string(line, br#""emit":""#)?;
    Some((PathBuf::from(OsString::from_vec(path)), emit))
}

fn json_string(line: &[u8], field: &[u8]) -> Option<Vec<u8>> {
    let start = line.windows(field.len()).position(|window| window == field)? + field.len();
    let mut value = Vec::new();
    let mut bytes = line[start..].iter();
    while let Some(&byte) = bytes.next() {
        match byte {
            b'"' => return Some(value),
            b'\\' => match bytes.next()? {
                escaped @ (b'"' | b'\\' | b'/') => value.push(*escaped),
                _ => return None,
            },
            _ => value.push(byte),
        }
    }
    None
}

fn is_build_out(path: &[u8]) -> bool {
    path.ends_with(b"/out") && path.windows(7).any(|window| window == b"/build/")
}

fn outputs(out_dir: &Path, suffix: &[u8]) -> Vec<PathBuf> {
    let mut outputs: Vec<PathBuf> = fs::read_dir(out_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.as_bytes();
            name.windows(suffix.len()).any(|window| window == suffix)
                && !name.windows(MEMO.len()).any(|window| window == MEMO.as_bytes())
                && entry.file_type().is_ok_and(|kind| kind.is_file())
        })
        .map(|entry| entry.path())
        .collect();
    outputs.sort();
    outputs
}

fn dependencies(depinfo: &[u8]) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let (mut paths, mut names) = (Vec::new(), Vec::new());
    for line in depinfo.split(|byte| *byte == b'\n') {
        if let Some(name) = line.strip_prefix(b"# env-dep:") {
            let end = name.iter().position(|byte| *byte == b'=').unwrap_or(name.len());
            names.push(name[..end].to_vec());
        } else if !line.starts_with(b"#") && line.ends_with(b":") && !line.ends_with(b"\\:") {
            paths.push(relocate(&line[..line.len() - 1], b"\\ ", b" "));
        }
    }
    paths.sort();
    paths.dedup();
    names.sort();
    names.dedup();
    (paths, names)
}

fn recorded_environment(names: &[Vec<u8>], environment: &Environment) -> Option<Vec<u8>> {
    let mut recorded = Vec::new();
    for name in names {
        recorded.extend_from_slice(name);
        if let Some(value) = environment.get(OsStr::from_bytes(name)) {
            if value.as_bytes().contains(&b'\n') {
                return None;
            }
            recorded.push(b'=');
            recorded.extend_from_slice(value.as_bytes());
        }
        recorded.push(b'\n');
    }
    Some(recorded)
}

fn memo_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(MEMO);
    PathBuf::from(name)
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|metadata| metadata.modified()).ok()
}

fn unchanged_since(path: &Path, started: SystemTime) -> Option<()> {
    (modified(path)? < started).then_some(())
}

fn fresh_memo(path: &Path) -> Option<String> {
    let memo = memo_path(path);
    let (file, memo_time) = (modified(path)?, modified(&memo)?);
    let text = fs::read_to_string(&memo).ok()?;
    let line = text.lines().next()?;
    (file <= memo_time && line.len() == 64).then(|| line.to_owned())
}

fn identity(path: &Path) -> Option<String> {
    if let Some(identity) = fresh_memo(path) {
        return Some(identity);
    }
    let digest = digest_file(path).ok()?;
    write_memo(path, &digest);
    Some(digest)
}

fn write_memo(path: &Path, digest: &str) {
    let memo = memo_path(path);
    let mut staging = memo.clone().into_os_string();
    staging.push(format!(".{}", std::process::id()));
    if fs::write(&staging, format!("{digest}\n")).is_ok() && fs::rename(&staging, &memo).is_err() {
        let _ = fs::remove_file(&staging);
    }
}

fn digest_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hasher.finish());
        }
        hasher.update(&buffer[..read]);
    }
}

fn tool_digest(cache: &Path, tool: &Path) -> Option<String> {
    let mut name: Vec<u8> = tool
        .as_os_str()
        .as_bytes()
        .iter()
        .map(|byte| if matches!(byte, b'/' | b' ') { b'_' } else { *byte })
        .collect();
    name.extend_from_slice(b".sha256");
    let memo = cache.join("rustc").join(OsStr::from_bytes(&name));
    if let (Some(binary), Some(memo_time)) = (modified(tool), modified(&memo))
        && binary <= memo_time
        && let Ok(text) = fs::read_to_string(&memo)
        && let Some(line) = text.lines().next()
        && line.len() == 64
    {
        return Some(line.to_owned());
    }
    let digest = digest_file(tool).ok()?;
    let _ = fs::create_dir_all(cache.join("rustc"));
    let mut staging = memo.clone().into_os_string();
    staging.push(format!(".{}", std::process::id()));
    if fs::write(&staging, format!("{digest}\n")).is_ok() {
        let _ = fs::rename(&staging, &memo);
    }
    Some(digest)
}

fn rustc_version(cache: &Path, rustc: &Path) -> Option<String> {
    let path = rustc.as_os_str().as_bytes();
    let memo = path
        .windows(12)
        .any(|window| window == b"/toolchains/")
        .then(|| {
            let name: Vec<u8> = path
                .iter()
                .map(|byte| if matches!(byte, b'/' | b' ') { b'_' } else { *byte })
                .collect();
            cache.join("rustc").join(OsStr::from_bytes(&name))
        });
    if let Some(memo) = &memo
        && let (Some(binary), Some(memo_time)) = (modified(rustc), modified(memo))
        && binary <= memo_time
        && let Ok(text) = fs::read_to_string(memo)
        && let Some(line) = text.lines().next()
    {
        return Some(line.to_owned());
    }
    let output = Command::new(rustc).arg("-vV").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(&output.stdout);
    let version = hasher.finish();
    if let Some(memo) = memo {
        let _ = fs::create_dir_all(cache.join("rustc"));
        let mut staging = memo.clone().into_os_string();
        staging.push(format!(".{}", std::process::id()));
        if fs::write(&staging, format!("{version}\n")).is_ok() {
            let _ = fs::rename(&staging, &memo);
        }
    }
    Some(version)
}

fn stable(cache: &Path, directory: &Path) -> Option<PathBuf> {
    let mut entries = Vec::new();
    walk(directory, Path::new(""), &mut entries).ok()?;
    let renames = object_renames(directory, &entries);
    let mut canonical = Vec::with_capacity(entries.len());
    for (relative, is_file) in entries {
        let name = renames.iter().find(|(from, _)| *from == relative).map_or_else(|| relative.clone(), |(_, to)| to.clone());
        let content = if is_file && !renames.is_empty() && relative.extension() == Some(OsStr::new("a")) {
            Some(rename_members(&fs::read(directory.join(&relative)).ok()?, &renames)?)
        } else {
            None
        };
        canonical.push((name, relative, is_file, content));
    }
    canonical.sort_by(|left, right| left.0.cmp(&right.0));
    if canonical.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return None;
    }
    let mut hasher = Sha256::new();
    for (name, relative, is_file, content) in &canonical {
        hasher.update(name.as_os_str().as_bytes());
        if *is_file {
            let digest = match content {
                Some(content) => {
                    let mut digest = Sha256::new();
                    digest.update(content);
                    digest.finish()
                }
                None => digest_file(&directory.join(relative)).ok()?,
            };
            hasher.update(b" ");
            hasher.update(digest.as_bytes());
        }
        hasher.update(b"\n");
    }
    let stable = cache.join("out").join(hasher.finish());
    if !stable.exists() {
        fs::create_dir_all(cache.join("out")).ok()?;
        let staging = cache.join("out").join(format!(".data.{}.{}", std::process::id(), unique()));
        let copied = (|| -> io::Result<()> {
            fs::create_dir(&staging)?;
            for (name, relative, is_file, content) in &canonical {
                match (is_file, content) {
                    (false, _) => fs::create_dir_all(staging.join(name))?,
                    (true, Some(content)) => fs::write(staging.join(name), content)?,
                    (true, None) => clone(&directory.join(relative), &staging.join(name))?,
                }
            }
            Ok(())
        })();
        if copied.is_err() || symlink(&staging, &stable).is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
    }
    stable.exists().then_some(stable)
}

fn object_renames(directory: &Path, entries: &[(PathBuf, bool)]) -> Vec<(PathBuf, PathBuf)> {
    const EXTENSIONS: [&str; 10] = ["c", "cc", "cpp", "cxx", "C", "s", "S", "asm", "m", "mm"];
    let prefix_of = |dirname: &str, extension: &str| {
        let mut hasher = std::hash::DefaultHasher::new();
        hasher.write(dirname.as_bytes());
        hasher.write(extension.as_bytes());
        format!("{:016x}", hasher.finish())
    };
    let mut renames = Vec::new();
    for (relative, is_file) in entries {
        let Some(name) = relative.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        let (prefix, rest) = name.split_at(name.len().min(16));
        if !is_file || !name.ends_with(".o") || !rest.starts_with('-') || !prefix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        let parent = relative.parent().unwrap_or(Path::new(""));
        let absolute = if parent.as_os_str().is_empty() { directory.to_path_buf() } else { directory.join(parent) };
        let (Some(absolute), Some(local)) = (absolute.to_str(), parent.to_str()) else {
            continue;
        };
        if let Some(extension) = EXTENSIONS.iter().find(|extension| prefix_of(absolute, extension) == prefix) {
            renames.push((relative.clone(), parent.join(format!("{}{rest}", prefix_of(local, extension)))));
        }
    }
    renames
}

fn rename_members(archive: &[u8], renames: &[(PathBuf, PathBuf)]) -> Option<Vec<u8>> {
    let mut archive = archive.to_vec();
    if !archive.starts_with(b"!<arch>\n") {
        return None;
    }
    let mut offset = 8;
    while offset + 60 <= archive.len() {
        let header = &archive[offset..offset + 60];
        let size: usize = std::str::from_utf8(&header[48..58]).ok()?.trim().parse().ok()?;
        let data = offset + 60;
        if data + size > archive.len() {
            return None;
        }
        if let Some(length) = header.strip_prefix(b"#1/") {
            let length: usize = std::str::from_utf8(&length[..13]).ok()?.trim().parse().ok()?;
            let stored = &archive[data..data + length.min(size)];
            let name = stored[..stored.iter().position(|byte| *byte == 0).unwrap_or(stored.len())].to_vec();
            if let Some((_, to)) = renames.iter().find(|(from, _)| from.file_name().map(OsStrExt::as_bytes) == Some(name.as_slice())) {
                let to = to.file_name()?.as_bytes();
                if to.len() != name.len() {
                    return None;
                }
                archive[data..data + name.len()].copy_from_slice(to);
            }
        } else if header.starts_with(b"//") {
            for (from, to) in renames {
                let (from, to) = (from.file_name()?.as_bytes(), to.file_name()?.as_bytes());
                let region = relocate(&archive[data..data + size], from, to);
                archive[data..data + size].copy_from_slice(&region);
            }
        }
        offset = data + size + size % 2;
    }
    Some(archive)
}

fn unique() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

fn walk(root: &Path, relative: &Path, entries: &mut Vec<(PathBuf, bool)>) -> io::Result<()> {
    for entry in fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let path = relative.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            entries.push((path.clone(), false));
            walk(root, &path, entries)?;
        } else {
            entries.push((path, true));
        }
    }
    Ok(())
}

fn clone(from: &Path, to: &Path) -> io::Result<()> {
    if fs::symlink_metadata(to).is_ok() {
        fs::remove_file(to)?;
    }
    fs::copy(from, to)?;
    Ok(())
}

fn touch(path: &Path) -> io::Result<()> {
    fs::File::options().write(true).open(path)?.set_modified(SystemTime::now())
}

fn relocate(text: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from.is_empty() {
        return text.to_vec();
    }
    let mut result = Vec::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        if text[index..].starts_with(from) {
            result.extend_from_slice(to);
            index += from.len();
        } else {
            result.push(text[index]);
            index += 1;
        }
    }
    result
}

fn same_environment(entry: &Path, environment: &Environment) -> bool {
    let Ok(recorded) = fs::read(entry.join("env")) else {
        return true;
    };
    recorded.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()).all(|line| {
        match line.iter().position(|byte| *byte == b'=') {
            Some(at) => environment
                .get(OsStr::from_bytes(&line[..at]))
                .is_some_and(|value| value.as_bytes() == &line[at + 1..]),
            None => !environment.contains_key(OsStr::from_bytes(line)),
        }
    })
}

fn sources_match(sources: &[u8]) -> bool {
    sources.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()).all(|line| {
        line.iter().position(|byte| *byte == b' ').is_some_and(|at| {
            digest_file(Path::new(OsStr::from_bytes(&line[at + 1..])))
                .is_ok_and(|digest| digest.as_bytes() == &line[..at])
        })
    })
}

fn restore(
    entry: &Path,
    out_dir: &Path,
    environment: &Environment,
    kind: Kind,
    inputs: &[u8],
) -> io::Result<()> {
    let cached_out = fs::read(entry.join("out-dir"))?;
    let cached_out = cached_out.strip_suffix(b"\n").unwrap_or(&cached_out).to_vec();
    if kind == Kind::Workspace
        && (fs::read(entry.join("inputs"))? != inputs || !sources_match(&fs::read(entry.join("sources"))?))
    {
        return Err(io::Error::other("inputs changed"));
    }
    let identities = fs::read(entry.join("identities"))?;
    if !same_environment(entry, environment) {
        return Err(io::Error::other("environment changed"));
    }
    let current = out_dir.as_os_str().as_bytes();
    let mut files: Vec<_> = fs::read_dir(entry.join("files"))?.collect::<io::Result<_>>()?;
    files.sort_by_key(fs::DirEntry::file_name);
    for file in &files {
        let name = file.file_name();
        let destination = out_dir.join(&name);
        if name.as_bytes().ends_with(b".d") {
            fs::write(&destination, relocate(&fs::read(file.path())?, &cached_out, current))?;
        } else {
            clone(&file.path(), &destination)?;
            touch(&destination)?;
        }
    }
    for line in identities.split(|byte| *byte == b'\n') {
        if let Some(at) = line.iter().position(|byte| *byte == b' ') {
            write_memo(&out_dir.join(OsStr::from_bytes(&line[..at])), &String::from_utf8_lossy(&line[at + 1..]));
        }
    }
    let stderr = fs::read(entry.join("stderr"))?;
    let mut output = io::stderr();
    output.write_all(&relocate(&stderr, &cached_out, current))?;
    output.flush()
}

fn stage(cache: &Path, directory: &str, unit: &Unit<'_>) -> io::Result<PathBuf> {
    fs::create_dir_all(cache.join(directory))?;
    let staging = cache.join(directory).join(format!(".data.{}.{}", std::process::id(), unique()));
    let result = (|| -> io::Result<()> {
        fs::create_dir_all(staging.join("files"))?;
        for output in unit.outputs {
            if let Some(name) = output.file_name() {
                clone(output, &staging.join("files").join(name))?;
            }
        }
        fs::write(staging.join("env"), unit.recorded)?;
        fs::write(staging.join("stderr"), unit.captured)?;
        fs::write(staging.join("inputs"), unit.inputs)?;
        fs::write(staging.join("sources"), unit.sources)?;
        fs::write(staging.join("identities"), unit.identities)?;
        let mut out = unit.out_dir.as_os_str().as_bytes().to_vec();
        out.push(b'\n');
        fs::write(staging.join("out-dir"), out)
    })();
    match result {
        Ok(()) => Ok(staging),
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            Err(error)
        }
    }
}

fn store(cache: &Path, entry: &Path, unit: &Unit<'_>) -> io::Result<()> {
    let staging = stage(cache, "units", unit)?;
    symlink(&staging, entry).inspect_err(|_| {
        let _ = fs::remove_dir_all(&staging);
    })
}

fn store_workspace(cache: &Path, entry: &Path, unit: &Unit<'_>) -> io::Result<()> {
    let (Some(directory), Some(slot)) = (entry.parent(), entry.file_name()) else {
        return Ok(());
    };
    let mut prefix = slot.as_bytes().to_vec();
    prefix.push(b'.');
    let mut version = prefix.clone();
    version.extend_from_slice(unit.identity.as_bytes());
    let version = directory.join(OsStr::from_bytes(&version));
    if !version.exists() {
        let staging = stage(cache, "workspace", unit)?;
        if fs::rename(&staging, &version).is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
    }
    let link = directory.join(format!(".link.{}.{}", std::process::id(), unique()));
    symlink(&version, &link)?;
    if let Err(error) = fs::rename(&link, entry) {
        let _ = fs::remove_file(&link);
        return Err(error);
    }
    let current = fs::read_link(entry).ok();
    for sibling in fs::read_dir(directory)?.flatten() {
        let path = sibling.path();
        if sibling.file_name().as_bytes().starts_with(&prefix) && current.as_deref() != Some(path.as_path()) {
            let _ = fs::remove_dir_all(&path);
        }
    }
    Ok(())
}

fn c(cache: &Path, args: &[OsString], environment: Environment) -> ExitCode {
    let (mut object, mut source, mut compile) = (None, None, false);
    for index in 1..args.len() {
        let arg = args[index].as_bytes();
        match args[index - 1].as_bytes() {
            b"-o" => object = Some(index),
            b"-c" => source = Some(index),
            _ => {}
        }
        if arg == b"-E" || arg.starts_with(b"-M") || (arg.starts_with(b"-o") && arg.len() > 2)
            || arg.starts_with(b"@") || arg == b"-"
        {
            return exec(args, &environment);
        }
        if arg == b"-c" {
            compile = true;
        }
    }
    let (Some(object), Some(source), true) = (object, source, compile) else {
        return exec(args, &environment);
    };
    if args[source].as_bytes().starts_with(b"-") {
        return exec(args, &environment);
    }
    let out_dir = environment.get(OsStr::new("OUT_DIR")).cloned().unwrap_or_default();
    let out_dir = out_dir.as_bytes().to_vec();
    let Ok(source_text) = fs::read(&args[source]) else {
        return exec(args, &environment);
    };
    if volatile(&source_text) {
        return exec(args, &environment);
    }
    let Ok(version) = Command::new(&args[0]).arg("--version").output() else {
        return exec(args, &environment);
    };
    let mut hasher = Sha256::new();
    hasher.update(C_KEY.as_bytes());
    hasher.update(b"\n");
    hasher.update(&version.stdout);
    hasher.update(&version.stderr);
    hasher.update(env::current_dir().unwrap_or_default().as_os_str().as_bytes());
    hasher.update(b"\n");
    for (index, arg) in args.iter().enumerate() {
        if index == object || index + 1 == object {
            continue;
        }
        hasher.update(&relocate(arg.as_bytes(), &out_dir, b"<out>"));
        hasher.update(b"\n");
    }
    for name in ["MACOSX_DEPLOYMENT_TARGET", "SDKROOT", "CPATH", "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH", "OBJC_INCLUDE_PATH"] {
        if let Some(value) = environment.get(OsStr::new(name)) {
            hasher.update(name.as_bytes());
            hasher.update(b"=");
            hasher.update(value.as_bytes());
            hasher.update(b"\n");
        }
    }
    let mut source_hasher = Sha256::new();
    source_hasher.update(&source_text);
    hasher.update(source_hasher.finish().as_bytes());
    let entry = cache.join("c").join(hasher.finish());
    let object = PathBuf::from(&args[object]);
    if headers_match(&entry, &out_dir)
        && clone(&entry.join("object"), &object).is_ok()
        && touch(&object).is_ok()
    {
        if let Ok(errors) = fs::read(entry.join("stderr")) {
            let _ = io::stderr().write_all(&errors);
        }
        return ExitCode::SUCCESS;
    }
    let mut depfile = object.clone().into_os_string();
    depfile.push(format!(".{}.buildcache.d", std::process::id()));
    let started = SystemTime::now();
    let mut map = OsString::from("-ffile-prefix-map=");
    map.push(OsStr::from_bytes(&out_dir));
    map.push("=.");
    let output = match Command::new(&args[0])
        .args(&args[1..])
        .arg(&map)
        .arg("-MD")
        .arg("-MF")
        .arg(&depfile)
        .env_clear()
        .envs(&environment)
        .stderr(Stdio::piped())
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            eprintln!("buildcache: could not run {}: {error}", args[0].to_string_lossy());
            return ExitCode::from(1);
        }
    };
    let _ = io::stdout().write_all(&output.stdout);
    let _ = io::stderr().write_all(&output.stderr);
    let dependencies = fs::read(&depfile);
    let _ = fs::remove_file(&depfile);
    if output.status.success()
        && let Ok(dependencies) = dependencies
        && dependency_paths(&dependencies)
            .iter()
            .all(|path| unchanged_since(Path::new(OsStr::from_bytes(path)), started).is_some())
        && let Some(manifest) = manifest(&dependencies, &out_dir)
    {
        let _ = store_object(cache, &entry, &object, &manifest, &output.stderr);
    }
    exit_code(output.status)
}

fn volatile(text: &[u8]) -> bool {
    [b"__DATE__" as &[u8], b"__TIME__", b"__TIMESTAMP__"]
        .iter()
        .any(|token| text.windows(token.len()).any(|window| window == *token))
}

fn dependency_paths(depfile: &[u8]) -> Vec<Vec<u8>> {
    let Some(colon) = depfile.windows(2).position(|window| window == b": ") else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    let mut current = Vec::new();
    let mut bytes = depfile[colon + 2..].iter().peekable();
    while let Some(&byte) = bytes.next() {
        match byte {
            b'\\' if bytes.peek() == Some(&&b' ') => {
                current.push(b' ');
                bytes.next();
            }
            b'\\' if bytes.peek() == Some(&&b'\n') => {
                bytes.next();
            }
            b' ' | b'\n' | b'\t' | b'\r' => {
                if !current.is_empty() {
                    paths.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(byte),
        }
    }
    if !current.is_empty() {
        paths.push(current);
    }
    paths
}

fn manifest(depfile: &[u8], out_dir: &[u8]) -> Option<Vec<u8>> {
    let mut manifest = Vec::new();
    for path in dependency_paths(depfile).into_iter().skip(1) {
        let contents = fs::read(OsStr::from_bytes(&path)).ok()?;
        if volatile(&contents) {
            return None;
        }
        let mut hasher = Sha256::new();
        hasher.update(&contents);
        manifest.extend_from_slice(hasher.finish().as_bytes());
        manifest.push(b' ');
        manifest.extend_from_slice(&relocate(&path, out_dir, b"<out>"));
        manifest.push(b'\n');
    }
    Some(manifest)
}

fn headers_match(entry: &Path, out_dir: &[u8]) -> bool {
    let Ok(manifest) = fs::read(entry.join("headers")) else {
        return false;
    };
    manifest.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()).all(|line| {
        let Some(at) = line.iter().position(|byte| *byte == b' ') else {
            return false;
        };
        let path = relocate(&line[at + 1..], b"<out>", out_dir);
        digest_file(Path::new(OsStr::from_bytes(&path))).is_ok_and(|digest| digest.as_bytes() == &line[..at])
    })
}

fn store_object(cache: &Path, entry: &Path, object: &Path, manifest: &[u8], errors: &[u8]) -> io::Result<()> {
    fs::create_dir_all(cache.join("c"))?;
    let staging = cache.join("c").join(format!(".data.{}.{}", std::process::id(), unique()));
    let result = (|| -> io::Result<()> {
        fs::create_dir(&staging)?;
        clone(object, &staging.join("object"))?;
        fs::write(staging.join("headers"), manifest)?;
        fs::write(staging.join("stderr"), errors)?;
        if fs::symlink_metadata(entry).is_ok() {
            fs::remove_file(entry)?;
        }
        symlink(&staging, entry)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    filled: usize,
    length: u64,
}

const ROUND: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

impl Sha256 {
    const fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: [0; 64],
            filled: 0,
            length: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.length += data.len() as u64;
        if self.filled > 0 {
            let take = (64 - self.filled).min(data.len());
            self.buffer[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled < 64 {
                return;
            }
            let block = self.buffer;
            self.compress(&block);
            self.filled = 0;
        }
        let whole = data.len() - data.len() % 64;
        self.blocks(&data[..whole]);
        let rest = &data[whole..];
        self.buffer[..rest.len()].copy_from_slice(rest);
        self.filled = rest.len();
    }

    fn blocks(&mut self, data: &[u8]) {
        #[cfg(target_arch = "aarch64")]
        if std::arch::is_aarch64_feature_detected!("sha2") {
            unsafe { compress_sha2(&mut self.state, data) };
            return;
        }
        for block in data.chunks_exact(64) {
            self.compress(block.try_into().expect("64-byte block"));
        }
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut words = [0u32; 64];
        for (index, word) in words.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes(block[index * 4..index * 4 + 4].try_into().expect("word"));
        }
        for index in 16..64 {
            let small0 = words[index - 15].rotate_right(7) ^ words[index - 15].rotate_right(18) ^ (words[index - 15] >> 3);
            let small1 = words[index - 2].rotate_right(17) ^ words[index - 2].rotate_right(19) ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(small0)
                .wrapping_add(words[index - 7])
                .wrapping_add(small1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for index in 0..64 {
            let big1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ (!e & g);
            let first = h
                .wrapping_add(big1)
                .wrapping_add(choose)
                .wrapping_add(ROUND[index])
                .wrapping_add(words[index]);
            let big0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let second = big0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(first);
            d = c;
            c = b;
            b = a;
            a = first.wrapping_add(second);
        }
        for (state, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *state = state.wrapping_add(value);
        }
    }

    fn finish(mut self) -> String {
        let bits = self.length.wrapping_mul(8);
        let mut padding = vec![0x80u8];
        let used = (self.length % 64) as usize;
        let zeros = if used < 56 { 55 - used } else { 119 - used };
        padding.extend(std::iter::repeat_n(0, zeros));
        padding.extend_from_slice(&bits.to_be_bytes());
        let length = self.length;
        self.update(&padding);
        self.length = length;
        self.state.iter().map(|word| format!("{word:08x}")).collect()
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "sha2")]
unsafe fn compress_sha2(state: &mut [u32; 8], data: &[u8]) {
    use std::arch::aarch64::{
        vaddq_u32, vld1q_u32, vld1q_u8, vreinterpretq_u32_u8, vrev32q_u8, vsha256h2q_u32, vsha256hq_u32,
        vsha256su0q_u32, vsha256su1q_u32, vst1q_u32,
    };
    unsafe {
        let mut abcd = vld1q_u32(state.as_ptr());
        let mut efgh = vld1q_u32(state.as_ptr().add(4));
        for block in data.chunks_exact(64) {
            let (saved_abcd, saved_efgh) = (abcd, efgh);
            let mut schedule = [
                vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(block.as_ptr()))),
                vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(block.as_ptr().add(16)))),
                vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(block.as_ptr().add(32)))),
                vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(block.as_ptr().add(48)))),
            ];
            for group in 0..16 {
                let words = vaddq_u32(schedule[0], vld1q_u32(ROUND.as_ptr().add(group * 4)));
                let previous = abcd;
                abcd = vsha256hq_u32(abcd, efgh, words);
                efgh = vsha256h2q_u32(efgh, previous, words);
                let next = vsha256su1q_u32(vsha256su0q_u32(schedule[0], schedule[1]), schedule[2], schedule[3]);
                schedule = [schedule[1], schedule[2], schedule[3], next];
            }
            abcd = vaddq_u32(abcd, saved_abcd);
            efgh = vaddq_u32(efgh, saved_efgh);
        }
        vst1q_u32(state.as_mut_ptr(), abcd);
        vst1q_u32(state.as_mut_ptr().add(4), efgh);
    }
}
