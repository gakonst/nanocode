//! `sed [-nEsz] [-i[SUFFIX]] [-l N] [-e SCRIPT]... [-f FILE]... [SCRIPT] [FILE...]`:
//! GNU sed's language and options (D1140), with both forms of `-i`: GNU's
//! `-i[SUFFIX]` and BSD's `-i SUFFIX`, told apart by D016's rule (D1141). Regexes are
//! psh-regex's POSIX BRE/ERE with GNU's extensions; `e` runs its commands in a fresh
//! psh shell (`sh -c`). `--debug` falls through to a real sed. Written for psh after
//! the GNU sed manual and POSIX.

mod compile;
mod exec;

use psh_engine::host::{FileKind, OpenOptions, error_text};
use psh_engine::util::UtilCtx;

use compile::{Error, Flags, Origin, Source};
use exec::{End, Reader, Run, Settings, Sink, State};

use crate::opts::{Spec, both, flag, long};

/// The options, for the registry and the classifier check; `run` parses argv itself
/// (`-i`'s two forms need lookahead).
pub const SPEC: Spec = Spec {
    name: "sed",
    opts: &[
        both('n', "quiet"),
        long("silent"),
        both('e', "expression").value(),
        both('f', "file").value(),
        both('E', "regexp-extended"),
        flag('r'),
        both('i', "in-place"),
        both('l', "line-length").value(),
        both('s', "separate"),
        both('z', "null-data"),
        long("zero-terminated"),
        both('u', "unbuffered"),
        both('b', "binary"),
        long("posix"),
        long("sandbox"),
        long("follow-symlinks"),
        long("debug"),
    ],
    posix_order: false,
};

const USAGE: &str = "Usage: sed [OPTION]... {script-only-if-no-other-script} [input-file]...";

enum ScriptArg {
    Expr(Vec<u8>),
    File(Vec<u8>),
}

#[derive(Default)]
struct Opts {
    quiet: bool,
    extended: bool,
    separate: bool,
    zero: bool,
    posix: bool,
    sandbox: bool,
    follow: bool,
    /// `-i`: the backup suffix (empty: none).
    in_place: Option<Vec<u8>>,
    line_wrap: Option<usize>,
    scripts: Vec<ScriptArg>,
    operands: Vec<Vec<u8>>,
}

enum ParseEnd {
    /// Run the real program (or report the option as unsupported).
    Unsupported(String),
    Usage(String),
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let utf8 = ctx.utf8();
    let o = match parse_args(argv, utf8) {
        Ok(o) => o,
        Err(ParseEnd::Unsupported(what)) => return fall_through(ctx, argv, &what).await,
        Err(ParseEnd::Usage(msg)) => {
            if !msg.is_empty() {
                ctx.error(&msg).await;
            }
            ctx.error_raw(format!("{USAGE}\n").as_bytes()).await;
            return 1;
        }
    };
    run_parsed(ctx, o, utf8).await
}

async fn fall_through(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>], what: &str) -> i32 {
    if let Some(status) = ctx.run_program(argv.to_vec()).await {
        return status;
    }
    ctx.error(&format!("{what} is not supported (psh built-in sed)"))
        .await;
    2
}

fn parse_args(argv: &[Vec<u8>], utf8: bool) -> Result<Opts, ParseEnd> {
    let mut o = Opts::default();
    // For telling `-i`'s forms apart: is -E given anywhere, does a -e/-f give the script.
    let any_ext = argv[1..].iter().any(|a| {
        a.as_slice() == b"--regexp-extended"
            || (a.len() > 1
                && a[0] == b'-'
                && a[1] != b'-'
                && a[1..].iter().any(|c| matches!(c, b'E' | b'r')))
    });
    let any_script_opt = argv[1..].iter().any(|a| {
        a.starts_with(b"--expression") || a.starts_with(b"--file") || {
            a.len() > 1
                && a[0] == b'-'
                && a[1] != b'-'
                && a[1..]
                    .iter()
                    .take_while(|c| !matches!(c, b'i' | b'l'))
                    .any(|c| matches!(c, b'e' | b'f'))
        }
    });
    let mut i = 1;
    let mut only_operands = false;
    let need = |i: usize, c: char| -> Result<Vec<u8>, ParseEnd> {
        argv.get(i)
            .cloned()
            .ok_or_else(|| ParseEnd::Usage(format!("option requires an argument -- '{c}'")))
    };
    while i < argv.len() {
        let a = &argv[i];
        i += 1;
        if only_operands || a.as_slice() == b"-" || a.first() != Some(&b'-') {
            o.operands.push(a.clone());
            continue;
        }
        if a.as_slice() == b"--" {
            only_operands = true;
            continue;
        }
        if let Some(rest) = a.strip_prefix(b"--") {
            let (name, value) = match rest.iter().position(|&b| b == b'=') {
                Some(k) => (&rest[..k], Some(rest[k + 1..].to_vec())),
                None => (rest, None),
            };
            let name = std::str::from_utf8(name).unwrap_or("");
            const LONGS: &[&str] = &[
                "expression",
                "file",
                "in-place",
                "line-length",
                "quiet",
                "silent",
                "regexp-extended",
                "separate",
                "null-data",
                "zero-terminated",
                "unbuffered",
                "binary",
                "posix",
                "sandbox",
                "follow-symlinks",
                "debug",
                "help",
                "version",
            ];
            let found: Vec<&str> = match LONGS.iter().find(|l| **l == name) {
                Some(l) => vec![l],
                None => LONGS
                    .iter()
                    .filter(|l| !name.is_empty() && l.starts_with(name))
                    .copied()
                    .collect(),
            };
            let [long] = found[..] else {
                return Err(ParseEnd::Unsupported(format!(
                    "option '--{}'",
                    String::from_utf8_lossy(rest)
                )));
            };
            let val = |i: &mut usize| -> Result<Vec<u8>, ParseEnd> {
                if let Some(v) = &value {
                    return Ok(v.clone());
                }
                let v = argv.get(*i).cloned().ok_or_else(|| {
                    ParseEnd::Usage(format!("option '--{long}' requires an argument"))
                })?;
                *i += 1;
                Ok(v)
            };
            match long {
                "expression" => o.scripts.push(ScriptArg::Expr(val(&mut i)?)),
                "file" => o.scripts.push(ScriptArg::File(val(&mut i)?)),
                "line-length" => o.line_wrap = Some(atoi(&val(&mut i)?)),
                "in-place" => o.in_place = Some(value.clone().unwrap_or_default()),
                "quiet" | "silent" => o.quiet = true,
                "regexp-extended" => o.extended = true,
                "separate" => o.separate = true,
                "null-data" | "zero-terminated" => o.zero = true,
                "posix" => o.posix = true,
                "sandbox" => o.sandbox = true,
                "follow-symlinks" => o.follow = true,
                "unbuffered" | "binary" => {}
                _ => return Err(ParseEnd::Unsupported(format!("option '--{long}'"))),
            }
            continue;
        }
        // A cluster of short options.
        let mut j = 1;
        while j < a.len() {
            let c = a[j];
            j += 1;
            match c {
                b'n' => o.quiet = true,
                b'E' | b'r' => o.extended = true,
                b's' => o.separate = true,
                b'z' => o.zero = true,
                b'u' | b'b' => {}
                b'e' | b'f' | b'l' => {
                    let v = if j < a.len() {
                        a[j..].to_vec()
                    } else {
                        let v = need(i, c as char)?;
                        i += 1;
                        v
                    };
                    j = a.len();
                    match c {
                        b'e' => o.scripts.push(ScriptArg::Expr(v)),
                        b'f' => o.scripts.push(ScriptArg::File(v)),
                        _ => o.line_wrap = Some(atoi(&v)),
                    }
                }
                b'i' => {
                    if j < a.len() {
                        o.in_place = Some(a[j..].to_vec());
                        j = a.len();
                    } else if bsd_suffix(
                        argv,
                        i,
                        any_script_opt || !o.scripts.is_empty(),
                        any_ext,
                        utf8,
                    ) {
                        o.in_place = Some(argv[i].clone());
                        i += 1;
                    } else {
                        o.in_place = Some(Vec::new());
                    }
                }
                _ => {
                    return Err(ParseEnd::Unsupported(format!(
                        "option '-{}'",
                        String::from_utf8_lossy(&[c])
                    )));
                }
            }
        }
    }
    Ok(o)
}

/// GNU's `-l` takes what number it can read.
fn atoi(v: &[u8]) -> usize {
    let digits: Vec<u8> = v
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .copied()
        .collect();
    std::str::from_utf8(&digits)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// D016 for a bare `-i` followed by `argv[next]` (D1141): whether that word is BSD's
/// backup suffix rather than GNU's script or file. `''` is a suffix; an option is not.
/// When the script is an operand: a word that parses as a script with operands after
/// it is GNU's script; a `.`-prefixed word without command characters followed by a
/// script is a BSD suffix. When `-e`/`-f` give the script: such a word followed by an
/// option is a BSD suffix (`sed -i .bak -e s/a/b/ f`), anything else a file.
fn bsd_suffix(
    argv: &[Vec<u8>],
    next: usize,
    script_opts: bool,
    extended: bool,
    utf8: bool,
) -> bool {
    let Some(word) = argv.get(next) else {
        return false;
    };
    if word.is_empty() {
        return true;
    }
    if word.len() > 1 && word[0] == b'-' {
        return false;
    }
    let after = &argv[next + 1..];
    let dotted = word[0] == b'.' && !word.iter().any(|b| b" \t\n;/{}\\!$=,~#".contains(b));
    if script_opts {
        return dotted && after.first().is_some_and(|f| f.len() > 1 && f[0] == b'-');
    }
    let parses = |s: &[u8]| {
        let src = Source {
            pieces: vec![(s.to_vec(), Origin::Expr(1))],
        };
        let flags = Flags {
            extended,
            utf8,
            ..Flags::default()
        };
        compile::compile(&src, flags).is_ok()
    };
    if parses(word) && !after.is_empty() {
        return false;
    }
    dotted && after.first().is_some_and(|s| parses(s))
}

async fn run_parsed(ctx: &mut UtilCtx<'_>, mut o: Opts, utf8: bool) -> i32 {
    if o.scripts.is_empty() {
        if o.operands.is_empty() {
            ctx.error_raw(format!("{USAGE}\n").as_bytes()).await;
            return 1;
        }
        o.scripts.push(ScriptArg::Expr(o.operands.remove(0)));
    }
    let mut src = Source::default();
    let mut n = 0;
    for s in &o.scripts {
        match s {
            ScriptArg::Expr(e) => {
                n += 1;
                src.pieces.push((e.clone(), Origin::Expr(n)));
            }
            ScriptArg::File(f) => {
                let mut text = Vec::new();
                let r = if f.as_slice() == b"-" {
                    ctx.read_to_end(&mut text).await
                } else {
                    match ctx.open(f, OpenOptions::read()).await {
                        Ok(file) => file.read_to_end(&mut text).await,
                        Err(e) => Err(e),
                    }
                };
                if let Err(e) = r {
                    let shown = String::from_utf8_lossy(f).into_owned();
                    ctx.error(&format!("couldn't open file {shown}: {}", error_text(&e)))
                        .await;
                    return 4;
                }
                // The file's last newline ends its last line, not an empty one.
                if text.last() == Some(&b'\n') {
                    text.pop();
                }
                src.pieces.push((text, Origin::File(f.clone())));
            }
        }
    }
    let flags = Flags {
        extended: o.extended,
        utf8,
        posix: o.posix,
        sandbox: o.sandbox,
    };
    let prog = match compile::compile(&src, flags) {
        Ok(p) => p,
        Err(Error::At(msg)) => {
            ctx.error(&msg).await;
            return 1;
        }
        // GNU checks labels after compiling, with status 4.
        Err(Error::Plain(msg)) => {
            ctx.error(&msg).await;
            return 4;
        }
    };
    let set = Settings {
        quiet: o.quiet || prog.quiet,
        zero: o.zero,
        line_wrap: o.line_wrap.unwrap_or(70),
        utf8,
    };
    let mut st = match State::new(ctx, &prog).await {
        Ok(s) => s,
        Err(status) => return status,
    };
    let delim = if o.zero { 0 } else { b'\n' };
    let mut status = 0;
    let end = if let Some(suffix) = &o.in_place {
        if o.operands.is_empty() {
            ctx.error("no input files").await;
            return 4;
        }
        let mut run = Run::new(&prog, &set, Sink::Buffer(Vec::new()));
        let mut end = End::Input;
        for f in &o.operands {
            match edit_in_place(ctx, &mut run, &mut st, f, suffix, o.follow, delim).await {
                Ok((e, s)) => {
                    status = status.max(s);
                    if e != End::Input {
                        end = e;
                        break;
                    }
                }
                Err(s) => {
                    end = End::Fail(s);
                    break;
                }
            }
        }
        end
    } else {
        let files = if o.operands.is_empty() {
            vec![b"-".to_vec()]
        } else {
            o.operands.clone()
        };
        let mut run = Run::new(&prog, &set, Sink::Stdout);
        let mut end = End::Input;
        if o.separate {
            for f in files {
                let mut reader = Reader::new(vec![f], delim);
                run.new_file();
                st.rewind();
                end = run.run(ctx, &mut st, &mut reader).await;
                status = status.max(reader.status);
                if end != End::Input {
                    break;
                }
            }
        } else {
            let mut reader = Reader::new(files, delim);
            end = run.run(ctx, &mut st, &mut reader).await;
            status = status.max(reader.status);
        }
        if let Err(s) = run.flush(ctx).await {
            return s;
        }
        end
    };
    if let Err(s) = st.flush(ctx, &prog).await {
        return s;
    }
    match end {
        End::Input => status,
        End::Quit(code) => code,
        End::Fail(s) => s,
    }
}

/// `-i`: runs over one file into memory, then replaces the file (through a temporary
/// file in its directory, keeping its mode), first renaming it to its backup name.
async fn edit_in_place(
    ctx: &mut UtilCtx<'_>,
    run: &mut Run<'_>,
    st: &mut State,
    name: &[u8],
    suffix: &[u8],
    follow: bool,
    delim: u8,
) -> Result<(End, i32), i32> {
    let shown = String::from_utf8_lossy(name).into_owned();
    let meta = match ctx.stat(name, true).await {
        Ok(m) => m,
        Err(e) => {
            ctx.error(&format!("can't read {shown}: {}", error_text(&e)))
                .await;
            return Ok((End::Input, 2));
        }
    };
    if meta.kind != FileKind::File || name == b"-" {
        ctx.error(&format!("couldn't edit {shown}: not a regular file"))
            .await;
        return Err(4);
    }
    let path = if follow {
        ctx.canonicalize(name)
            .await
            .unwrap_or_else(|_| name.to_vec())
    } else {
        name.to_vec()
    };
    let mut reader = Reader::new(vec![path.clone()], delim);
    run.new_file();
    st.rewind();
    run.set_sink(Sink::Buffer(Vec::new()));
    let end = run.run(ctx, st, &mut reader).await;
    run.flush(ctx).await?;
    let Sink::Buffer(data) = run.set_sink(Sink::Buffer(Vec::new())) else {
        unreachable!()
    };
    if let End::Fail(s) = end {
        return Err(s);
    }
    let (dir, base) = match path.iter().rposition(|&b| b == b'/') {
        Some(k) => (path[..k + 1].to_vec(), path[k + 1..].to_vec()),
        None => (Vec::new(), path.clone()),
    };
    let temp = temp_name(ctx, &dir);
    let opts = OpenOptions {
        mode: Some(meta.mode & 0o7777),
        create_new: true,
        ..OpenOptions::write()
    };
    let fail = |what: &str, e: std::io::Error| format!("{what}: {}", error_text(&e));
    let file = match ctx.open(&temp, opts).await {
        Ok(f) => f,
        Err(e) => {
            let t = String::from_utf8_lossy(&temp).into_owned();
            ctx.error(&fail(&format!("couldn't open temporary file {t}"), e))
                .await;
            return Err(4);
        }
    };
    if let Err(e) = file.write_all(&data).await {
        drop(file);
        let _ = ctx.remove_file(&temp).await;
        let t = String::from_utf8_lossy(&temp).into_owned();
        ctx.error(&fail(&format!("couldn't write {t}"), e)).await;
        return Err(4);
    }
    drop(file);
    // Hosts that do not apply the mode at creation (Windows) still get it set.
    let _ = ctx.set_mode(&temp, meta.mode & 0o7777).await;
    if !suffix.is_empty() {
        let backup = if suffix.contains(&b'*') {
            let mut b = dir.clone();
            for &c in suffix {
                if c == b'*' {
                    b.extend_from_slice(&base);
                } else {
                    b.push(c);
                }
            }
            b
        } else {
            let mut b = path.clone();
            b.extend_from_slice(suffix);
            b
        };
        if let Err(e) = ctx.rename(&path, &backup).await {
            let _ = ctx.remove_file(&temp).await;
            let b = String::from_utf8_lossy(&backup).into_owned();
            ctx.error(&fail(&format!("cannot rename {shown} to {b}"), e))
                .await;
            return Err(4);
        }
    }
    if let Err(e) = ctx.rename(&temp, &path).await {
        let _ = ctx.remove_file(&temp).await;
        let t = String::from_utf8_lossy(&temp).into_owned();
        ctx.error(&fail(&format!("cannot rename {t}"), e)).await;
        return Err(4);
    }
    Ok((end, reader.status))
}

/// `DIR/sedXXXXXX` like GNU's temporary files, from the clock and a counter.
fn temp_name(ctx: &UtilCtx<'_>, dir: &[u8]) -> Vec<u8> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut x = ctx.now().as_nanos() as u64 ^ COUNTER.fetch_add(0x9e37_79b9, Ordering::Relaxed);
    let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut name = dir.to_vec();
    name.extend_from_slice(b"sed");
    for _ in 0..6 {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        name.push(chars[(x >> 33) as usize % chars.len()]);
    }
    name
}

#[cfg(test)]
mod tests;
