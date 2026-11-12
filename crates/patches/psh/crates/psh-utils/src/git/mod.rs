//! git's local commands (nanocode's addition to psh's utilities): status, log, diff,
//! show, ls-files, rev-parse, cat-file, branch, blame, grep and the plumbing around
//! them, and the ones that change a repository without merging: init, add, rm, mv,
//! commit, checkout, switch, restore, reset, stash, branch, tag and config. All of it
//! over the repository as the shell's file system shows it (loose objects, packs, the
//! index, references, linked worktrees).
//!
//! Output is git 2.50's, byte for byte, where the differential corpus covers it, and a
//! repository this has written is one git reads. What merges histories or needs the
//! network is not here: such a command ends with status 127 and a message that says
//! what this git lacks, so that a host with a machine can run the real one. A fallback
//! utility (D015): where PATH has git, that runs.

mod blame;
mod checkout;
mod cmd;
mod commit;
mod diff;
mod extra;
mod index;
mod line_diff;
mod log;
mod patch;
mod pretty;
mod repo;
mod rev;
mod setup;
mod stage;
mod stash;
mod status;
mod store;
mod wild;
mod worktree;
mod write;

use std::cell::RefCell;
use std::io::{self, BufWriter, Write};
use std::sync::mpsc;

use psh_engine::util::{BlockingOptions, SyncCtx, SyncWriter, UtilCtx};

pub(crate) use wild::wildmatch;

use repo::Repo;
use store::Fs;

/// The git whose output this matches.
const VERSION: &str = "2.50.1";

pub enum Error {
    Io(io::Error),
    /// fatal: MESSAGE, status 128.
    Fatal(String),
    /// A usage error: the message as it is, status 129.
    Usage(String),
    /// What was asked for and what this git lacks; status 127.
    Lacks(String, &'static str),
    /// Nothing more to say; the status.
    Exit(i32),
    /// What git prints to stderr, as it is, and the status.
    Says(String, i32),
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Io(e)
    }
}

pub type Res<T> = Result<T, Error>;

pub type Out = BufWriter<SyncWriter>;

/// What a command runs with.
pub struct Git<'a> {
    pub repo: Repo<'a>,
    /// The working directory inside the worktree: empty at its root, else with a
    /// trailing slash.
    pub prefix: Vec<u8>,
    pub cwd: Vec<u8>,
    /// Seconds since the epoch.
    pub now: i64,
    /// The local zone's offset from UTC, in seconds.
    pub offset: i64,
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
    /// COLUMNS, when it is set.
    pub columns: Option<usize>,
    /// What the command changes, carried out once its body has ended.
    pub plan: RefCell<write::Plan>,
}

impl Git<'_> {
    /// The worktree, or git's message for a command that needs one.
    pub fn work_tree(&self) -> Res<&[u8]> {
        match &self.repo.work_tree {
            Some(tree) => Ok(tree),
            None => Err(Error::Fatal(
                "this operation must be run in a work tree".to_string(),
            )),
        }
    }
}

/// Where a command writes.
pub struct Io<'a> {
    pub out: &'a mut Out,
    pub err: SyncWriter,
}

impl Io<'_> {
    pub fn warn(&mut self, msg: &str) {
        let _ = self.out.flush();
        let _ = self.err.write_all(msg.as_bytes());
    }
}

/// A command that changes the repository or the worktree in a way this git does not.
const WRITES: &[&str] = &[
    "am",
    "apply",
    "bisect",
    "cherry-pick",
    "commit-tree",
    "filter-branch",
    "format-patch",
    "gc",
    "hash-object",
    "lfs",
    "maintenance",
    "mergetool",
    "notes",
    "pack-refs",
    "prune",
    "read-tree",
    "rebase",
    "repack",
    "replace",
    "rerere",
    "revert",
    "sparse-checkout",
    "submodule",
    "update-index",
    "update-ref",
    "write-tree",
];

/// A command that talks to another repository.
const NETWORK: &[&str] = &[
    "clone",
    "fetch",
    "pull",
    "push",
    "ls-remote",
    "send-email",
    "request-pull",
    "svn",
    "bundle",
];

pub fn writes(what: &str) -> Error {
    Error::Lacks(
        format!("'{what}' changes the repository in a way it does not"),
        "git that merges, rewrites history or repacks: it reads, stages, commits and checks out",
    )
}

pub fn option(command: &str, arg: &str) -> Error {
    Error::Lacks(format!("'{command}' with '{arg}'"), "git with that option")
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let now = ctx.now().as_secs() as i64;
    let offset = i64::from(ctx.utc_offset(now));
    let (planned, plans) = mpsc::channel();
    let status = ctx
        .run_blocking(
            BlockingOptions { stdin: false },
            (argv.to_vec(), now, offset, planned),
            body,
        )
        .await;
    // The body ran on answers that were all there, or this is not reached: the plan
    // of a run that is thrown away is never carried out.
    let Ok(plan) = plans.try_recv() else {
        return status;
    };
    match write::apply(ctx, plan).await {
        Ok(()) => status,
        Err(e) => {
            let text = format!("fatal: {}\n", psh_engine::host::error_text(&e));
            ctx.error_raw(text.as_bytes()).await;
            128
        }
    }
}

type Args = (Vec<Vec<u8>>, i64, i64, mpsc::Sender<write::Plan>);

fn body(ctx: &mut SyncCtx, (argv, now, offset, planned): Args) -> i32 {
    let mut out = BufWriter::with_capacity(64 * 1024, ctx.writer(1));
    let mut err = ctx.writer(2);
    let mut plan = write::Plan::default();
    let result = start(ctx, &argv, (now, offset), &mut out, &mut err, &mut plan);
    let result = match result {
        Ok(status) => out.flush().map(|()| status).map_err(Error::Io),
        Err(e) => Err(e),
    };
    if result.is_ok() && (!plan.ops.is_empty() || plan.index.is_some()) {
        let _ = planned.send(plan);
    }
    match result {
        Ok(status) => status,
        Err(e) => {
            let flushed = out.flush();
            let (text, status) = match e {
                Error::Io(e) if e.kind() == io::ErrorKind::BrokenPipe => return 141,
                // The host has yet to answer: this run is thrown away and run again.
                Error::Io(e) if e.kind() == io::ErrorKind::WouldBlock => return 1,
                Error::Io(e) => (
                    format!("fatal: {}\n", psh_engine::host::error_text(&e)),
                    128,
                ),
                Error::Fatal(msg) => (format!("fatal: {msg}\n"), 128),
                Error::Usage(msg) => (format!("{msg}\n"), 129),
                Error::Lacks(what, lacks) => (
                    format!("git: {what} (this simulated shell has no {lacks})\n"),
                    127,
                ),
                Error::Exit(status) => (String::new(), status),
                Error::Says(text, status) => (text, status),
            };
            if flushed.is_err_and(|e| e.kind() == io::ErrorKind::BrokenPipe) {
                return 141;
            }
            let _ = err.write_all(text.as_bytes());
            status
        }
    }
}

fn start(
    ctx: &SyncCtx,
    argv: &[Vec<u8>],
    (now, offset): (i64, i64),
    out: &mut Out,
    err: &mut SyncWriter,
    plan: &mut write::Plan,
) -> Res<i32> {
    let mut args = Vec::with_capacity(argv.len());
    for arg in &argv[1..] {
        match std::str::from_utf8(arg) {
            Ok(arg) => args.push(arg.to_string()),
            Err(_) => {
                return Err(Error::Lacks(
                    "an argument is not UTF-8".to_string(),
                    "git for such names",
                ));
            }
        }
    }
    let mut cwd = ctx.cwd().to_vec();
    let mut overrides = Vec::new();
    let mut git_dir = None;
    let mut work_tree = None;
    let mut optional_locks = true;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let value = |i: &mut usize| -> Res<String> {
            *i += 1;
            args.get(*i)
                .cloned()
                .ok_or_else(|| Error::Usage(format!("error: no value given for '{arg}'")))
        };
        match arg {
            "-C" => {
                let dir = value(&mut i)?;
                cwd = if dir.starts_with('/') {
                    dir.into_bytes()
                } else {
                    store::join(&cwd, dir.as_bytes())
                };
            }
            "-c" => {
                let pair = value(&mut i)?;
                let (key, val) = pair.split_once('=').unwrap_or((&pair, "true"));
                overrides.push((config_key(key), val.to_string()));
            }
            "--git-dir" => git_dir = Some(value(&mut i)?),
            "--work-tree" => work_tree = Some(value(&mut i)?),
            "--no-optional-locks" => optional_locks = false,
            "--no-pager"
            | "-P"
            | "--literal-pathspecs"
            | "--no-replace-objects"
            | "--paginate"
            | "-p" => {}
            "--version" | "-v" => {
                writeln!(out, "git version {VERSION}")?;
                return Ok(0);
            }
            _ if arg.starts_with("--git-dir=") => git_dir = Some(arg[10..].to_string()),
            _ if arg.starts_with("--work-tree=") => work_tree = Some(arg[12..].to_string()),
            _ if arg.starts_with('-') => return Err(option("git", arg)),
            _ => break,
        }
        i += 1;
    }
    let Some(command) = args.get(i).cloned() else {
        return Err(Error::Lacks(
            "no command was named".to_string(),
            "git's help pages",
        ));
    };
    let rest = &args[i + 1..];
    if command == "version" {
        writeln!(out, "git version {VERSION}")?;
        return Ok(0);
    }
    if command == "help" || rest.iter().any(|a| a == "--help") {
        return Err(Error::Lacks(
            format!("help for '{command}'"),
            "git's help pages",
        ));
    }
    if NETWORK.contains(&command.as_str()) {
        return Err(Error::Lacks(
            format!(
                "'{command}' talks to another repository: network access is not available in this shell"
            ),
            "git that reaches other repositories",
        ));
    }
    if WRITES.contains(&command.as_str()) {
        return Err(writes(&command));
    }
    let cwd = repo::normalize(&cwd);
    let env = |name: &[u8]| -> Option<Vec<u8>> {
        match name {
            b"GIT_DIR" if git_dir.is_some() => git_dir.clone().map(String::into_bytes),
            b"GIT_WORK_TREE" if work_tree.is_some() => work_tree.clone().map(String::into_bytes),
            _ => ctx.var(name).map(<[u8]>::to_vec),
        }
    };
    let fs = Fs::new(ctx);
    if fs.stat_opt(&cwd, true)?.is_none() {
        return Err(Error::Fatal(format!(
            "cannot change to '{}': No such file or directory",
            String::from_utf8_lossy(&cwd)
        )));
    }
    let mut io = Io {
        out,
        err: err.clone(),
    };
    if let Some(status) = setup::outside(&fs, &cwd, &env, &command, rest, &mut io, plan)? {
        return Ok(status);
    }
    let repo = Repo::discover(&fs, &cwd, &env, &overrides)?;
    let prefix = match &repo.work_tree {
        Some(tree) if cwd.starts_with(tree) && cwd.len() > tree.len() => {
            let mut prefix = cwd[tree.len()..].to_vec();
            if prefix.first() == Some(&b'/') {
                prefix.remove(0);
            }
            if !prefix.is_empty() {
                prefix.push(b'/');
            }
            prefix
        }
        _ => Vec::new(),
    };
    let columns = ctx
        .var(b"COLUMNS")
        .and_then(|v| std::str::from_utf8(v).ok()?.parse().ok());
    let git = Git {
        repo,
        prefix,
        cwd,
        now,
        offset,
        env: {
            let mut env = ctx.env().to_vec();
            if !optional_locks {
                env.retain(|(name, _)| name != b"GIT_OPTIONAL_LOCKS");
                env.push((b"GIT_OPTIONAL_LOCKS".to_vec(), b"0".to_vec()));
            }
            env
        },
        columns,
        plan: RefCell::default(),
    };
    let status = cmd::dispatch(&git, &mut io, &command, rest)?;
    *plan = git.plan.take();
    let objects = git.repo.odb.made();
    plan.ops.splice(
        0..0,
        objects.into_iter().map(|(path, data)| write::Op::Write {
            path,
            data,
            mode: write::Mode::Object,
        }),
    );
    Ok(status)
}

/// A configuration key as the files' parser names it.
pub fn config_key(key: &str) -> String {
    match (key.find('.'), key.rfind('.')) {
        (Some(first), Some(last)) if first != last => format!(
            "{}{}{}",
            key[..first].to_ascii_lowercase(),
            &key[first..last],
            key[last..].to_ascii_lowercase()
        ),
        _ => key.to_ascii_lowercase(),
    }
}
