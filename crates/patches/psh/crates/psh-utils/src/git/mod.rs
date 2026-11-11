//! git's local read commands (nanocode's addition to psh's utilities): status, log,
//! diff, show, ls-files, rev-parse, cat-file, branch, blame, grep and the plumbing
//! around them, over the repository as the shell's file system shows it (loose
//! objects, packs, the index, references, linked worktrees).
//!
//! Output is git 2.50's, byte for byte, where the differential corpus covers it. What
//! changes a repository or needs the network is not here: such a command ends with
//! status 127 and a message that says what this git lacks, so that a host with a
//! machine can run the real one. A fallback utility (D015): where PATH has git, that
//! runs.

mod blame;
mod cmd;
mod diff;
mod extra;
mod index;
mod log;
mod patch;
mod pretty;
mod repo;
mod rev;
mod status;
mod store;
mod wild;
mod worktree;

use std::io::{self, BufWriter, Write};

use psh_engine::util::{BlockingOptions, SyncCtx, SyncWriter, UtilCtx};

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

/// A command that changes the repository or the worktree.
const WRITES: &[&str] = &[
    "add",
    "am",
    "apply",
    "bisect",
    "checkout",
    "cherry-pick",
    "clean",
    "commit",
    "commit-tree",
    "filter-branch",
    "format-patch",
    "gc",
    "hash-object",
    "init",
    "lfs",
    "maintenance",
    "merge",
    "mergetool",
    "mv",
    "notes",
    "pack-refs",
    "prune",
    "read-tree",
    "rebase",
    "repack",
    "replace",
    "rerere",
    "reset",
    "restore",
    "revert",
    "rm",
    "sparse-checkout",
    "submodule",
    "switch",
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
        format!("'{what}' changes the repository"),
        "git that writes: it reads status, log, diff, show, blame and the like",
    )
}

pub fn option(command: &str, arg: &str) -> Error {
    Error::Lacks(format!("'{command}' with '{arg}'"), "git with that option")
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let now = ctx.now().as_secs() as i64;
    let offset = i64::from(ctx.utc_offset(now));
    ctx.run_blocking(
        BlockingOptions { stdin: false },
        (argv.to_vec(), now, offset),
        body,
    )
    .await
}

fn body(ctx: &mut SyncCtx, (argv, now, offset): (Vec<Vec<u8>>, i64, i64)) -> i32 {
    let mut out = BufWriter::with_capacity(64 * 1024, ctx.writer(1));
    let mut err = ctx.writer(2);
    let result = start(ctx, &argv, (now, offset), &mut out, &mut err);
    let result = match result {
        Ok(status) => out.flush().map(|()| status).map_err(Error::Io),
        Err(e) => Err(e),
    };
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
            "--no-pager"
            | "-P"
            | "--no-optional-locks"
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
        env: ctx.env().to_vec(),
        columns,
    };
    let mut io = Io {
        out,
        err: err.clone(),
    };
    cmd::dispatch(&git, &mut io, &command, rest)
}

/// A configuration key as the files' parser names it.
fn config_key(key: &str) -> String {
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
