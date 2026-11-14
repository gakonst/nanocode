use std::{cell::RefCell, io, io::Read, rc::Rc, time::Duration};

use psh_engine::{
    host::{FileKind, OpenOptions},
    util::{
        BlockingOptions, INLINE_INPUT_LIMIT, INLINE_OUTPUT_LIMIT, LocalBoxFuture, Mode, SyncCtx,
        SyncFiles, UtilCtx, UtilEntry,
    },
};

use crate::{Ended, Files, Kind, Limit, Limits, Python, Ran, Remove, Run, Stat, Write};

pub const UTILITY: UtilEntry = UtilEntry {
    names: &["python3", "python"],
    mode: Mode::SandboxOnly,
    destructive: false,
    run: utility,
};

fn utility<'a>(ctx: &'a mut UtilCtx<'_>, argv: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
    Box::pin(ctx.run_blocking(BlockingOptions { stdin: true }, argv.to_vec(), body))
}

struct Shell(SyncFiles);

impl Files for Shell {
    fn stat(&self, path: &[u8]) -> io::Result<Stat> {
        let meta = self.0.stat(path, true)?;
        Ok(Stat {
            kind: match meta.kind {
                FileKind::Dir => Kind::Dir,
                FileKind::Symlink => Kind::Symlink,
                _ => Kind::File,
            },
            size: meta.size,
            mode: meta.mode,
            mtime_ns: meta.mtime_ns,
        })
    }

    fn read(&self, path: &[u8], limit: u64) -> io::Result<Vec<u8>> {
        self.0.read(path, limit)
    }

    fn write(&self, path: &[u8], data: &[u8], how: Write) -> io::Result<()> {
        let options = OpenOptions {
            write: true,
            create: true,
            truncate: how == Write::Replace,
            append: how == Write::Append,
            create_new: how == Write::New,
            ..OpenOptions::default()
        };
        self.0.write(path, data, options)
    }

    fn list(&self, path: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        let entries = self.0.read_dir(path)?;
        Ok(entries.into_iter().map(|entry| entry.name).collect())
    }

    fn mkdir(&self, path: &[u8], mode: u32) -> io::Result<()> {
        self.0.mkdir(path, mode)
    }

    fn remove(&self, path: &[u8], what: Remove) -> io::Result<()> {
        match what {
            Remove::File => self.0.remove_file(path),
            Remove::Dir => self.0.remove_dir(path),
        }
    }

    fn rename(&self, from: &[u8], to: &[u8]) -> io::Result<()> {
        self.0.rename(from, to)
    }
}

fn kept(run: Run, files: SyncFiles) -> io::Result<Ran> {
    thread_local! {
        static PYTHON: RefCell<Option<Python>> = const { RefCell::new(None) };
    }
    PYTHON.with_borrow_mut(|kept| {
        let python = match kept {
            Some(python) => python,
            None => kept.insert(Python::new()?),
        };
        Ok(python.run(run, Rc::new(Shell(files))))
    })
}

#[cfg(target_family = "wasm")]
fn execute(run: Run, files: SyncFiles) -> io::Result<Ran> {
    kept(run, files)
}

#[cfg(not(target_family = "wasm"))]
fn execute(run: Run, files: SyncFiles) -> io::Result<Ran> {
    use nanocode_sync::Slot;
    use psh_engine::host::NativePathFn;
    use std::sync::{LazyLock, mpsc};

    struct Job {
        run: Run,
        native: NativePathFn,
        reply: mpsc::Sender<io::Result<Ran>>,
    }

    static IDLE: LazyLock<Slot<mpsc::Sender<Job>>> = LazyLock::new(Slot::default);

    let Some(native) = files.native() else {
        return kept(run, files);
    };
    let jobs = match IDLE.take() {
        Some(jobs) => jobs,
        None => {
            let (jobs, queue) = mpsc::channel::<Job>();
            let owner = move || {
                for job in queue {
                    let _ = job.reply.send(kept(job.run, SyncFiles::on(job.native)));
                }
            };
            std::thread::Builder::new()
                .name("python3".to_owned())
                .spawn(owner)?;
            jobs
        }
    };
    let ended = || io::Error::other("the thread that keeps the interpreter ended");
    let (reply, ran) = mpsc::channel();
    let job = Job { run, native, reply };
    jobs.send(job).map_err(|_| ended())?;
    let ran = ran.recv().map_err(|_| ended())?;
    drop(IDLE.put(jobs));
    ran
}

const fn limits() -> Limits {
    Limits {
        output: INLINE_OUTPUT_LIMIT as u64,
        file: INLINE_INPUT_LIMIT as u64,
        sleep: Duration::MAX,
        steps: u64::MAX,
        time: Duration::MAX,
        memory: None,
        depth: usize::MAX,
    }
}

fn ending(ended: Ended, name: &str, say: &mut dyn FnMut(&[u8])) -> i32 {
    match ended {
        Ended::Exited(code) => code,
        Ended::Needs(missing) => {
            for missing in missing {
                say(format!("{missing}\n").as_bytes());
            }
            127
        }
        Ended::Refused(limit) => {
            let why = match limit {
                Limit::Output => "it printed more than this shell keeps",
                Limit::Sleep => "it cannot wait in this shell",
                Limit::Steps => "it ran more steps than this shell allows",
                Limit::Time => "it ran longer than this shell allows",
                Limit::Memory => "it held more memory than this shell allows",
            };
            say(format!("{name}: {why}\n").as_bytes());
            126
        }
        Ended::Cancelled => 128 + psh_engine::host::SIGTERM,
    }
}

/// `python3` as a program of its own (nanocode-python): the arguments after the
/// program's name, stdin, the working directory and the environment are the process's,
/// files are the machine's, and the exit status is the one the utility returns.
#[cfg(unix)]
#[must_use]
pub fn serve() -> i32 {
    use std::{
        ffi::{OsStr, OsString},
        io::Write as _,
        os::unix::ffi::{OsStrExt, OsStringExt},
        path::PathBuf,
        sync::Arc,
    };

    let mut args = std::env::args_os();
    let name = args.next().map_or_else(
        || "python3".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let mut stderr = io::stderr().lock();
    let Ok(argv) = args
        .map(OsString::into_string)
        .collect::<Result<Vec<_>, _>>()
    else {
        let _ = writeln!(stderr, "{name}: an argument is not UTF-8");
        return 2;
    };
    let mut stdin = Vec::new();
    if let Err(error) = io::stdin().read_to_end(&mut stdin) {
        let _ = writeln!(stderr, "{name}: read error: {error}");
        return 1;
    }
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd.into_os_string().into_vec(),
        Err(error) => {
            let _ = writeln!(stderr, "{name}: cannot start: {error}");
            return 1;
        }
    };
    let run = Run {
        argv,
        stdin,
        cwd,
        env: std::env::vars_os()
            .map(|(key, value)| (key.into_vec(), value.into_vec()))
            .collect(),
        limits: limits(),
        stop: Arc::new(|| false),
    };
    let python = match Python::new() {
        Ok(python) => python,
        Err(error) => {
            let _ = writeln!(stderr, "{name}: cannot start: {error}");
            return 1;
        }
    };
    let machine = SyncFiles::on(|path| Ok(PathBuf::from(OsStr::from_bytes(path))));
    let ran = python.run(run, Rc::new(Shell(machine)));
    std::mem::forget(python);
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(&ran.stdout).and_then(|()| stdout.flush());
    let _ = stderr.write_all(&ran.stderr);
    ending(ran.ended, &name, &mut |text| drop(stderr.write_all(text)))
}

fn body(ctx: &mut SyncCtx, argv: Vec<Vec<u8>>) -> i32 {
    ctx.no_replay();
    let name = argv.first().map_or_else(
        || "python3".to_owned(),
        |name| String::from_utf8_lossy(name).into_owned(),
    );
    let argv: Result<Vec<String>, _> = argv.into_iter().skip(1).map(String::from_utf8).collect();
    let Ok(argv) = argv else {
        ctx.error("an argument is not UTF-8");
        return 2;
    };
    let mut stdin = Vec::new();
    if ctx.stdin_readable()
        && let Err(error) = ctx.stdin().read_to_end(&mut stdin)
    {
        ctx.error(&format!("read error: {error}"));
        return 1;
    }
    let run = Run {
        argv,
        stdin,
        cwd: ctx.cwd().to_vec(),
        env: ctx.env().to_vec(),
        limits: limits(),
        stop: ctx.canceller(),
    };
    let ran = match execute(run, ctx.files()) {
        Ok(ran) => ran,
        Err(error) => {
            ctx.error(&format!("cannot start: {error}"));
            return 1;
        }
    };
    let _ = ctx.write(&ran.stdout);
    ctx.error_raw(&ran.stderr);
    ending(ran.ended, &name, &mut |text| ctx.error_raw(text))
}
