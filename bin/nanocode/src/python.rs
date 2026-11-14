use std::{
    ffi::OsStr,
    fs,
    io::{Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::PermissionsExt,
        process::{CommandExt, ExitStatusExt},
    },
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use nanocode_compression::xz::{Action, Status, Stream};
use psh_engine::util::{BlockingOptions, LocalBoxFuture, Mode, SyncCtx, UtilCtx, UtilEntry};

/// `python3` and `python`, run by nanocode-python, a program of its own that is
/// downloaded the first time a command needs it.
pub const UTILITY: UtilEntry = UtilEntry {
    names: &["python3", "python"],
    mode: Mode::SandboxOnly,
    destructive: false,
    run: utility,
};

const PROGRAM: &str = "nanocode-python";
const DIGEST: Option<&str> = option_env!("NANOCODE_PYTHON_SHA256");
const RELEASE: &str = "https://github.com/nanocodexyz/nanocode/releases/download/nanocode-cli";
const RELEASE_VAR: &str = "NANOCODE_RELEASE";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const CANCEL_POLL: Duration = Duration::from_millis(20);
const UNPACK_CHUNK: usize = 1 << 20;
const EXECUTABLE: u32 = 0o755;

fn utility<'a>(ctx: &'a mut UtilCtx<'_>, argv: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
    Box::pin(ctx.run_blocking(BlockingOptions { stdin: true }, argv.to_vec(), body))
}

fn body(ctx: &mut SyncCtx, argv: Vec<Vec<u8>>) -> i32 {
    ctx.no_replay();
    let Some(native) = ctx.files().native() else {
        ctx.error("needs the machine's own files");
        return 1;
    };
    let program = match program() {
        Ok(program) => program,
        Err(error) => {
            ctx.error(&error);
            return 1;
        }
    };
    let mut stdin = Vec::new();
    if ctx.stdin_readable()
        && let Err(error) = ctx.stdin().read_to_end(&mut stdin)
    {
        ctx.error(&format!("read error: {error}"));
        return 1;
    }
    let cwd = match native(ctx.cwd()) {
        Ok(cwd) => cwd,
        Err(error) => {
            ctx.error(&format!("cannot start: {error}"));
            return 1;
        }
    };
    let mut command = Command::new(&program);
    if let Some((name, arguments)) = argv.split_first() {
        command
            .arg0(OsStr::from_bytes(name))
            .args(arguments.iter().map(|argument| OsStr::from_bytes(argument)));
    }
    command
        .env_clear()
        .envs(
            ctx.env()
                .iter()
                .map(|(key, value)| (OsStr::from_bytes(key), OsStr::from_bytes(value))),
        )
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            ctx.error(&format!("cannot start {}: {error}", program.display()));
            return 1;
        }
    };
    if let Some(mut pipe) = child.stdin.take() {
        thread::spawn(move || pipe.write_all(&stdin));
    }
    let (done, finished) = mpsc::channel();
    let reader = |pipe: Option<Box<dyn Read + Send>>, done: Option<mpsc::Sender<()>>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                drop(pipe.read_to_end(&mut bytes));
            }
            if let Some(done) = done {
                let _ = done.send(());
            }
            bytes
        })
    };
    let stdout = reader(
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        Some(done),
    );
    let stderr = reader(
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        None,
    );
    let stop = ctx.canceller();
    while let Err(mpsc::RecvTimeoutError::Timeout) = finished.recv_timeout(CANCEL_POLL) {
        if stop() {
            drop(child.kill());
            drop(child.wait());
            return 128 + psh_engine::host::SIGTERM;
        }
    }
    let status = child.wait();
    let _ = ctx.write(&stdout.join().unwrap_or_default());
    ctx.error_raw(&stderr.join().unwrap_or_default());
    match status {
        Ok(status) => status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or_default()),
        Err(error) => {
            ctx.error(&format!("cannot wait for {}: {error}", program.display()));
            1
        }
    }
}

/// The program: in a release build the one whose SHA-256 the build recorded, kept in
/// nanocode's home and downloaded from the release the first time; in any other build
/// the one built beside this executable.
fn program() -> Result<PathBuf, String> {
    let Some(digest) = DIGEST else {
        let beside = std::env::current_exe()
            .map_err(|error| error.to_string())?
            .with_file_name(PROGRAM);
        return if beside.is_file() {
            Ok(beside)
        } else {
            Err(format!(
                "this build of nanocode has no Python: build {PROGRAM} beside it (cargo build -p nanocode-python-cli)"
            ))
        };
    };
    let home = nanocode_home::AgentHome::from_env().map_err(|error| error.to_string())?;
    let directory = home.nanocode_home().join("bin");
    let path = directory.join(format!("{PROGRAM}-{digest}"));
    if path.is_file() {
        return Ok(path);
    }
    let release = std::env::var(RELEASE_VAR).unwrap_or_else(|_| RELEASE.to_owned());
    let url = format!("{release}/{PROGRAM}-{digest}.xz");
    let bytes = fetch(&url, digest).map_err(|reason| {
        format!(
            "Python is downloaded the first time a command runs it, and getting {url} failed: {reason}; connect to the network and run the command again"
        )
    })?;
    let partial = directory.join(format!(".{PROGRAM}-{digest}.{}", std::process::id()));
    let installed = fs::create_dir_all(&directory)
        .and_then(|()| fs::write(&partial, &bytes))
        .and_then(|()| fs::set_permissions(&partial, fs::Permissions::from_mode(EXECUTABLE)))
        .and_then(|()| fs::rename(&partial, &path));
    if let Err(error) = installed {
        drop(fs::remove_file(&partial));
        return Err(format!(
            "cannot install Python at {}: {error}",
            path.display()
        ));
    }
    Ok(path)
}

fn fetch(url: &str, digest: &str) -> Result<Vec<u8>, String> {
    let response = nanocode_http::blocking::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .and_then(|client| client.get(url).send())
        .and_then(nanocode_http::blocking::Response::error_for_status)
        .and_then(nanocode_http::blocking::Response::bytes)
        .map_err(|error| error.to_string())?;
    let bytes = unpack(&response).map_err(|error| format!("it is not an xz stream ({error:?})"))?;
    let found: String = nanocode_tempo::sha256(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if found == digest {
        Ok(bytes)
    } else {
        Err(format!(
            "its SHA-256 is {found}, not the {digest} this nanocode was built with"
        ))
    }
}

fn unpack(mut packed: &[u8]) -> Result<Vec<u8>, nanocode_compression::xz::Error> {
    let mut stream = Stream::stream_decoder(u64::MAX);
    let mut bytes = Vec::new();
    loop {
        bytes.reserve(UNPACK_CHUNK);
        let before = stream.total_in();
        let status = stream.process_vec(packed, &mut bytes, Action::Finish)?;
        let read = usize::try_from(stream.total_in() - before).unwrap_or(packed.len());
        packed = &packed[read.min(packed.len())..];
        if status == Status::StreamEnd {
            return Ok(bytes);
        }
    }
}
