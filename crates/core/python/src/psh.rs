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
    mode: Mode::PathFallback,
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
    use psh_engine::host::NativePathFn;
    use std::sync::{OnceLock, mpsc};

    struct Job {
        run: Run,
        native: NativePathFn,
        reply: mpsc::Sender<io::Result<Ran>>,
    }

    static OWNER: OnceLock<Option<mpsc::Sender<Job>>> = OnceLock::new();

    let Some(native) = files.native() else {
        return kept(run, files);
    };
    let owner = OWNER.get_or_init(|| {
        let (jobs, queue) = mpsc::channel::<Job>();
        let owner = move || {
            for job in queue {
                let _ = job.reply.send(kept(job.run, SyncFiles::on(job.native)));
            }
        };
        let spawned = std::thread::Builder::new()
            .name("python3".to_owned())
            .spawn(owner);
        spawned.ok().map(|_| jobs)
    });
    let ended = || io::Error::other("the thread that keeps the interpreter ended");
    let jobs = owner.as_ref().ok_or_else(ended)?;
    let (reply, ran) = mpsc::channel();
    let job = Job { run, native, reply };
    jobs.send(job).map_err(|_| ended())?;
    ran.recv().map_err(|_| ended())?
}

fn body(ctx: &mut SyncCtx, argv: Vec<Vec<u8>>) -> i32 {
    ctx.no_replay();
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
        limits: Limits {
            output: INLINE_OUTPUT_LIMIT as u64,
            file: INLINE_INPUT_LIMIT as u64,
            sleep: Duration::MAX,
            steps: u64::MAX,
            time: Duration::MAX,
            memory: None,
            depth: usize::MAX,
        },
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
    match ran.ended {
        Ended::Exited(code) => code,
        Ended::Needs(missing) => {
            for missing in missing {
                ctx.error_raw(format!("{missing}\n").as_bytes());
            }
            127
        }
        Ended::Refused(limit) => {
            ctx.error(match limit {
                Limit::Output => "it printed more than this shell keeps",
                Limit::Sleep => "it cannot wait in this shell",
                Limit::Steps => "it ran more steps than this shell allows",
                Limit::Time => "it ran longer than this shell allows",
                Limit::Memory => "it held more memory than this shell allows",
            });
            126
        }
        Ended::Cancelled => 128 + psh_engine::host::SIGTERM,
    }
}
