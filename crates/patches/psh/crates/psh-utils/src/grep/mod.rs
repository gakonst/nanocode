//! `grep`, `egrep`, `fgrep` (D1137, ARCH "Search"): GNU grep 3.11's command line,
//! output and exit rules on the search layer shared with `rg`: POSIX patterns through
//! `psh_regex`'s translator into `grep-regex`, `grep-searcher` with psh's own GNU sink,
//! and GNU's recursion on the walker seam (hidden files searched, no ignore files,
//! `-R` follows symbolic links). Written for psh after GNU grep's manual and its
//! observed behavior.
//!
//! The front end here parses the options and the patterns (reading `-f` files through
//! the utility's file system) and runs the search as a blocking utility (D1130):
//! [`search::run_sync`]. What psh leaves to the real grep (D1082): options it does not
//! know (`-T`, `--help`, `--version`, BSD's `-O`/`-S`/`-p`/`-J`...), `-P` patterns the
//! regex crate does not take, and unknown option values.

mod pattern;
mod search;
mod sink;

use globset::{GlobBuilder, GlobMatcher};
use psh_engine::host::{OpenOptions, error_text};
use psh_engine::util::{BlockingOptions, UtilCtx};

use crate::opts::{Opt, Spec, both, flag, long};
use pattern::{MatchOptions, PatternError};

const OPTS: &[Opt] = &[
    both('E', "extended-regexp"),
    both('F', "fixed-strings"),
    both('G', "basic-regexp"),
    both('P', "perl-regexp"),
    both('e', "regexp").value(),
    both('f', "file").value(),
    both('i', "ignore-case"),
    flag('y'),
    long("no-ignore-case"),
    both('w', "word-regexp"),
    both('x', "line-regexp"),
    both('z', "null-data"),
    both('s', "no-messages"),
    both('v', "invert-match"),
    both('m', "max-count").value(),
    both('b', "byte-offset"),
    both('n', "line-number"),
    long("line-buffered"),
    both('H', "with-filename"),
    both('h', "no-filename"),
    long("label").value(),
    both('o', "only-matching"),
    both('q', "quiet"),
    long("silent"),
    long("binary-files").value(),
    both('a', "text"),
    flag('I'),
    both('d', "directories").value(),
    both('D', "devices").value(),
    both('r', "recursive"),
    both('R', "dereference-recursive"),
    long("include").value(),
    long("exclude").value(),
    long("exclude-from").value(),
    long("exclude-dir").value(),
    both('L', "files-without-match"),
    both('l', "files-with-matches"),
    both('c', "count"),
    both('Z', "null"),
    both('B', "before-context").value(),
    both('A', "after-context").value(),
    both('C', "context").value(),
    long("group-separator").value(),
    long("no-group-separator"),
    long("color").optional(),
    long("colour").optional(),
    both('U', "binary"),
];

pub const SPEC: Spec = Spec {
    name: "grep",
    opts: OPTS,
    posix_order: false,
};

pub const EGREP_SPEC: Spec = Spec {
    name: "egrep",
    ..SPEC
};

pub const FGREP_SPEC: Spec = Spec {
    name: "fgrep",
    ..SPEC
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Syntax {
    Basic,
    Extended,
    Fixed,
    Perl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinaryFiles {
    /// Output stops at the first NUL; lines that are not valid in the locale are not
    /// printed; `grep: FILE: binary file matches` after.
    Binary,
    Text,
    /// A binary file does not match (`-I`).
    WithoutMatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dirs {
    Read,
    Skip,
    Recurse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum List {
    Matching,
    NonMatching,
}

/// GREP_COLORS' capabilities (SGR parameters; empty: not colored).
#[derive(Clone, Debug)]
pub(crate) struct Colors {
    pub ms: String,
    pub mc: String,
    pub sl: String,
    pub cx: String,
    pub fname: String,
    pub ln: String,
    pub bn: String,
    pub se: String,
    pub rv: bool,
    /// Without `\33[K` after each SGR sequence.
    pub ne: bool,
}

impl Default for Colors {
    fn default() -> Colors {
        Colors {
            ms: "01;31".into(),
            mc: "01;31".into(),
            sl: String::new(),
            cx: String::new(),
            fname: "35".into(),
            ln: "32".into(),
            bn: "32".into(),
            se: "36".into(),
            rv: false,
            ne: false,
        }
    }
}

impl Colors {
    /// `GREP_COLORS` (`ms=01;31:fn=35:ne`) over the defaults.
    fn parse(&mut self, spec: &[u8]) {
        for item in String::from_utf8_lossy(spec).split(':') {
            let (k, v) = match item.split_once('=') {
                Some((k, v)) => (k, Some(v.to_string())),
                None => (item, None),
            };
            match (k, v) {
                ("mt", Some(v)) => {
                    self.ms = v.clone();
                    self.mc = v;
                }
                ("ms", Some(v)) => self.ms = v,
                ("mc", Some(v)) => self.mc = v,
                ("sl", Some(v)) => self.sl = v,
                ("cx", Some(v)) => self.cx = v,
                ("fn", Some(v)) => self.fname = v,
                ("ln", Some(v)) => self.ln = v,
                ("bn", Some(v)) => self.bn = v,
                ("se", Some(v)) => self.se = v,
                ("rv", None) => self.rv = true,
                ("ne", None) => self.ne = true,
                _ => {}
            }
        }
    }
}

/// Everything the search needs, from the options.
pub(crate) struct Config {
    pub invert: bool,
    pub max_count: Option<u64>,
    pub byte_offset: bool,
    pub line_number: bool,
    pub line_buffered: bool,
    pub with_filename: Option<bool>,
    pub label: Vec<u8>,
    pub only_matching: bool,
    pub quiet: bool,
    pub discard: bool,
    pub no_messages: bool,
    pub binary: BinaryFiles,
    pub dirs: Dirs,
    pub skip_devices: bool,
    /// `-R`: follow symbolic links while recursing.
    pub deref: bool,
    /// `--include` (`true`) and `--exclude` globs, in command-line order.
    pub filters: Vec<(bool, GlobMatcher)>,
    pub exclude_dirs: Vec<GlobMatcher>,
    pub list: Option<List>,
    pub count: bool,
    /// `-Z`: a NUL after file names.
    pub null: bool,
    pub before: usize,
    pub after: usize,
    /// A context option was given (even 0): groups are separated.
    pub context: bool,
    pub group_separator: Option<Vec<u8>>,
    pub colors: Option<Colors>,
    pub null_data: bool,
    pub crlf: bool,
    pub utf8: bool,
    pub terminal: bool,
    /// The file operands (the pattern taken out).
    pub operands: Vec<Vec<u8>>,
    /// No patterns (`-f /dev/null`) and no `-v`: GNU selects nothing without reading
    /// (no errors, no counts; `-L` lists the operands).
    pub nothing: bool,
}

const USAGE: &str =
    "Usage: grep [OPTION]... PATTERNS [FILE]...\nTry 'grep --help' for more information.\n";

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let base = ctx
        .name()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("grep")
        .to_string();
    let (spec, mut syntax) = match base.as_str() {
        "egrep" => (&EGREP_SPEC, Syntax::Extended),
        "fgrep" => (&FGREP_SPEC, Syntax::Fixed),
        _ => (&SPEC, Syntax::Basic),
    };
    let args = rewrite_context(&argv[1..]);
    let p = match spec.parse(&args) {
        Ok(p) => p,
        Err(e) if e.is_unknown() => {
            return crate::common::option_error(ctx, spec, argv, &e).await;
        }
        Err(e) => return usage(ctx, Some(&e.to_string())).await,
    };

    let mut cfg = Config {
        invert: false,
        max_count: None,
        byte_offset: false,
        line_number: false,
        line_buffered: false,
        with_filename: None,
        label: b"(standard input)".to_vec(),
        only_matching: false,
        quiet: false,
        discard: false,
        no_messages: false,
        binary: BinaryFiles::Binary,
        dirs: Dirs::Read,
        skip_devices: false,
        deref: false,
        filters: Vec::new(),
        exclude_dirs: Vec::new(),
        list: None,
        count: false,
        null: false,
        before: 0,
        after: 0,
        context: false,
        group_separator: Some(b"--".to_vec()),
        colors: None,
        null_data: false,
        crlf: cfg!(windows),
        utf8: ctx.utf8(),
        terminal: ctx.is_terminal(1),
        operands: Vec::new(),
        nothing: false,
    };
    let (mut icase, mut word, mut line) = (false, false, false);
    let mut patterns: Vec<Vec<u8>> = Vec::new();
    let mut have_patterns = false;
    let mut color: Option<Option<&[u8]>> = None;
    let (mut a, mut b, mut c) = (None, None, None);
    for f in &p.opts {
        let key = spec.opts[f.index].key();
        let v = f.value.as_deref().unwrap_or_default();
        match key.as_str() {
            "extended-regexp" => syntax = Syntax::Extended,
            "fixed-strings" => syntax = Syntax::Fixed,
            "basic-regexp" => syntax = Syntax::Basic,
            "perl-regexp" => syntax = Syntax::Perl,
            "regexp" => {
                have_patterns = true;
                patterns.extend(v.split(|&b| b == b'\n').map(<[u8]>::to_vec));
            }
            "file" => {
                have_patterns = true;
                match read_input(ctx, v).await {
                    Ok(text) => patterns.extend(lines_of(&text)),
                    Err(e) => {
                        let msg = format!("{}: {}", String::from_utf8_lossy(v), error_text(&e));
                        return fail(ctx, &msg).await;
                    }
                }
            }
            "ignore-case" | "y" => icase = true,
            "no-ignore-case" => icase = false,
            "word-regexp" => word = true,
            "line-regexp" => line = true,
            "null-data" => cfg.null_data = true,
            "no-messages" => cfg.no_messages = true,
            "invert-match" => cfg.invert = true,
            "max-count" => match parse_number(v) {
                Some(Number::Value(n)) => cfg.max_count = Some(n),
                Some(Number::Negative) => cfg.max_count = None,
                None => return fail(ctx, "invalid max count").await,
            },
            "byte-offset" => cfg.byte_offset = true,
            "line-number" => cfg.line_number = true,
            "line-buffered" => cfg.line_buffered = true,
            "with-filename" => cfg.with_filename = Some(true),
            "no-filename" => cfg.with_filename = Some(false),
            "label" => cfg.label = v.to_vec(),
            "only-matching" => cfg.only_matching = true,
            "quiet" | "silent" => cfg.quiet = true,
            "binary-files" => {
                cfg.binary = match v {
                    b"binary" => BinaryFiles::Binary,
                    b"text" => BinaryFiles::Text,
                    b"without-match" => BinaryFiles::WithoutMatch,
                    _ => return fail(ctx, "unknown binary-files type").await,
                }
            }
            "text" => cfg.binary = BinaryFiles::Text,
            "I" => cfg.binary = BinaryFiles::WithoutMatch,
            "directories" => {
                cfg.dirs = match v {
                    b"read" => Dirs::Read,
                    b"skip" => Dirs::Skip,
                    b"recurse" => Dirs::Recurse,
                    _ => {
                        return crate::textio::fall_through(ctx, argv, "--directories value").await;
                    }
                }
            }
            "devices" => {
                cfg.skip_devices = match v {
                    b"read" => false,
                    b"skip" => true,
                    _ => return crate::textio::fall_through(ctx, argv, "--devices value").await,
                }
            }
            "recursive" => {
                cfg.dirs = Dirs::Recurse;
                cfg.deref = false;
            }
            "dereference-recursive" => {
                cfg.dirs = Dirs::Recurse;
                cfg.deref = true;
            }
            "include" | "exclude" => cfg.filters.push((key == "include", fnmatch(v))),
            "exclude-from" => match read_input(ctx, v).await {
                Ok(text) => {
                    for g in lines_of(&text) {
                        cfg.filters.push((false, fnmatch(&g)));
                    }
                }
                Err(e) => {
                    let msg = format!("{}: {}", String::from_utf8_lossy(v), error_text(&e));
                    return fail(ctx, &msg).await;
                }
            },
            "exclude-dir" => cfg.exclude_dirs.push(fnmatch(v)),
            "files-without-match" => cfg.list = Some(List::NonMatching),
            "files-with-matches" => cfg.list = Some(List::Matching),
            "count" => cfg.count = true,
            "null" => cfg.null = true,
            "before-context" | "after-context" | "context" => {
                let Some(Number::Value(n)) = parse_number(v) else {
                    let msg = format!(
                        "{}: invalid context length argument",
                        String::from_utf8_lossy(v)
                    );
                    return fail(ctx, &msg).await;
                };
                let n = usize::try_from(n).unwrap_or(usize::MAX);
                match key.as_str() {
                    "before-context" => b = Some(n),
                    "after-context" => a = Some(n),
                    _ => c = Some(n),
                }
            }
            "group-separator" => cfg.group_separator = Some(v.to_vec()),
            "no-group-separator" => cfg.group_separator = None,
            "color" | "colour" => color = Some(f.value.as_deref()),
            "binary" => cfg.crlf = false,
            _ => {}
        }
    }
    cfg.context = a.is_some() || b.is_some() || c.is_some();
    cfg.after = a.or(c).unwrap_or(0);
    cfg.before = b.or(c).unwrap_or(0);
    if cfg.null_data {
        cfg.crlf = false;
    }

    let mut operands = p.operands;
    if !have_patterns {
        if operands.is_empty() {
            return usage(ctx, None).await;
        }
        let first = operands.remove(0);
        patterns.extend(first.split(|&b| b == b'\n').map(<[u8]>::to_vec));
    }
    cfg.operands = operands;
    cfg.nothing = patterns.is_empty() && !cfg.invert;

    let on = match color {
        None => false,
        Some(None) => auto_color(ctx),
        Some(Some(w)) => match w {
            b"always" | b"yes" | b"force" => true,
            b"never" | b"no" | b"none" => false,
            b"auto" | b"tty" | b"if-tty" => auto_color(ctx),
            _ => return crate::textio::fall_through(ctx, argv, "--color value").await,
        },
    };
    if on {
        let mut colors = Colors::default();
        if let Some(old) = ctx.var(b"GREP_COLOR").filter(|v| !v.is_empty()) {
            let old = String::from_utf8_lossy(old).into_owned();
            ctx.error_raw(
                format!(
                    "grep: warning: GREP_COLOR='{old}' is deprecated; use GREP_COLORS='mt={old}'\n"
                )
                .as_bytes(),
            )
            .await;
            colors.ms = old.clone();
            colors.mc = old;
        }
        if let Some(spec) = ctx.var(b"GREP_COLORS").map(<[u8]>::to_vec) {
            colors.parse(&spec);
        }
        cfg.colors = Some(colors);
    }

    if matches!(syntax, Syntax::Basic | Syntax::Extended) {
        let mut warnings = Vec::new();
        for p in &mut patterns {
            match pattern::diagnose(p, syntax == Syntax::Extended, &mut warnings) {
                Ok(fixed) => *p = fixed,
                Err(msg) => return fail(ctx, &msg).await,
            }
        }
        for w in warnings {
            ctx.error_raw(format!("grep: warning: {w}\n").as_bytes())
                .await;
        }
    }
    cfg.discard = ctx.discards(1);
    let shows_lines = !cfg.quiet && !cfg.discard && cfg.list.is_none() && !cfg.count;
    let opts = MatchOptions {
        syntax,
        icase,
        word,
        line,
        null_data: cfg.null_data,
        crlf: cfg.crlf,
        utf8: cfg.utf8,
        spans: shows_lines && (cfg.only_matching || cfg.colors.is_some()),
    };
    let matchers = match pattern::build(&patterns, opts) {
        Ok(m) => m,
        Err(PatternError::Message(msg)) => return fail(ctx, &msg).await,
        Err(PatternError::Unsupported) => {
            return crate::textio::fall_through(ctx, argv, "-P pattern").await;
        }
    };
    if cfg.max_count == Some(0) {
        return 1;
    }
    let stdin = cfg.operands.iter().any(|o| o == b"-")
        || (cfg.operands.is_empty() && cfg.dirs != Dirs::Recurse);
    ctx.run_blocking(BlockingOptions { stdin }, (cfg, matchers), search::run_sync)
        .await
}

/// GNU's `-NUM` (context): every digit run in a cluster of short options, as
/// `--context=NUM` (`-5`, `-n5`, `-5n`). Values of options are left alone.
fn rewrite_context(args: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if a == b"--" {
            out.extend(args[i - 1..].iter().cloned());
            break;
        }
        if let Some(name) = a.strip_prefix(b"--") {
            out.push(a.clone());
            let takes = OPTS
                .iter()
                .find(|o| o.long.is_some_and(|l| l.as_bytes() == name))
                .is_some_and(|o| o.arg == crate::opts::Arg::Required);
            if takes && i < args.len() {
                out.push(args[i].clone());
                i += 1;
            }
            continue;
        }
        if a.len() < 2 || a[0] != b'-' {
            out.push(a.clone());
            continue;
        }
        let mut rest = Vec::new();
        let mut digits = Vec::new();
        let mut needs_value = false;
        let mut j = 1;
        while j < a.len() {
            let ch = a[j];
            if ch.is_ascii_digit() {
                digits.push(ch);
                j += 1;
                continue;
            }
            rest.push(ch);
            j += 1;
            let takes = OPTS
                .iter()
                .any(|o| o.short == Some(ch as char) && o.arg == crate::opts::Arg::Required);
            if takes {
                rest.extend_from_slice(&a[j..]);
                needs_value = j >= a.len();
                break;
            }
        }
        if digits.is_empty() {
            out.push(a.clone());
        } else {
            if !rest.is_empty() {
                let mut opt = vec![b'-'];
                opt.extend_from_slice(&rest);
                out.push(opt);
            }
            let mut ctx = b"--context=".to_vec();
            ctx.extend_from_slice(&digits);
            out.push(ctx);
        }
        if needs_value && i < args.len() {
            out.push(args[i].clone());
            i += 1;
        }
    }
    out
}

enum Number {
    Value(u64),
    Negative,
}

/// A decimal count (saturating, as GNU does for huge values); negative counts are
/// "no limit" for `-m`.
fn parse_number(v: &[u8]) -> Option<Number> {
    let s = std::str::from_utf8(v).ok()?.trim_start();
    if let Some(n) = s.strip_prefix('-') {
        return (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            .then_some(Number::Negative);
    }
    let s = s.strip_prefix('+').unwrap_or(s);
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(Number::Value(s.parse().unwrap_or(u64::MAX)))
}

/// A file's lines (patterns, globs): the last newline ends a line, it does not start
/// an empty one; an empty file has none.
fn lines_of(text: &[u8]) -> Vec<Vec<u8>> {
    if text.is_empty() {
        return Vec::new();
    }
    let text = text.strip_suffix(b"\n").unwrap_or(text);
    text.split(|&b| b == b'\n').map(<[u8]>::to_vec).collect()
}

/// A file's contents, or stdin's for `-`.
async fn read_input(ctx: &mut UtilCtx<'_>, path: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    if path == b"-" {
        ctx.read_to_end(&mut out).await?;
        return Ok(out);
    }
    if let Ok(m) = ctx.stat(path, true).await
        && m.kind == psh_engine::host::FileKind::Dir
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::IsADirectory,
            "Is a directory",
        ));
    }
    let f = ctx.open(path, OpenOptions::read()).await?;
    f.read_to_end(&mut out).await?;
    Ok(out)
}

/// A GNU glob (fnmatch without `FNM_PATHNAME`: `*` matches `/`; no braces) for
/// `--include`, `--exclude` and `--exclude-dir`.
fn fnmatch(glob: &[u8]) -> GlobMatcher {
    let g = String::from_utf8_lossy(glob);
    let mut s = String::with_capacity(g.len());
    let mut in_bracket = false;
    let mut chars = g.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if !in_bracket => {
                s.push('\\');
                if let Some(n) = chars.next() {
                    s.push(n);
                }
            }
            '[' if !in_bracket => {
                in_bracket = true;
                s.push('[');
                if let Some(&n) = chars.peek()
                    && (n == '!' || n == '^')
                {
                    s.push('!');
                    chars.next();
                }
                if chars.peek() == Some(&']') {
                    s.push(']');
                    chars.next();
                }
            }
            ']' if in_bracket => {
                in_bracket = false;
                s.push(']');
            }
            '{' | '}' if !in_bracket => {
                s.push('\\');
                s.push(c);
            }
            _ => s.push(c),
        }
    }
    let build = |p: &str| {
        GlobBuilder::new(p)
            .literal_separator(false)
            .backslash_escape(true)
            .build()
            .map(|g| g.compile_matcher())
    };
    build(&s).unwrap_or_else(|_| {
        // fnmatch takes what globset rejects (an unclosed `[`) literally.
        build(&globset::escape(&g)).expect("an escaped glob builds")
    })
}

fn auto_color(ctx: &UtilCtx<'_>) -> bool {
    ctx.is_terminal(1)
        && ctx
            .var(b"TERM")
            .is_some_and(|t| !t.is_empty() && t != b"dumb")
}

/// `grep: MSG` and status 2.
async fn fail(ctx: &mut UtilCtx<'_>, msg: &str) -> i32 {
    ctx.error_raw(format!("grep: {msg}\n").as_bytes()).await;
    2
}

/// A usage error: the message, GNU's usage lines and status 2.
async fn usage(ctx: &mut UtilCtx<'_>, msg: Option<&str>) -> i32 {
    let mut text = String::new();
    if let Some(m) = msg {
        text.push_str(&format!("grep: {m}\n"));
    }
    text.push_str(USAGE);
    ctx.error_raw(text.as_bytes()).await;
    2
}

#[cfg(test)]
mod tests;
