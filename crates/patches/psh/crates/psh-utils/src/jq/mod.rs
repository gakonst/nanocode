//! `jq` on psh's own jq interpreter (lexer, parser, compiler and lazy evaluator in
//! `lex.rs`, `parse.rs`, `compile.rs` and `eval.rs`, which parse and run programs
//! as jaq-core 3.1.2 did, D028) with jq 1.7's values, builtins, output and messages
//! (D1180–D1182). The command line is jq's (`-r -c -s -n -e -S -j -a -R
//! -C -M --tab --indent --arg --argjson --slurpfile --rawfile --args --jsonargs -f --seq
//! --stream --raw-output0`), exit statuses too (5 for an error, 4 for no output with
//! `-e`, 3 for a compile error, 2 for a usage or file error). It runs as a blocking
//! utility (the interpreter is synchronous): on a worker thread natively, inline
//! elsewhere.

mod compile;
mod eval;
mod funs;
mod iter;
mod json;
mod lex;
mod list;
mod map;
mod num;
mod parse;
mod path;
mod prep;
mod time;
mod val;

#[cfg(test)]
mod upstream;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{self, Read};

use psh_engine::util::{BlockingOptions, SyncCtx, UtilCtx};

use crate::opts::{Spec, both, flag, long};

use compile::{Filter, Undefined};
use eval::{Ctx, EVAL_DEPTH_MAX, Vars};
use funs::{Data, Env, Inputs};
use json::{Colors, Indent, Reader, Style};
use map::Map;
use val::Val;

/// jq's options as the classifier models them (`-f FILE`, `--arg NAME` taking the
/// next word); the parser below is jq's own (`--arg NAME VALUE`, the program file as
/// the first operand).
pub const SPEC: Spec = Spec {
    name: "jq",
    opts: &[
        both('r', "raw-output"),
        both('j', "join-output"),
        both('a', "ascii-output"),
        both('c', "compact-output"),
        both('s', "slurp"),
        both('n', "null-input"),
        both('e', "exit-status"),
        both('S', "sort-keys"),
        both('R', "raw-input"),
        both('C', "color-output"),
        both('M', "monochrome-output"),
        both('f', "from-file").value(),
        flag('L').value(),
        both('h', "help"),
        both('V', "version"),
        long("tab"),
        long("indent").value(),
        long("arg").value(),
        long("argjson").value(),
        long("slurpfile").value(),
        long("rawfile").value(),
        long("args"),
        long("jsonargs"),
        long("seq"),
        long("stream"),
        long("stream-errors"),
        long("raw-output0"),
        long("unbuffered"),
    ],
    posix_order: false,
};

const DEFS: &str = include_str!("defs.jq");
const USAGE_TAIL: &str = "Use jq --help for help with command-line options,\nor see the jq manpage, or online docs  at https://jqlang.github.io/jq\n";

/// The parsed command line.
#[derive(Debug, Default)]
struct Args {
    filter: Option<String>,
    from_file: bool,
    files: Vec<Vec<u8>>,
    raw_out: bool,
    join: bool,
    nul: bool,
    ascii: bool,
    compact: bool,
    tab: bool,
    indent: usize,
    slurp: bool,
    null_input: bool,
    exit_status: bool,
    sort_keys: bool,
    raw_in: bool,
    color: Option<bool>,
    seq: bool,
    stream: bool,
    /// `--arg`, `--argjson`, `--slurpfile`, `--rawfile`, in order.
    named: Vec<(String, NamedArg)>,
    positional: Vec<String>,
    positional_json: Vec<bool>,
    version: bool,
    help: bool,
    /// Filled in by the async front end.
    utc_offset: i64,
    zone: String,
    now: f64,
}

#[derive(Debug)]
enum NamedArg {
    Str(String),
    Json(String),
    SlurpFile(String),
    RawFile(String),
}

enum ArgError {
    /// An option psh does not implement: the real jq runs it.
    Unsupported(String),
    /// jq's message for a bad command line (status 2).
    Usage(String),
}

fn utf8(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// jq's command line: options anywhere, the first operand is the filter (or, with
/// `-f`, the filter's file), the others are input files, or with `--args`/`--jsonargs`
/// values for `$ARGS.positional`.
fn parse_args(argv: &[Vec<u8>]) -> Result<Args, ArgError> {
    let mut a = Args {
        indent: 2,
        ..Args::default()
    };
    let mut i = 0;
    let mut further_opts = true;
    // Where operands after the filter go: files, string args, JSON args.
    let mut rest_mode = 0u8;
    let take = |i: &mut usize, n: usize, name: &str| -> Result<Vec<String>, ArgError> {
        if *i + n >= argv.len() {
            let what = match name {
                "arg" | "argjson" | "slurpfile" | "rawfile" => format!(
                    "--{name} takes two parameters (e.g. --{name} varname {})",
                    if name == "arg" {
                        "value"
                    } else if name == "argjson" {
                        "text"
                    } else {
                        "filename"
                    }
                ),
                "indent" => "--indent takes one parameter".to_string(),
                _ => format!("-{name} takes a parameter"),
            };
            return Err(ArgError::Usage(what));
        }
        let vals = (1..=n).map(|k| utf8(&argv[*i + k])).collect();
        *i += n;
        Ok(vals)
    };
    while i < argv.len() {
        let arg = &argv[i];
        let text = utf8(arg);
        if further_opts && text == "--" {
            further_opts = false;
            i += 1;
            continue;
        }
        if !further_opts || !text.starts_with('-') || text == "-" {
            if a.filter.is_none() {
                a.filter = Some(text);
            } else {
                match rest_mode {
                    1 => {
                        a.positional.push(text);
                        a.positional_json.push(false);
                    }
                    2 => {
                        a.positional.push(text);
                        a.positional_json.push(true);
                    }
                    _ => a.files.push(arg.clone()),
                }
            }
            i += 1;
            continue;
        }
        if let Some(long) = text.strip_prefix("--") {
            match long {
                "raw-output" => a.raw_out = true,
                "join-output" => {
                    a.raw_out = true;
                    a.join = true;
                }
                "raw-output0" => {
                    a.raw_out = true;
                    a.nul = true;
                }
                "ascii-output" => a.ascii = true,
                "compact-output" => a.compact = true,
                "tab" => a.tab = true,
                "indent" => {
                    let v = take(&mut i, 1, "indent")?;
                    match v[0].parse::<i64>() {
                        Ok(n) if (-1..=7).contains(&n) => {
                            if n <= 0 {
                                a.compact = true;
                            }
                            a.indent = n.max(0) as usize;
                        }
                        _ => {
                            return Err(ArgError::Usage(
                                "--indent takes a number between -1 and 7".into(),
                            ));
                        }
                    }
                }
                "slurp" => a.slurp = true,
                "null-input" => a.null_input = true,
                "exit-status" => a.exit_status = true,
                "sort-keys" => a.sort_keys = true,
                "raw-input" => a.raw_in = true,
                "color-output" => a.color = Some(true),
                "monochrome-output" => a.color = Some(false),
                "from-file" => a.from_file = true,
                "seq" => a.seq = true,
                "stream" => a.stream = true,
                "unbuffered" => {}
                "args" => rest_mode = 1,
                "jsonargs" => rest_mode = 2,
                "arg" => {
                    let v = take(&mut i, 2, "arg")?;
                    a.named.push((v[0].clone(), NamedArg::Str(v[1].clone())));
                }
                "argjson" => {
                    let v = take(&mut i, 2, "argjson")?;
                    a.named.push((v[0].clone(), NamedArg::Json(v[1].clone())));
                }
                "slurpfile" => {
                    let v = take(&mut i, 2, "slurpfile")?;
                    a.named
                        .push((v[0].clone(), NamedArg::SlurpFile(v[1].clone())));
                }
                "rawfile" => {
                    let v = take(&mut i, 2, "rawfile")?;
                    a.named
                        .push((v[0].clone(), NamedArg::RawFile(v[1].clone())));
                }
                "help" => a.help = true,
                "version" => a.version = true,
                _ => return Err(ArgError::Unsupported(text)),
            }
            i += 1;
            continue;
        }
        // Short options, clustered (`-nr`).
        let shorts: Vec<char> = text[1..].chars().collect();
        for (k, c) in shorts.iter().enumerate() {
            match c {
                'r' => a.raw_out = true,
                'j' => {
                    a.raw_out = true;
                    a.join = true;
                }
                'a' => a.ascii = true,
                'c' => a.compact = true,
                's' => a.slurp = true,
                'n' => a.null_input = true,
                'e' => a.exit_status = true,
                'S' => a.sort_keys = true,
                'R' => a.raw_in = true,
                'C' => a.color = Some(true),
                'M' => a.color = Some(false),
                'f' => a.from_file = true,
                'h' => a.help = true,
                'V' => a.version = true,
                'L' => {
                    // The library path: modules are not supported, the value is ignored.
                    if k + 1 == shorts.len() {
                        take(&mut i, 1, "L")?;
                    }
                    break;
                }
                _ => return Err(ArgError::Unsupported(format!("-{c}"))),
            }
        }
        i += 1;
    }
    Ok(a)
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let mut args = match parse_args(&argv[1..]) {
        Ok(a) => a,
        Err(ArgError::Unsupported(opt)) => {
            if let Some(st) = ctx.run_program(argv.to_vec()).await {
                return st;
            }
            ctx.error(&format!("unsupported option '{opt}' (psh built-in jq)"))
                .await;
            return 2;
        }
        Err(ArgError::Usage(msg)) => {
            let text = format!("jq: {msg}\n{USAGE_TAIL}");
            ctx.error_raw(text.as_bytes()).await;
            return 2;
        }
    };
    if args.version {
        let _ = ctx.write(b"jq-1.7.1\n").await;
        return 0;
    }
    if args.help {
        let text = "Usage:\tjq [OPTIONS] FILTER [FILES...]\n\tjq [OPTIONS] --args FILTER [ARGUMENTS...]\n\tjq [OPTIONS] --jsonargs FILTER [JSON_VALUES...]\n\njq is a tool for processing JSON inputs, applying the given filter to\nits JSON text inputs and producing the filter's results as JSON on\nstandard output (psh's built-in jq).\n";
        let _ = ctx.write(text.as_bytes()).await;
        return 0;
    }
    if args.filter.is_none() {
        if ctx.is_terminal(0) || ctx.is_terminal(1) {
            let text = format!("Usage:\tjq [OPTIONS] FILTER [FILES...]\n{USAGE_TAIL}");
            ctx.error_raw(text.as_bytes()).await;
            return 2;
        }
        args.filter = Some(".".into());
    }
    let now = ctx.now();
    args.now = now.as_secs_f64();
    let secs = i64::try_from(now.as_secs()).unwrap_or(0);
    args.utc_offset = i64::from(ctx.utc_offset(secs));
    args.zone = zone_name(ctx.var(b"TZ"), args.utc_offset);
    if args.color.is_none() {
        let no_color = ctx.var(b"NO_COLOR").is_some_and(|v| !v.is_empty());
        args.color = Some(ctx.is_terminal(1) && !no_color);
    }
    let stdin = args.files.is_empty() && (!args.null_input || program_reads_input(&args));
    let opts = BlockingOptions { stdin };
    ctx.run_blocking(opts, args, body).await
}

/// Whether a `-n` program may read its inputs (only then is stdin read up front on the
/// inline backend).
fn program_reads_input(a: &Args) -> bool {
    a.from_file || a.filter.as_deref().is_some_and(|f| f.contains("input"))
}

/// `%Z` for `localtime`: the zone's abbreviation where `TZ` gives one plainly, else UTC
/// or the offset.
fn zone_name(tz: Option<&[u8]>, offset: i64) -> String {
    if let Some(tz) = tz {
        let t = utf8(tz);
        let name: String = t.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        if name.len() >= 3 {
            return name;
        }
    }
    if offset == 0 {
        "UTC".into()
    } else {
        let sign = if offset < 0 { '-' } else { '+' };
        format!("{sign}{:02}", offset.unsigned_abs() / 3600)
    }
}

/// The shared mutable state of a run: the utility's context, borrowed by the inputs
/// (stdin, files) and the output writer in turn.
type Shared<'s> = RefCell<&'s mut SyncCtx>;

/// stdin through the shared context.
struct CtxStdin<'s>(&'s Shared<'s>);

impl Read for CtxStdin<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.borrow_mut().read(buf)
    }
}

/// The inputs of a run: files (or stdin) in order, as JSON values, raw lines, one slurped
/// value, or `--stream` events.
struct Source<'s> {
    sync: &'s Shared<'s>,
    files: Vec<Vec<u8>>,
    next_file: usize,
    started: bool,
    reader: Option<Reader<Box<dyn Read + 's>>>,
    name: Val,
    raw: bool,
    slurp: bool,
    stream: bool,
    pending: VecDeque<Val>,
    slurped: bool,
    /// 2 after a file could not be opened.
    status: i32,
    any_read: bool,
    /// The line count of the last finished file.
    last_line: usize,
}

impl<'s> Source<'s> {
    /// Opens the next file (or stdin, once); `false` at the end.
    fn open_next(&mut self) -> bool {
        loop {
            if self.files.is_empty() {
                if self.started {
                    return false;
                }
                self.started = true;
                self.name = Val::str("<stdin>");
                self.reader = Some(Reader::new(Box::new(CtxStdin(self.sync))));
                return true;
            }
            if self.next_file >= self.files.len() {
                return false;
            }
            let f = self.files[self.next_file].clone();
            self.next_file += 1;
            let opened = if f == b"-" {
                self.name = Val::str("<stdin>");
                Ok(Box::new(CtxStdin(self.sync)) as Box<dyn Read + 's>)
            } else {
                self.sync
                    .borrow()
                    .open(&f)
                    .map(|r| Box::new(r) as Box<dyn Read + 's>)
            };
            match opened {
                Ok(r) => {
                    if f != b"-" {
                        self.name = Val::str(utf8(&f));
                    }
                    self.reader = Some(Reader::new(r));
                    return true;
                }
                Err(e) => {
                    let msg = format!(
                        "jq: error: Could not open file {}: {}\n",
                        utf8(&f),
                        psh_engine::host::error_text(&e)
                    );
                    self.sync.borrow_mut().error_raw(msg.as_bytes());
                    self.status = 2;
                }
            }
        }
    }

    /// The next value of the current files, JSON or raw.
    fn next_raw(&mut self) -> Option<Result<Val, String>> {
        loop {
            if self.reader.is_none() && !self.open_next() {
                return None;
            }
            let reader = self.reader.as_mut()?;
            let next = if self.raw {
                reader.next_line().map(|r| r.map(Val::str).map_err(|e| e.0))
            } else {
                reader.next_value().map(|r| r.map_err(|e| e.0))
            };
            match next {
                Some(r) => {
                    self.any_read = true;
                    return Some(r.map_err(|m| format!("\u{0}parse:{m}")));
                }
                None => {
                    self.last_line = reader.lines_read;
                    self.reader = None;
                }
            }
        }
    }
}

impl Inputs for Source<'_> {
    fn next_input(&mut self) -> Option<Result<Val, String>> {
        if let Some(v) = self.pending.pop_front() {
            return Some(Ok(v));
        }
        if self.slurp {
            if self.slurped {
                return None;
            }
            self.slurped = true;
            if self.raw {
                let mut all = String::new();
                while let Some(r) = self.next_raw_chunk() {
                    match r {
                        Ok(s) => all.push_str(&s),
                        Err(e) => return Some(Err(e)),
                    }
                }
                return Some(Ok(Val::str(all)));
            }
            let mut all = Vec::new();
            while let Some(r) = self.next_raw() {
                match r {
                    Ok(v) => all.push(v),
                    Err(e) => return Some(Err(e)),
                }
            }
            return Some(Ok(Val::arr(all)));
        }
        let v = self.next_raw()?;
        if self.stream {
            return match v {
                Ok(v) => {
                    stream_events(&v, &mut Vec::new(), true, &mut self.pending);
                    self.pending.pop_front().map(Ok)
                }
                Err(e) => Some(Err(e)),
            };
        }
        Some(v)
    }

    fn filename(&self) -> Val {
        self.name.clone()
    }

    fn line(&self) -> usize {
        self.reader
            .as_ref()
            .map_or(self.last_line, |r| r.lines_read)
    }
}

impl Source<'_> {
    /// `-R -s`: the whole text of each file in turn.
    fn next_raw_chunk(&mut self) -> Option<Result<String, String>> {
        if self.reader.is_none() && !self.open_next() {
            return None;
        }
        let reader = self.reader.take()?;
        Some(reader.rest().map_err(|e| e.0))
    }
}

/// `--stream`: jq's events for a value (`tostream`).
fn stream_events(v: &Val, path: &mut Vec<Val>, top: bool, out: &mut VecDeque<Val>) {
    let leaf = |path: &[Val], v: &Val| Val::arr(vec![Val::arr(path.to_vec()), v.clone()]);
    match v {
        Val::Arr(a) if !a.is_empty() => {
            for (i, x) in a.iter().enumerate() {
                path.push(Val::from(i));
                stream_events(x, path, false, out);
                path.pop();
            }
            let mut last = path.clone();
            last.push(Val::from(a.len() - 1));
            out.push_back(Val::arr(vec![Val::arr(last)]));
        }
        Val::Obj(o) if !o.is_empty() => {
            let mut last_key = None;
            for (k, x) in o.iter() {
                path.push(Val::Str(k.clone()));
                stream_events(x, path, false, out);
                path.pop();
                last_key = Some(k.clone());
            }
            let mut last = path.clone();
            if let Some(k) = last_key {
                last.push(Val::Str(k));
            }
            out.push_back(Val::arr(vec![Val::arr(last)]));
        }
        v => {
            out.push_back(leaf(path, v));
        }
    }
    let _ = top;
}

/// The message of a compile error, as jq words it: the line of the error, padded to
/// the error's column (where jq would put a caret).
fn compile_report(shown: &str, code: &str, errs: &[(String, Option<usize>)]) -> String {
    let mut out = String::new();
    for (msg, off) in errs {
        let off = off.unwrap_or(0).min(code.len());
        let line = 1 + code[..off].matches('\n').count();
        let start = code[..off].rfind('\n').map_or(0, |i| i + 1);
        let col = code[start..off].chars().count();
        let text = shown.lines().nth(line - 1).unwrap_or("");
        out.push_str(&format!(
            "jq: error: {msg} at <top-level>, line {line}:\n{text}{}\n",
            " ".repeat(col)
        ));
    }
    let n = errs.len();
    out.push_str(&format!(
        "jq: {n} compile error{}\n",
        if n == 1 { "" } else { "s" }
    ));
    out
}

/// Where `part` starts in `code`; `None` when it is not a slice of it (an error inside
/// psh's own definitions).
fn offset_of(code: &str, part: &str) -> Option<usize> {
    let start = code.as_ptr() as usize;
    let at = part.as_ptr() as usize;
    (at >= start && at <= start + code.len()).then(|| at - start)
}

fn syntax_error(what: &str) -> String {
    format!("syntax error, unexpected {what} (Unix shell quoting issues?)")
}

fn nesting_error() -> String {
    format!(
        "syntax error, the program nests deeper than {} levels",
        parse::NESTING_DEPTH_MAX
    )
}

/// The main module of a program, or its lexer, parser and module errors with where
/// each was found.
fn load(code: &str) -> Result<parse::Module<'_, parse::Term<'_>>, Vec<(String, Option<usize>)>> {
    let tokens = match lex::Lexer::new(code).lex() {
        Ok(tokens) => tokens,
        Err(errors) => {
            let mut list = Vec::new();
            for (expect, found) in errors {
                let token: String = found.chars().take(1).collect();
                let message = match expect {
                    lex::Expect::Depth => nesting_error(),
                    lex::Expect::Delim => syntax_error("end of file"),
                    lex::Expect::Character if token.is_empty() => syntax_error("end of file"),
                    lex::Expect::Character => syntax_error(&format!("INVALID_CHARACTER '{token}'")),
                };
                list.push((message, offset_of(code, found)));
            }
            return Err(list);
        }
    };
    let parsed = parse::Parser::new(&tokens).parse(|p| p.module(|p| p.term()));
    let module = match parsed {
        Ok(module) => module,
        Err(errors) => {
            let mut list = Vec::new();
            for (problem, found) in errors {
                let found = parse::found_text(found, code);
                let message = match problem {
                    parse::Problem::Depth => nesting_error(),
                    parse::Problem::Unexpected if found.is_empty() => syntax_error("end of file"),
                    parse::Problem::Unexpected => {
                        let token: String = found
                            .chars()
                            .take_while(|c| !c.is_whitespace())
                            .take(16)
                            .collect();
                        syntax_error(&format!("'{token}'"))
                    }
                };
                list.push((message, offset_of(code, found)));
            }
            return Err(list);
        }
    };
    let mut modules = Vec::new();
    for dep in &module.deps {
        if !dep.name.is_some_and(|name| name.starts_with('$')) {
            modules.push((
                format!("module {}: module loading not supported", dep.path),
                offset_of(code, dep.path),
            ));
        }
    }
    if modules.is_empty() {
        Ok(module)
    } else {
        Err(modules)
    }
}

/// Compiles a program with psh's definitions and natives and the given global variables
/// (`$name`); the error is jq's report.
fn compile(code: &str, names: &[String]) -> Result<Filter, String> {
    let original = code;
    let prepared = prep::prepare(code);
    let code = prepared.as_deref().unwrap_or(code);
    let defs_tokens = lex::Lexer::new(DEFS)
        .lex()
        .unwrap_or_else(|_| panic!("psh's jq definitions lex"));
    let defs = parse::Parser::new(&defs_tokens)
        .parse(|p| p.defs())
        .unwrap_or_else(|_| panic!("psh's jq definitions parse"));
    let module = match load(code) {
        Ok(module) => module,
        Err(list) => return Err(compile_report(original, code, &list)),
    };
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    match compile::compile(defs, module, funs::funs(), &names) {
        Ok(filter) => Ok(filter),
        Err(errors) => {
            let mut list = Vec::new();
            for (found, undefined) in errors {
                let message = match undefined {
                    Undefined::Filter(arity) => format!("{found}/{arity} is not defined"),
                    Undefined::Var => format!("{found} is not defined"),
                    Undefined::Label => format!("$*label-{found} is not defined"),
                    Undefined::Module => format!("module {found} is not defined"),
                };
                list.push((message, offset_of(code, found)));
            }
            Err(compile_report(original, code, &list))
        }
    }
}

/// The blocking body.
fn body(sync: &mut SyncCtx, args: Args) -> i32 {
    let code = match (&args.filter, args.from_file) {
        (Some(path), true) => {
            let mut text = String::new();
            match sync.open(path.as_bytes()) {
                Ok(mut f) => {
                    let mut b = Vec::new();
                    if f.read_to_end(&mut b).is_ok() {
                        text = String::from_utf8_lossy(&b).into_owned();
                    }
                }
                Err(e) => {
                    let msg = format!(
                        "jq: Could not open {path}: {}\n",
                        psh_engine::host::error_text(&e)
                    );
                    sync.error_raw(msg.as_bytes());
                    return 2;
                }
            }
            text
        }
        (Some(f), false) => f.clone(),
        (None, _) => ".".into(),
    };
    let mut files = args.files.clone();
    if args.from_file && args.filter.is_some() {
        // `-f FILE`: the operands after it are inputs; nothing to shift.
    }
    let _ = &mut files;

    // Global variables: named arguments, then $ENV and $ARGS.
    let mut names: Vec<String> = Vec::new();
    let mut values: Vec<Val> = Vec::new();
    let mut named = Map::new();
    for (name, arg) in &args.named {
        let v = match arg {
            NamedArg::Str(s) => Val::str(s.as_str()),
            NamedArg::Json(t) => match json::parse_all(t.as_bytes()) {
                Ok(v) if v.len() == 1 => v.into_iter().next().unwrap_or_default(),
                _ => {
                    let msg = format!("jq: invalid JSON text passed to --argjson\n{USAGE_TAIL}");
                    sync.error_raw(msg.as_bytes());
                    return 2;
                }
            },
            NamedArg::SlurpFile(path) | NamedArg::RawFile(path) => {
                let slurp = matches!(arg, NamedArg::SlurpFile(_));
                let opt = if slurp { "--slurpfile" } else { "--rawfile" };
                let mut b = Vec::new();
                let read = sync
                    .open(path.as_bytes())
                    .and_then(|mut f| f.read_to_end(&mut b));
                if let Err(e) = read {
                    let msg = format!(
                        "jq: Bad JSON in {opt} {name} {path}: Could not open {path}: {}\n",
                        psh_engine::host::error_text(&e)
                    );
                    sync.error_raw(msg.as_bytes());
                    return 2;
                }
                if slurp {
                    match json::parse_all(&b) {
                        Ok(v) => Val::arr(v),
                        Err(json::ParseError(m)) => {
                            let msg = format!("jq: Bad JSON in {opt} {name} {path}: {m}\n");
                            sync.error_raw(msg.as_bytes());
                            return 2;
                        }
                    }
                } else {
                    Val::str(String::from_utf8_lossy(&b).into_owned())
                }
            }
        };
        named.insert(name.as_str().into(), v.clone());
        names.push(format!("${name}"));
        values.push(v);
    }
    let mut positional = Vec::new();
    for (text, is_json) in args.positional.iter().zip(&args.positional_json) {
        if *is_json {
            match json::parse_all(text.as_bytes()) {
                Ok(v) if v.len() == 1 => positional.push(v.into_iter().next().unwrap_or_default()),
                _ => {
                    let msg = format!("jq: Invalid JSON text passed to --jsonargs\n{USAGE_TAIL}");
                    sync.error_raw(msg.as_bytes());
                    return 2;
                }
            }
        } else {
            positional.push(Val::str(text.as_str()));
        }
    }
    let env: Map = sync
        .env()
        .iter()
        .map(|(k, v)| (utf8(k).into(), Val::str(utf8(v))))
        .collect();
    let env = Val::obj(env);
    let mut args_obj = Map::new();
    args_obj.insert("positional".into(), Val::arr(positional));
    args_obj.insert("named".into(), Val::obj(named));
    names.push("$ENV".into());
    values.push(env.clone());
    names.push("$ARGS".into());
    values.push(Val::obj(args_obj));

    let filter = match compile(&code, &names) {
        Ok(f) => f,
        Err(report) => {
            sync.error_raw(report.as_bytes());
            return 3;
        }
    };

    let builtins: Val = funs::builtin_names(DEFS)
        .into_iter()
        .map(Val::from)
        .collect();
    let env_data = Env {
        env,
        stderr: RefCell::new(Vec::new()),
        utc_offset: args.utc_offset,
        zone: args.zone.clone(),
        now: args.now,
        builtins,
    };
    let style = Style {
        indent: if args.compact {
            None
        } else if args.tab {
            Some(Indent::Tab)
        } else {
            Some(Indent::Spaces(args.indent))
        },
        sort_keys: args.sort_keys,
        ascii: args.ascii,
        color: args
            .color
            .unwrap_or(false)
            .then(|| Colors::new(sync.var(b"JQ_COLORS").map(utf8).as_deref())),
    };

    let shared: Shared<'_> = RefCell::new(sync);
    let source = RefCell::new(Source {
        sync: &shared,
        files,
        next_file: 0,
        started: false,
        reader: None,
        name: Val::Null,
        raw: args.raw_in,
        slurp: args.slurp,
        stream: args.stream,
        pending: VecDeque::new(),
        slurped: false,
        status: 0,
        any_read: false,
        last_line: 0,
    });
    let data = Data {
        lut: &filter.lut,
        env: &env_data,
        inputs: &source,
    };
    let mut status = 0;
    let mut last: Option<bool> = None;
    let mut out = Vec::new();

    // One input through the filter: outputs written as they come.
    let mut run_one = |input: Val, last: &mut Option<bool>, status: &mut i32| -> Result<(), i32> {
        let ctx = Ctx::new(&data, Vars::new(values.iter().cloned()));
        for r in filter.id.run((ctx, input)) {
            flush_stderr(&shared, &env_data);
            match r {
                Ok(v) => {
                    *last = Some(v.as_bool());
                    out.clear();
                    if args.seq {
                        out.push(0x1e);
                    }
                    match (&v, args.raw_out) {
                        (Val::Str(s), true) => out.extend_from_slice(s.as_bytes()),
                        _ => json::write(&mut out, &v, &style, 0),
                    }
                    if args.nul {
                        out.push(0);
                    } else if !args.join {
                        out.push(b'\n');
                    }
                    let mut sync = shared.borrow_mut();
                    if sync.write(&out).is_err() || sync.cancelled() {
                        return Err(141);
                    }
                }
                Err(e) => match e.get_err() {
                    Ok(e) => {
                        let where_ = position(&source);
                        let v = e.into_val();
                        let text = match &v {
                            Val::Str(s) => {
                                let s = s.replace(
                                    "invalid path expression with input ",
                                    "Invalid path expression with result ",
                                );
                                format!("jq: error (at {where_}): {s}\n")
                            }
                            v => {
                                format!("jq: error (at {where_}) (not a string): {}\n", v.to_json())
                            }
                        };
                        shared.borrow_mut().error_raw(text.as_bytes());
                        *status = 5;
                        return Ok(());
                    }
                    Err(e) if e.is_depth() => {
                        let where_ = position(&source);
                        let text = format!(
                            "jq: error (at {where_}): the program nests deeper than {EVAL_DEPTH_MAX} levels as it runs\n"
                        );
                        shared.borrow_mut().error_raw(text.as_bytes());
                        *status = 5;
                        return Ok(());
                    }
                    Err(e) => {
                        let code = e.get_halt().unwrap_or(5);
                        flush_stderr(&shared, &env_data);
                        return Err(code);
                    }
                },
            }
        }
        flush_stderr(&shared, &env_data);
        Ok(())
    };

    let finish = |shared: &Shared<'_>| {
        let _ = shared.borrow_mut().flush();
    };
    if args.null_input {
        if let Err(code) = run_one(Val::Null, &mut last, &mut status) {
            finish(&shared);
            return code;
        }
    } else {
        loop {
            if shared.borrow().cancelled() {
                return 130;
            }
            let next = source.borrow_mut().next_input();
            let Some(next) = next else {
                break;
            };
            match next {
                Ok(v) => {
                    // As in jq, the status is the last input's: an error in an earlier
                    // one does not count.
                    status = 0;
                    if let Err(code) = run_one(v, &mut last, &mut status) {
                        finish(&shared);
                        return code;
                    }
                }
                Err(m) => {
                    let m = m.strip_prefix("\u{0}parse:").unwrap_or(&m).to_string();
                    let text = format!("jq: parse error: {m}\n");
                    shared.borrow_mut().error_raw(text.as_bytes());
                    status = 5;
                    break;
                }
            }
        }
    }
    finish(&shared);
    let file_status = source.borrow().status;
    if status == 0 && file_status != 0 {
        status = file_status;
    }
    if status != 0 {
        return status;
    }
    if args.exit_status {
        return match last {
            None => 4,
            Some(false) => 1,
            Some(true) => 0,
        };
    }
    0
}

/// `<stdin>:N`, `FILE:N`, or `<unknown>` before any input was read.
fn position(source: &RefCell<Source<'_>>) -> String {
    let s = source.borrow();
    if !s.any_read {
        return "<unknown>".into();
    }
    let name = match s.filename() {
        Val::Str(n) => n.to_string(),
        _ => "<unknown>".into(),
    };
    format!("{name}:{}", s.line())
}

fn flush_stderr(shared: &Shared<'_>, env: &Env) {
    let mut e = env.stderr.borrow_mut();
    if !e.is_empty() {
        let mut sync = shared.borrow_mut();
        let _ = sync.flush();
        sync.error_raw(&e);
        e.clear();
    }
}
