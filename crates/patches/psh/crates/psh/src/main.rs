//! `psh -c CMD [NAME [ARGS…]]`, `psh FILE [ARGS…]`, or a script on stdin: the standalone
//! shell. It drives the engine on a thread of its own with an explicit stack (D1050),
//! parking while the engine is blocked.
//!
//! `psh --parse [--compact] [-O extglob] [-c CMD | FILE]` prints the syntax tree as
//! JSON (stdin when neither `-c` nor a file is given); status 0 if it parses, 2 if not.
//! With `--tree` it prints `psh_syntax::outline_text` instead.
//!
//! `psh --classify [--compact] [-O extglob] [-c CMD | FILE]` prints what the script does
//! as JSON (`classify_json.rs`); status 0 if it parses, 2 if not.
//!
//! `cli.rs` splits the command line between psh's own modes and bash's invocation.

mod classify_json;
mod cli;
mod json;
mod parse_json;
mod utils;

use std::io::{Read, Write};
use std::process::ExitCode;
use std::rc::Rc;

use psh_engine::invocation::{Invocation, parse_args};
use psh_engine::std_host::{drive, engine_thread};
use psh_engine::{Host, HostFile, Script, Shell, ShellConfig};

/// The shell's descriptors 0, 1 and 2.
type Stdio = [Option<Rc<dyn HostFile>>; 3];

/// The OS host. The standalone shell keeps its externals in its own process group, as a
/// non-interactive bash does, so whoever kills psh's group (a terminal's Ctrl-C, a
/// harness, `timeout`) reaches everything it started; background jobs outlive it as
/// with bash (D1067).
#[cfg(unix)]
fn os_host() -> (Rc<dyn Host>, Stdio) {
    use psh_os::unix::{Leftovers, Placement, UnixConfig, UnixFile, UnixHost};
    let host = UnixHost::new(UnixConfig {
        placement: Placement::Inherit,
        leftovers: Leftovers::Keep,
        // bash's rule: what was ignored when the shell started stays ignored.
        inherit_ignored: true,
        ..UnixConfig::default()
    });
    let stdio = |n| {
        UnixFile::stdio(n)
            .ok()
            .map(|f| Rc::new(f) as Rc<dyn HostFile>)
    };
    (Rc::new(host), [stdio(0), stdio(1), stdio(2)])
}

/// The OS host. As on Unix, externals stay in psh's console group (a Ctrl-C typed in
/// the console reaches them) and background jobs outlive it.
#[cfg(windows)]
fn os_host() -> (Rc<dyn Host>, Stdio) {
    use psh_os::windows::{Leftovers, Placement, WinFile, WindowsConfig, WindowsHost};
    let host = WindowsHost::new(WindowsConfig {
        placement: Placement::Inherit,
        leftovers: Leftovers::Keep,
        ..WindowsConfig::default()
    });
    let stdio = |n| {
        WinFile::stdio(n)
            .ok()
            .map(|f| Rc::new(f) as Rc<dyn HostFile>)
    };
    (Rc::new(host), [stdio(0), stdio(1), stdio(2)])
}

fn os_bytes(s: std::ffi::OsString) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        s.into_vec()
    }
    #[cfg(not(unix))]
    {
        s.to_string_lossy().into_owned().into_bytes()
    }
}

/// A path from the command line's bytes (not necessarily UTF-8 on Unix).
fn os_path(b: &[u8]) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b))
    }
    #[cfg(not(unix))]
    {
        std::path::PathBuf::from(String::from_utf8_lossy(b).into_owned())
    }
}

/// Set (to anything but empty), a command named `bash` or `sh` runs the program on
/// `PATH` instead of psh (D1410, D1412).
const NESTED_OFF_VAR: &str = "PSH_NESTED_OFF";

/// The standalone shell's configuration: the OS host and its stdio, the utilities (with
/// `PSH_UTILS_FORCE`), the environment and the working directory. `psh --utils` asks a
/// shell configured so which utilities run.
fn shell_config() -> ShellConfig {
    let (host, [stdin, stdout, stderr]) = os_host();
    let mut cfg = ShellConfig::new(host);
    cfg.utilities = psh_utils::policy(false);
    cfg.env = std::env::vars_os()
        .map(|(k, v)| (os_bytes(k), os_bytes(v)))
        .collect();
    // PSH_UTILS_FORCE (D1192): utilities that run in-process even in F/W mode; the
    // variable is psh's, so it is not passed on.
    if let Some(i) = cfg
        .env
        .iter()
        .position(|(k, _)| k == psh_utils::FORCE_VAR.as_bytes())
    {
        let (_, v) = cfg.env.remove(i);
        cfg.utilities.force_names = psh_utils::force_names(&v);
    }
    // PSH_NESTED_OFF (D1412): `bash` and `sh` go to PATH; psh's, so not passed on.
    if let Some(i) = cfg
        .env
        .iter()
        .position(|(k, _)| k == NESTED_OFF_VAR.as_bytes())
    {
        let (_, v) = cfg.env.remove(i);
        cfg.nested_shells = v.is_empty();
    }
    // A nested `bash` that runs the real program never runs psh itself (D1413).
    cfg.own_program = std::env::current_exe().ok().map(|p| {
        let b = os_bytes(p.into_os_string());
        if cfg!(windows) {
            b.into_iter()
                .map(|c| if c == b'\\' { b'/' } else { c })
                .collect()
        } else {
            b
        }
    });
    cfg.cwd = std::env::current_dir()
        .map(|p| os_bytes(p.into_os_string()))
        .unwrap_or_else(|_| b"/".to_vec());
    #[cfg(windows)]
    {
        cfg.cwd = cfg
            .cwd
            .iter()
            .map(|&b| if b == b'\\' { b'/' } else { b })
            .collect();
        cfg.env = psh_os::windows::shell_env(std::mem::take(&mut cfg.env));
    }
    cfg.stdin = stdin;
    cfg.stdout = stdout;
    cfg.stderr = stderr;
    cfg
}

/// A shell invocation error: bash's message, then the usage, status 2.
fn usage_error(msg: &str) -> ExitCode {
    eprintln!("psh: {msg}\n{}", cli::usage_hint());
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args = match cli::parse(std::env::args_os().skip(1).collect()) {
        Ok(cli::Mode::Shell(args)) => args,
        Ok(cli::Mode::Parse(d)) => return parse_command(d),
        Ok(cli::Mode::Classify(d)) => return classify_command(d),
        Ok(cli::Mode::Utils { json }) => return utils::list(json),
        Err(e) => {
            e.print();
            return ExitCode::from(e.exit_code() as u8);
        }
    };
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let script = match parse_args(&args) {
        Err(msg) => return usage_error(&msg),
        // `--help` after bash's options (`psh -e --help`).
        Ok(Invocation::Help) => {
            println!("{}", cli::help_text());
            return ExitCode::SUCCESS;
        }
        // After bash's options (`psh -e --version`), as `psh --version` prints it.
        Ok(Invocation::Version) => {
            println!("psh {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Ok(Invocation::Command {
            source,
            arg0,
            args,
            options,
        }) => Script {
            source,
            arg0: arg0.unwrap_or_else(|| b"psh".to_vec()),
            args,
            dash_c: true,
            options,
            stdin: false,
        },
        Ok(Invocation::File {
            path,
            args,
            options,
        }) => match std::fs::read(os_path(&path)) {
            Ok(src) => Script {
                source: src,
                arg0: path,
                args,
                dash_c: false,
                options,
                stdin: false,
            },
            Err(e) => {
                // As bash: 127 for a script that does not exist, 126 for one that
                // cannot be read (a directory, no permission). Windows reports a
                // directory as missing or as access denied, so ask.
                let dir = std::fs::metadata(os_path(&path)).is_ok_and(|m| m.is_dir());
                let why = if dir {
                    "Is a directory".to_string()
                } else {
                    psh_engine::host::error_text(&e)
                };
                eprintln!("psh: {}: {why}", text(&path));
                let missing = !dir && e.kind() == std::io::ErrorKind::NotFound;
                return ExitCode::from(if missing { 127 } else { 126 });
            }
        },
        // Read as it runs, one command at a time, as bash does (D1414).
        Ok(Invocation::Stdin { args, options }) => Script {
            source: Vec::new(),
            arg0: b"psh".to_vec(),
            args,
            dash_c: false,
            options,
            stdin: true,
        },
    };
    let signals = Signals::catch();
    let control = signals.control.clone();
    let thread = engine_thread(move || {
        let cfg = shell_config();
        let mut shell = Shell::new(cfg);
        Signals::attach(&control, shell.control());
        shell.start(script);
        let status = drive(&mut shell);
        (status, shell.killed_by())
    });
    match thread.map(|t| t.join()) {
        Ok(Ok((status, killed_by))) => {
            signals.finish(killed_by);
            ExitCode::from((status & 0xff) as u8)
        }
        _ => ExitCode::from(2),
    }
}

/// The standalone shell's signals (ARCH.md "psh-os", D1903): the ones a script may trap
/// are caught and sent to the shell (`ControlHandle::signal`), which runs a trap,
/// ignores them, or ends with `128+n`; a foreground command is abandoned then (TERM,
/// KILL after the grace period). Ended by a signal's default action, psh dies by it,
/// so its parent sees what bash shows. Embedded shells install nothing.
struct Signals {
    /// The running shell's control, once it exists, and the first signal caught.
    control: Control,
}

/// The signals the standalone shell catches (Linux numbers): those a script may trap
/// whose default action ends a process, but not QUIT (bash ignores it), PIPE (writes
/// see EPIPE) or the synchronous faults.
#[cfg(unix)]
const CAUGHT: [psh_engine::host::Signal; 12] = [1, 2, 10, 12, 14, 15, 24, 25, 26, 27, 29, 30];

type Control = std::sync::Arc<std::sync::Mutex<Option<psh_engine::ControlHandle>>>;

impl Signals {
    #[cfg(unix)]
    fn catch() -> Signals {
        let control: Control = Default::default();
        let c = control.clone();
        let _ = psh_os::unix::signals::catch(&CAUGHT, move |_sig| {
            // The handler set the signal's bit already (once the shell exists): this
            // only wakes the shell. Setting the bit again would deliver it twice when
            // the shell took the first already.
            if let Some(ctl) = &*c.lock().unwrap_or_else(|e| e.into_inner()) {
                ctl.wake();
            }
        });
        // Only the engine thread takes these signals (it unblocks them): a `kill $$`
        // from the script then acts before `kill` returns, as bash dies at once.
        psh_os::unix::signals::block_in_thread(&CAUGHT);
        Signals { control }
    }

    #[cfg(windows)]
    fn catch() -> Signals {
        let control: Control = Default::default();
        let c = control.clone();
        let _ = psh_os::windows::signals::catch(move |sig| {
            if let Some(ctl) = &*c.lock().unwrap_or_else(|e| e.into_inner()) {
                ctl.signal(sig);
            }
        });
        Signals { control }
    }

    /// The shell exists, on the engine thread: it takes the signals from now on (the
    /// handler cancels it directly), and a signal caught before cancels it now.
    fn attach(
        control: &std::sync::Mutex<Option<psh_engine::ControlHandle>>,
        ctl: psh_engine::ControlHandle,
    ) {
        #[cfg(unix)]
        {
            // One shell per process: the handle lives as long as the process.
            let word = Box::leak(Box::new(ctl.clone())).signal_word();
            psh_os::unix::signals::forward_to(word);
            psh_os::unix::signals::unblock_in_thread(&CAUGHT);
        }
        // A console event before the shell existed.
        #[cfg(windows)]
        if let Some(sig) = psh_os::windows::signals::first_caught() {
            ctl.signal(sig);
        }
        *control.lock().unwrap_or_else(|e| e.into_inner()) = Some(ctl);
    }

    /// The run is over: if a signal's default action ended it, wait until the
    /// abandoned externals were killed and reaped, and die by the signal.
    fn finish(self, killed_by: Option<i32>) {
        #[cfg(unix)]
        if let Some(sig) = killed_by {
            let grace = psh_os::unix::UnixConfig::DEFAULT_GRACE;
            psh_os::unix::settle(grace + std::time::Duration::from_secs(1));
            psh_os::unix::signals::die_by(sig);
        }
        #[cfg(windows)]
        if let Some(sig) = killed_by {
            let grace = psh_os::windows::WindowsConfig::DEFAULT_GRACE;
            psh_os::windows::settle(grace + std::time::Duration::from_secs(1));
            psh_os::windows::signals::die_by(sig);
        }
        let _ = (self, killed_by);
    }
}

fn parse_command(d: cli::Diag) -> ExitCode {
    let Some(bytes) = read_source(d.source) else {
        return ExitCode::from(2);
    };
    if d.tree {
        return parse_tree(&bytes, d.options);
    }
    let (json, ok) = parse_json::parse_to_json(&bytes, d.options);
    print_json(&json, d.compact, ok)
}

/// `--parse --tree`: the outline, or the error on stderr; status 0 or 2 as with JSON.
fn parse_tree(bytes: &[u8], options: psh_syntax::Options) -> ExitCode {
    match psh_syntax::parse_bytes(bytes, options) {
        Ok(ast) => {
            let text = psh_syntax::outline_text(&psh_syntax::outline(&ast));
            let mut stdout = std::io::stdout().lock();
            if stdout
                .write_all(text.as_bytes())
                .and_then(|_| stdout.flush())
                .is_err()
            {
                return ExitCode::from(2);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            let lc = psh_syntax::LineIndex::from_bytes(bytes).line_col_bytes(bytes, e.span.start);
            eprintln!("psh: line {}, column {}: {}", lc.line, lc.column, e.message);
            ExitCode::from(2)
        }
    }
}

fn classify_command(d: cli::Diag) -> ExitCode {
    // Invalid UTF-8 becomes U+FFFD, which moves the byte offsets after it.
    let Some(text) = read_source(d.source).map(|b| String::from_utf8_lossy(&b).into_owned()) else {
        return ExitCode::from(2);
    };
    let c = match psh_syntax::parse_with(&text, d.options) {
        Ok(ast) => psh_classify::classify_ast(&ast),
        // As `psh_classify::classify` reports a script that does not parse.
        Err(e) => {
            let mut c = psh_classify::Classification::default();
            c.categories.insert(psh_classify::Category::Unknown);
            c.error = Some(e);
            c
        }
    };
    let ok = c.error.is_none();
    print_json(&classify_json::to_json(&c, &text), d.compact, ok)
}

/// The script of `--parse` and `--classify`, byte for byte.
fn read_source(source: cli::Source) -> Option<Vec<u8>> {
    match source {
        cli::Source::Command(b) => Some(b),
        cli::Source::File(path) => match std::fs::read(&path) {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!(
                    "psh: {}: {}",
                    path.display(),
                    psh_engine::host::error_text(&e)
                );
                None
            }
        },
        cli::Source::Stdin => {
            let mut b = Vec::new();
            if let Err(e) = std::io::stdin().read_to_end(&mut b) {
                eprintln!("psh: stdin: {}", psh_engine::host::error_text(&e));
                return None;
            }
            Some(b)
        }
    }
}

/// Prints the JSON; status 0 if the input parsed, 2 if not or if stdout fails.
fn print_json(json: &json::Json, compact: bool, ok: bool) -> ExitCode {
    let out = if compact {
        json.to_compact()
    } else {
        json.to_pretty()
    };
    let mut stdout = std::io::stdout().lock();
    if writeln!(stdout, "{out}")
        .and_then(|_| stdout.flush())
        .is_err()
    {
        return ExitCode::from(2);
    }
    ExitCode::from(if ok { 0 } else { 2 })
}
