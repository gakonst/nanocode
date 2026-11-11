//! `xargs` with GNU semantics plus BSD's empty-input rule (D1170): items from stdin (or
//! `-a FILE`) are appended to a command, as many as fit (`-s`, 128 KiB by default), or
//! `-n` per command, or `-L` lines per command, or one line per command with `-I`
//! replacing a string in the arguments. Input splits on blanks and newlines with
//! quotes and backslashes (`-0` and `-d` split on one byte, literally); `-E` ends it at
//! a word. Commands go through the shell's dispatch (utilities, standalone builtins,
//! programs), run with stdin from `/dev/null` (`/dev/tty` with `-o`; xargs's own stdin
//! with `-a`), `-P` at once at most, and are stopped with xargs. Statuses as GNU's: 123
//! when a command failed (1–125), 124 when one exited 255, 125 for a signal death, 126
//! and 127 when a command could not run. Written for psh after GNU's and BSD's manuals.

use std::task::Poll;

use psh_engine::util::{SpawnEnv, UtilChild, UtilCtx};

use crate::common;
use crate::opts::{Spec, both, flag, long};

pub const SPEC: Spec = Spec {
    name: "xargs",
    opts: &[
        both('0', "null"),
        both('a', "arg-file").value(),
        both('d', "delimiter").value(),
        flag('E').value(),
        both('e', "eof").optional(),
        flag('I').value(),
        both('i', "replace").optional(),
        flag('L').value(),
        both('l', "max-lines").optional(),
        both('n', "max-args").value(),
        both('P', "max-procs").value(),
        both('p', "interactive"),
        both('r', "no-run-if-empty"),
        both('s', "max-chars").value(),
        both('t', "verbose"),
        both('x', "exit"),
        both('o', "open-tty"),
        long("show-limits"),
        long("process-slot-var").value(),
    ],
    posix_order: true,
};

/// GNU's default command-line size.
const DEFAULT_MAX_CHARS: usize = 128 * 1024;

#[derive(Debug, Default)]
struct Opts {
    /// The separator byte (`-0`, `-d`), else blanks, newlines and quotes.
    delim: Option<u8>,
    arg_file: Option<Vec<u8>>,
    eof: Option<Vec<u8>>,
    replace: Option<Vec<u8>>,
    max_lines: Option<usize>,
    max_args: Option<usize>,
    max_procs: usize,
    max_chars: usize,
    verbose: bool,
    exit: bool,
    open_tty: bool,
}

/// What the parser could not take: the real program gets the command line.
enum Parse {
    Ok(Opts, Vec<Vec<u8>>),
    /// An option psh does not implement.
    FallThrough(String),
    /// A usage error: message, status.
    Error(String),
}

fn number(name: &str, v: &[u8], min: usize) -> Result<usize, String> {
    let text = String::from_utf8_lossy(v);
    match text.trim().parse::<usize>() {
        Ok(n) if n >= min => Ok(n),
        Ok(_) => Err(format!("value {text} for {name} option should be >= {min}")),
        Err(_) => Err(format!("invalid number \"{text}\" for {name} option")),
    }
}

/// `-d`'s argument: one byte, or a C escape (`\n`, `\t`, `\0`, `\x41`, `\101`).
fn delimiter(v: &[u8]) -> Result<u8, String> {
    let bad = || {
        format!(
            "invalid input delimiter specification {}: the delimiter must be either a single character or an escape sequence starting with \\",
            String::from_utf8_lossy(v)
        )
    };
    match v {
        [b] => Ok(*b),
        [b'\\', rest @ ..] => match rest {
            [b'n'] => Ok(b'\n'),
            [b't'] => Ok(b'\t'),
            [b'r'] => Ok(b'\r'),
            [b'a'] => Ok(7),
            [b'b'] => Ok(8),
            [b'f'] => Ok(12),
            [b'v'] => Ok(11),
            [b'\\'] => Ok(b'\\'),
            [b'x', hex @ ..] if !hex.is_empty() && hex.len() <= 2 => {
                u8::from_str_radix(std::str::from_utf8(hex).map_err(|_| bad())?, 16)
                    .map_err(|_| bad())
            }
            oct if !oct.is_empty()
                && oct.len() <= 3
                && oct.iter().all(|c| (b'0'..=b'7').contains(c)) =>
            {
                u32::from_str_radix(std::str::from_utf8(oct).map_err(|_| bad())?, 8)
                    .ok()
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or_else(bad)
            }
            _ => Err(bad()),
        },
        _ => Err(bad()),
    }
}

/// GNU getopt order: options until the first operand (the command), `--` ends them; the
/// old forms with an optional attached value (`-e[EOF]`, `-i[R]`, `-l[N]`).
fn parse(args: &[Vec<u8>]) -> Parse {
    let mut o = Opts {
        max_procs: 1,
        max_chars: DEFAULT_MAX_CHARS,
        ..Opts::default()
    };
    let mut i = 0;
    macro_rules! value {
        ($rest:expr, $name:expr) => {{
            if !$rest.is_empty() {
                $rest.to_vec()
            } else if let Some(v) = args.get(i + 1) {
                i += 1;
                v.clone()
            } else {
                return Parse::Error(format!("option requires an argument -- '{}'", $name));
            }
        }};
    }
    macro_rules! try_ {
        ($e:expr) => {
            match $e {
                Ok(v) => v,
                Err(m) => return Parse::Error(m),
            }
        };
    }
    while i < args.len() {
        let a = &args[i];
        if a == b"--" {
            i += 1;
            break;
        }
        if a.len() < 2 || a[0] != b'-' {
            break;
        }
        if let Some(rest) = a.strip_prefix(b"--") {
            let text = String::from_utf8_lossy(rest).into_owned();
            let (name, inline) = match text.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(v.as_bytes().to_vec())),
                None => (text.clone(), None),
            };
            let longs = [
                "null",
                "arg-file",
                "delimiter",
                "eof",
                "replace",
                "max-lines",
                "max-args",
                "max-procs",
                "interactive",
                "no-run-if-empty",
                "max-chars",
                "verbose",
                "exit",
                "open-tty",
                "show-limits",
                "process-slot-var",
            ];
            let hits: Vec<&str> = longs
                .iter()
                .copied()
                .filter(|l| l.starts_with(&name))
                .collect();
            let full = match hits.as_slice() {
                [one] => *one,
                _ if longs.contains(&name.as_str()) => longs
                    .iter()
                    .copied()
                    .find(|l| *l == name)
                    .unwrap_or_default(),
                [] => return Parse::FallThrough(format!("--{name}")),
                _ => return Parse::Error(format!("option '--{name}' is ambiguous")),
            };
            let mut required = || -> Option<Vec<u8>> {
                if let Some(v) = &inline {
                    return Some(v.clone());
                }
                let v = args.get(i + 1)?.clone();
                i += 1;
                Some(v)
            };
            match full {
                "null" => o.delim = Some(0),
                "arg-file" => match required() {
                    Some(v) => o.arg_file = Some(v),
                    None => return Parse::Error("option '--arg-file' requires an argument".into()),
                },
                "delimiter" => match required() {
                    Some(v) => o.delim = Some(try_!(delimiter(&v))),
                    None => {
                        return Parse::Error("option '--delimiter' requires an argument".into());
                    }
                },
                "eof" => o.eof = inline.filter(|v| !v.is_empty()),
                "replace" => {
                    o.replace = Some(inline.unwrap_or_else(|| b"{}".to_vec()));
                    o.max_lines = None;
                    o.max_args = None;
                }
                "max-lines" => {
                    o.max_lines = Some(match inline {
                        Some(v) => try_!(number("--max-lines", &v, 1)),
                        None => 1,
                    });
                    o.max_args = None;
                    o.replace = None;
                }
                "max-args" => match required() {
                    Some(v) => {
                        o.max_args = Some(try_!(number("-n", &v, 1)));
                        o.max_lines = None;
                    }
                    None => return Parse::Error("option '--max-args' requires an argument".into()),
                },
                "max-procs" => match required() {
                    Some(v) => o.max_procs = try_!(number("-P", &v, 0)),
                    None => {
                        return Parse::Error("option '--max-procs' requires an argument".into());
                    }
                },
                "max-chars" => match required() {
                    Some(v) => o.max_chars = try_!(number("-s", &v, 1)),
                    None => {
                        return Parse::Error("option '--max-chars' requires an argument".into());
                    }
                },
                "no-run-if-empty" => {}
                "verbose" => o.verbose = true,
                "exit" => o.exit = true,
                "open-tty" => o.open_tty = true,
                // Prompting, limits and slot variables are not built in.
                other => return Parse::FallThrough(format!("--{other}")),
            }
            i += 1;
            continue;
        }
        // A cluster of short options.
        let mut j = 1;
        while j < a.len() {
            let c = a[j];
            let rest = &a[j + 1..];
            match c {
                b'0' => o.delim = Some(0),
                b'r' => {}
                b't' => o.verbose = true,
                b'x' => o.exit = true,
                b'o' => o.open_tty = true,
                b'a' => {
                    o.arg_file = Some(value!(rest, "a"));
                    break;
                }
                b'd' => {
                    let v = value!(rest, "d");
                    o.delim = Some(try_!(delimiter(&v)));
                    break;
                }
                b'E' => {
                    let v = value!(rest, "E");
                    o.eof = (!v.is_empty()).then_some(v);
                    break;
                }
                b'e' => {
                    o.eof = (!rest.is_empty()).then(|| rest.to_vec());
                    break;
                }
                b'I' => {
                    o.replace = Some(value!(rest, "I"));
                    o.max_lines = None;
                    o.max_args = None;
                    break;
                }
                b'i' => {
                    o.replace = Some(if rest.is_empty() {
                        b"{}".to_vec()
                    } else {
                        rest.to_vec()
                    });
                    o.max_lines = None;
                    o.max_args = None;
                    break;
                }
                b'L' => {
                    let v = value!(rest, "L");
                    o.max_lines = Some(try_!(number("-L", &v, 1)));
                    o.max_args = None;
                    o.replace = None;
                    break;
                }
                b'l' => {
                    o.max_lines = Some(if rest.is_empty() {
                        1
                    } else {
                        try_!(number("-l", rest, 1))
                    });
                    o.max_args = None;
                    o.replace = None;
                    break;
                }
                b'n' => {
                    let v = value!(rest, "n");
                    o.max_args = Some(try_!(number("-n", &v, 1)));
                    o.max_lines = None;
                    break;
                }
                b'P' => {
                    let v = value!(rest, "P");
                    o.max_procs = try_!(number("-P", &v, 0));
                    break;
                }
                b's' => {
                    let v = value!(rest, "s");
                    o.max_chars = try_!(number("-s", &v, 1));
                    break;
                }
                // `-p` (prompting) and BSD's -J, -R, -S are not built in.
                other => return Parse::FallThrough(format!("-{}", other as char)),
            }
            j += 1;
        }
        i += 1;
    }
    let command = if i < args.len() {
        args[i..].to_vec()
    } else {
        vec![b"echo".to_vec()]
    };
    Parse::Ok(o, command)
}

/// Reads the input as items, by lines where `-L` and `-I` need them.
struct Reader {
    buf: Vec<u8>,
    pos: usize,
    eof: bool,
    /// The `-E` word was seen: no more input.
    ended: bool,
}

/// One logical line's items (`-L`, `-I`), or one item.
enum Got {
    Item(Vec<u8>),
    /// The end of a logical line (only reported in line modes).
    LineEnd,
    End,
    Error(String),
}

impl Reader {
    async fn fill(&mut self, ctx: &mut UtilCtx<'_>, input: &common::Input) -> bool {
        if self.eof {
            return false;
        }
        if self.pos > 0 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        let mut chunk = vec![0u8; 16 * 1024];
        match input.read(ctx, &mut chunk).await {
            Ok(0) | Err(_) => {
                self.eof = true;
                false
            }
            Ok(n) => {
                self.buf.extend_from_slice(&chunk[..n]);
                true
            }
        }
    }

    async fn byte(&mut self, ctx: &mut UtilCtx<'_>, input: &common::Input) -> Option<u8> {
        if self.pos >= self.buf.len() && !self.fill(ctx, input).await {
            return None;
        }
        let b = self.buf[self.pos];
        self.pos += 1;
        Some(b)
    }

    /// The next item split on `delim` (`-0`, `-d`): literal bytes. A trailing delimiter
    /// ends the last item; it does not start an empty one.
    async fn next_delimited(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        input: &common::Input,
        delim: u8,
    ) -> Got {
        let mut item = Vec::new();
        let mut any = false;
        loop {
            match self.byte(ctx, input).await {
                None => {
                    return if any { Got::Item(item) } else { Got::End };
                }
                Some(b) if b == delim => return Got::Item(item),
                Some(b) => {
                    any = true;
                    item.push(b);
                }
            }
        }
    }

    /// The next item of the default syntax: blanks and newlines separate items;
    /// `'…'`, `"…"` and `\x` quote. With `lines`, the end of a logical line (a newline
    /// not after a blank) is reported; with `whole_line` (`-I`), only newlines separate
    /// items and leading blanks are dropped.
    async fn next_word(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        input: &common::Input,
        lines: bool,
        whole_line: bool,
    ) -> Got {
        let mut item = Vec::new();
        let mut started = false;
        let mut quote: Option<u8> = None;
        loop {
            let Some(b) = self.byte(ctx, input).await else {
                if let Some(q) = quote {
                    let kind = if q == b'\'' { "single" } else { "double" };
                    return Got::Error(format!(
                        "unmatched {kind} quote; by default quotes are special to xargs unless you use the -0 option"
                    ));
                }
                return if started { Got::Item(item) } else { Got::End };
            };
            if let Some(q) = quote {
                if b == q {
                    quote = None;
                } else if b == b'\n' {
                    let kind = if q == b'\'' { "single" } else { "double" };
                    return Got::Error(format!(
                        "unmatched {kind} quote; by default quotes are special to xargs unless you use the -0 option"
                    ));
                } else {
                    item.push(b);
                }
                continue;
            }
            match b {
                b'\n' => {
                    if started {
                        // The newline ends the line too: it is reported after the item.
                        if lines {
                            self.pos -= 1;
                            // Re-read the newline as a line end next time.
                            return Got::Item(item);
                        }
                        return Got::Item(item);
                    }
                    if lines {
                        return Got::LineEnd;
                    }
                }
                b' ' | b'\t' if !whole_line => {
                    if started {
                        // A blank before the newline continues the logical line (-L).
                        if lines && self.peek_newline(ctx, input).await {
                            self.pos += 1;
                        }
                        return Got::Item(item);
                    }
                }
                b' ' | b'\t' if !started => {}
                b'\'' | b'"' => {
                    quote = Some(b);
                    started = true;
                }
                b'\\' => {
                    started = true;
                    if let Some(c) = self.byte(ctx, input).await {
                        item.push(c);
                    }
                }
                _ => {
                    started = true;
                    item.push(b);
                }
            }
        }
    }

    async fn peek_newline(&mut self, ctx: &mut UtilCtx<'_>, input: &common::Input) -> bool {
        if self.pos >= self.buf.len() && !self.fill(ctx, input).await {
            return false;
        }
        self.buf.get(self.pos) == Some(&b'\n')
    }
}

/// The running commands and the status so far.
struct Runner {
    children: Vec<UtilChild>,
    max: usize,
    /// GNU's summary: 0, or 123 once a command failed with 1–125.
    status: i32,
    /// A command that ended everything: 124, 125, 126, 127.
    fatal: Option<i32>,
    stdin: Option<Vec<u8>>,
}

impl Runner {
    /// Records a finished command's status; `Some` ends xargs with it.
    async fn record(&mut self, ctx: &mut UtilCtx<'_>, name: &str, st: i32) {
        match st {
            0 => {}
            1..=125 => self.status = 123,
            126 | 127 => self.fatal = Some(st),
            255 => {
                ctx.error(&format!("{name}: exited with status 255; aborting"))
                    .await;
                self.fatal = Some(124);
            }
            _ => {
                ctx.error(&format!("{name}: terminated by signal {}", st - 128))
                    .await;
                self.fatal = Some(125);
            }
        }
    }

    /// Waits until a slot is free (`-P`), or every command ended (`max` 0).
    async fn wait_for(&mut self, ctx: &mut UtilCtx<'_>, names: &mut Vec<String>, keep: usize) {
        while self.children.len() > keep && self.fatal.is_none() {
            let (i, st) = std::future::poll_fn(|cx| {
                for (i, c) in self.children.iter_mut().enumerate() {
                    if let Poll::Ready(st) = c.poll_wait(cx) {
                        return Poll::Ready((i, st));
                    }
                }
                Poll::Pending
            })
            .await;
            self.children.swap_remove(i);
            let name = names.swap_remove(i);
            self.record(ctx, &name, st).await;
        }
    }
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let (o, command) = match parse(&argv[1..]) {
        Parse::Ok(o, c) => (o, c),
        Parse::FallThrough(opt) => {
            if let Some(st) = ctx.run_program(argv.to_vec()).await {
                return st;
            }
            ctx.error(&format!("unsupported option '{opt}' (psh built-in xargs)"))
                .await;
            return 2;
        }
        Parse::Error(msg) => {
            ctx.error(&msg).await;
            ctx.error_raw(b"Try 'xargs --help' for more information.\n")
                .await;
            return 1;
        }
    };
    let input = match &o.arg_file {
        Some(f) => match common::Input::open(ctx, f).await {
            Ok(i) => i,
            Err(e) => {
                let msg = format!(
                    "cannot open input file {}: {}",
                    common::quote(ctx, f),
                    psh_engine::host::error_text(&e)
                );
                ctx.error(&msg).await;
                return 1;
            }
        },
        None => common::Input::Stdin,
    };
    if o.open_tty
        && let Err(e) = ctx
            .open(b"/dev/tty", psh_engine::host::OpenOptions::read())
            .await
    {
        let msg = format!("/dev/tty: {}", psh_engine::host::error_text(&e));
        ctx.error(&msg).await;
        return 1;
    }
    let base: usize = command.iter().map(|a| a.len() + 1).sum();
    if base > o.max_chars {
        ctx.error("argument line too long").await;
        return 1;
    }
    let mut runner = Runner {
        children: Vec::new(),
        max: o.max_procs,
        status: 0,
        fatal: None,
        // The commands read /dev/null (GNU), or the terminal; with -a, xargs's stdin.
        stdin: if o.open_tty {
            Some(b"/dev/tty".to_vec())
        } else if o.arg_file.is_none() {
            Some(b"/dev/null".to_vec())
        } else {
            None
        },
    };
    let mut names: Vec<String> = Vec::new();
    let mut reader = Reader {
        buf: Vec::new(),
        pos: 0,
        eof: false,
        ended: false,
    };
    let lines_mode = o.max_lines.is_some();
    let replace_mode = o.replace.is_some();
    let mut args: Vec<Vec<u8>> = Vec::new();
    let mut size = base;
    let mut lines = 0usize;
    loop {
        ctx.check().await;
        if runner.fatal.is_some() {
            break;
        }
        let got = if reader.ended {
            Got::End
        } else if let Some(d) = o.delim {
            match reader.next_delimited(ctx, &input, d).await {
                Got::Item(i) if replace_mode || lines_mode => {
                    // With a delimiter, every item is a line of its own.
                    args_push_line(&mut args, i, &mut size);
                    lines += 1;
                    Got::LineEnd
                }
                g => g,
            }
        } else {
            reader
                .next_word(ctx, &input, lines_mode || replace_mode, replace_mode)
                .await
        };
        let flush = match got {
            Got::Error(msg) => {
                runner.wait_for(ctx, &mut names, 0).await;
                ctx.error(&msg).await;
                return 1;
            }
            Got::End => {
                if !args.is_empty() {
                    launch(ctx, &o, &command, &mut args, &mut runner, &mut names).await;
                }
                break;
            }
            Got::Item(item) => {
                if o.eof.as_deref() == Some(&item[..]) {
                    reader.ended = true;
                    continue;
                }
                if replace_mode {
                    // -I: the whole (trimmed) line is one argument.
                    args.push(item);
                    lines += 1;
                    true
                } else {
                    let len = item.len() + 1;
                    if !args.is_empty() && size + len > o.max_chars {
                        launch(ctx, &o, &command, &mut args, &mut runner, &mut names).await;
                        size = base;
                        lines = 0;
                    }
                    if base + len > o.max_chars {
                        ctx.error("argument line too long").await;
                        runner.wait_for(ctx, &mut names, 0).await;
                        return 1;
                    }
                    args.push(item);
                    size += len;
                    o.max_args.is_some_and(|n| args.len() >= n)
                }
            }
            Got::LineEnd => {
                if !replace_mode && o.delim.is_none() {
                    lines += 1;
                }
                (replace_mode && !args.is_empty())
                    || o.max_lines.is_some_and(|n| lines >= n && !args.is_empty())
            }
        };
        if flush {
            if o.exit && o.max_args.is_some() && size > o.max_chars {
                ctx.error("argument line too long").await;
                runner.wait_for(ctx, &mut names, 0).await;
                return 1;
            }
            launch(ctx, &o, &command, &mut args, &mut runner, &mut names).await;
            size = base;
            lines = 0;
        }
    }
    runner.wait_for(ctx, &mut names, 0).await;
    runner.fatal.unwrap_or(runner.status)
}

fn args_push_line(args: &mut Vec<Vec<u8>>, item: Vec<u8>, size: &mut usize) {
    *size += item.len() + 1;
    args.push(item);
}

/// Starts the command with the collected arguments (BSD rule: never with none, D1170).
async fn launch(
    ctx: &mut UtilCtx<'_>,
    o: &Opts,
    command: &[Vec<u8>],
    args: &mut Vec<Vec<u8>>,
    runner: &mut Runner,
    names: &mut Vec<String>,
) {
    let items = std::mem::take(args);
    let argv: Vec<Vec<u8>> = match &o.replace {
        Some(r) => {
            // One command per line, the line in place of every `R` in the arguments.
            let line = items.into_iter().next().unwrap_or_default();
            command.iter().map(|a| replace_all(a, r, &line)).collect()
        }
        None => command.iter().cloned().chain(items).collect(),
    };
    if o.verbose {
        let mut text = argv
            .iter()
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        text.push('\n');
        ctx.error_raw(text.as_bytes()).await;
    }
    let keep = if runner.max == 0 {
        usize::MAX
    } else {
        runner.max - 1
    };
    runner.wait_for(ctx, names, keep).await;
    if runner.fatal.is_some() {
        return;
    }
    let name = String::from_utf8_lossy(&argv[0]).into_owned();
    let env = SpawnEnv {
        stdin: runner.stdin.clone(),
        ..SpawnEnv::default()
    };
    let mut child = ctx.spawn(argv, env).await;
    if runner.max == 1 {
        let st = child.wait().await;
        runner.record(ctx, &name, st).await;
    } else {
        runner.children.push(child);
        names.push(name);
    }
}

fn replace_all(arg: &[u8], pat: &[u8], with: &[u8]) -> Vec<u8> {
    if pat.is_empty() {
        return arg.to_vec();
    }
    let mut out = Vec::with_capacity(arg.len());
    let mut i = 0;
    while i < arg.len() {
        if arg[i..].starts_with(pat) {
            out.extend_from_slice(with);
            i += pat.len();
        } else {
            out.push(arg[i]);
            i += 1;
        }
    }
    out
}
