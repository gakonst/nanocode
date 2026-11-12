//! `diff [OPTION]... FILE1 FILE2`: GNU diff's output formats (normal by default,
//! `-u`/`-U N`, `-c`/`-C N`, `-y`, `-q`), comparison options (`-i -w -b -B -I RE
//! --strip-trailing-cr -a`), directories (`-r -N -x -X`, `Only in`, `Common
//! subdirectories`), `--label`, `-p`/`-F RE`, `--color`. Status 0 same, 1 different, 2
//! trouble. Written for psh after the GNU diffutils manual; the algorithm is
//! `diff_algo`. BSD diff (macOS) prints the same for these options, except where noted
//! in the cases.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io;

use psh_engine::host::FileKind;
use psh_engine::util::UtilCtx;

use crate::common::{self, Input};
use crate::diff_algo::{self, Change};
use crate::opts::{Parsed, Spec, both, flag, long};
use crate::timefmt;

pub const SPEC: Spec = Spec {
    name: "diff",
    opts: &[
        flag('u'),
        long("unified").optional(),
        flag('U').value(),
        flag('c'),
        long("context").optional(),
        flag('C').value(),
        long("normal"),
        both('q', "brief"),
        both('s', "report-identical-files"),
        both('y', "side-by-side"),
        both('W', "width").value(),
        long("suppress-common-lines"),
        long("left-column"),
        both('r', "recursive"),
        both('N', "new-file"),
        both('x', "exclude").value(),
        both('X', "exclude-from").value(),
        both('I', "ignore-matching-lines").value(),
        both('i', "ignore-case"),
        both('w', "ignore-all-space"),
        both('b', "ignore-space-change"),
        both('B', "ignore-blank-lines"),
        long("strip-trailing-cr"),
        both('a', "text"),
        both('L', "label").value(),
        long("color").optional(),
        both('d', "minimal"),
        both('p', "show-c-function"),
        both('F', "show-function-line").value(),
    ],
    posix_order: false,
};

#[derive(Clone, Copy, PartialEq)]
enum Style {
    Normal,
    Unified(usize),
    Context(usize),
    Side,
}

struct Opts {
    style: Style,
    brief: bool,
    report_same: bool,
    width: usize,
    suppress_common: bool,
    left_column: bool,
    recursive: bool,
    new_file: bool,
    exclude: Vec<crate::search::glob::GlobMatcher>,
    ignore_re: Vec<psh_pattern::bytes::Regex>,
    icase: bool,
    all_space: bool,
    space_change: bool,
    blank_lines: bool,
    strip_cr: bool,
    text: bool,
    labels: Vec<Vec<u8>>,
    color: bool,
    function_re: Option<psh_pattern::bytes::Regex>,
    /// The options as written, for the `diff -r a/x b/x` lines.
    header_opts: Vec<u8>,
}

/// diffutils' usage error: status 2.
async fn usage(ctx: &mut UtilCtx<'_>, msg: &str) -> i32 {
    ctx.error(msg).await;
    ctx.error("Try 'diff --help' for more information.").await;
    2
}

/// A word as GNU's `diff -r` lines show it: single-quoted unless every byte is safe.
fn shell_word(w: &[u8]) -> Vec<u8> {
    let safe = !w.is_empty()
        && w.iter()
            .all(|&b| b.is_ascii_alphanumeric() || b"_./-+,:@%".contains(&b));
    if safe {
        return w.to_vec();
    }
    let mut out = vec![b'\''];
    for &b in w {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    out
}

fn context_arg(v: Option<&[u8]>) -> Option<usize> {
    match v {
        None => Some(3),
        Some(v) => std::str::from_utf8(v).ok()?.parse().ok(),
    }
}

fn compile_bre(pat: &[u8]) -> Option<psh_pattern::bytes::Regex> {
    let t = crate::search::posix::for_lines(pat, false).ok()?;
    psh_pattern::bytes::Regex::new(&t.pattern?).ok()
}

async fn options(ctx: &mut UtilCtx<'_>, p: &Parsed, argv: &[Vec<u8>]) -> Result<Opts, i32> {
    let mut o = Opts {
        style: Style::Normal,
        brief: false,
        report_same: false,
        width: 130,
        suppress_common: false,
        left_column: false,
        recursive: false,
        new_file: false,
        exclude: Vec::new(),
        ignore_re: Vec::new(),
        icase: false,
        all_space: false,
        space_change: false,
        blank_lines: false,
        strip_cr: false,
        text: false,
        labels: Vec::new(),
        color: false,
        function_re: None,
        header_opts: Vec::new(),
    };
    let mut patterns: Vec<Vec<u8>> = Vec::new();
    for (k, v) in p.iter(&SPEC) {
        let bad_context = |v: Option<&[u8]>| {
            let q = String::from_utf8_lossy(v.unwrap_or_default()).into_owned();
            format!("invalid context length '{q}'")
        };
        match k.as_str() {
            "u" => o.style = Style::Unified(3),
            "unified" | "U" => match context_arg(v) {
                Some(n) => o.style = Style::Unified(n),
                None => return Err(usage(ctx, &bad_context(v)).await),
            },
            "c" => o.style = Style::Context(3),
            "context" | "C" => match context_arg(v) {
                Some(n) => o.style = Style::Context(n),
                None => return Err(usage(ctx, &bad_context(v)).await),
            },
            "normal" => o.style = Style::Normal,
            "side-by-side" => o.style = Style::Side,
            "brief" => o.brief = true,
            "report-identical-files" => o.report_same = true,
            "width" => {
                match std::str::from_utf8(v.unwrap_or_default())
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
                    .filter(|&w| w > 0)
                {
                    Some(w) => o.width = w,
                    None => {
                        let q = String::from_utf8_lossy(v.unwrap_or_default()).into_owned();
                        return Err(usage(ctx, &format!("invalid width '{q}'")).await);
                    }
                }
            }
            "suppress-common-lines" => o.suppress_common = true,
            "left-column" => o.left_column = true,
            "recursive" => o.recursive = true,
            "new-file" => o.new_file = true,
            "exclude" => patterns.push(v.unwrap_or_default().to_vec()),
            "exclude-from" => {
                let name = v.unwrap_or_default();
                let mut text = Vec::new();
                let r = match Input::open(ctx, name).await {
                    Ok(i) => i.read_to_end(ctx, &mut text).await,
                    Err(e) => Err(e),
                };
                if let Err(e) = r {
                    common::file_error(ctx, name, &e).await;
                    return Err(2);
                }
                patterns.extend(
                    text.split(|&b| b == b'\n')
                        .filter(|l| !l.is_empty())
                        .map(<[u8]>::to_vec),
                );
            }
            "ignore-matching-lines" => match compile_bre(v.unwrap_or_default()) {
                Some(re) => o.ignore_re.push(re),
                // A pattern the translation cannot carry: the real diff, if any.
                None => return Err(fall_through(ctx, argv, "-I").await),
            },
            "ignore-case" => o.icase = true,
            "ignore-all-space" => o.all_space = true,
            "ignore-space-change" => o.space_change = true,
            "ignore-blank-lines" => o.blank_lines = true,
            "strip-trailing-cr" => o.strip_cr = true,
            "text" => o.text = true,
            "label" => o.labels.push(v.unwrap_or_default().to_vec()),
            "color" => {
                o.color = match v {
                    None | Some(b"auto") => ctx.is_terminal(1),
                    Some(b"always") => true,
                    Some(b"never") => false,
                    Some(other) => {
                        let q = String::from_utf8_lossy(other).into_owned();
                        return Err(
                            usage(ctx, &format!("invalid argument '{q}' for '--color'")).await
                        );
                    }
                }
            }
            "show-c-function" => o.function_re = compile_bre(b"^[[:alpha:]$_]"),
            "show-function-line" => match compile_bre(v.unwrap_or_default()) {
                Some(re) => o.function_re = Some(re),
                None => return Err(fall_through(ctx, argv, "-F").await),
            },
            _ => {}
        }
    }
    for pat in patterns {
        match crate::search::glob::GlobBuilder::new(&String::from_utf8_lossy(&pat))
            .literal_separator(true)
            .backslash_escape(true)
            .build()
        {
            Ok(g) => o.exclude.push(g.compile_matcher()),
            Err(_) => return Err(fall_through(ctx, argv, "-x").await),
        }
    }
    // The options as written: argv without its two operands.
    let mut words: Vec<&Vec<u8>> = argv[1..].iter().collect();
    for op in p.operands.iter().rev() {
        if let Some(i) = words.iter().rposition(|w| *w == op) {
            words.remove(i);
        }
    }
    for w in words {
        o.header_opts.extend_from_slice(&shell_word(w));
        o.header_opts.push(b' ');
    }
    Ok(o)
}

/// A form psh cannot carry: the real program, or a clear failure.
async fn fall_through(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>], what: &str) -> i32 {
    if let Some(st) = ctx.run_program(argv.to_vec()).await {
        return st;
    }
    ctx.error(&format!("unsupported {what} pattern (psh built-in diff)"))
        .await;
    2
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match SPEC.parse(&argv[1..]) {
        Ok(p) => p,
        Err(e) if e.is_unknown() => return common::option_error(ctx, &SPEC, argv, &e).await,
        Err(e) => return usage(ctx, &e.to_string()).await,
    };
    let o = match options(ctx, &p, argv).await {
        Ok(o) => o,
        Err(st) => return st,
    };
    match p.operands.len() {
        0 => return usage(ctx, "missing operand after 'diff'").await,
        1 => {
            let q = String::from_utf8_lossy(&p.operands[0]).into_owned();
            return usage(ctx, &format!("missing operand after '{q}'")).await;
        }
        2 => {}
        _ => {
            let q = String::from_utf8_lossy(&p.operands[2]).into_owned();
            return usage(ctx, &format!("extra operand '{q}'")).await;
        }
    }
    ctx.load_zone().await;
    let mut d = Differ { o, status: 0 };
    let (a, b) = (p.operands[0].clone(), p.operands[1].clone());
    let ka = kind(ctx, &a).await;
    let kb = kind(ctx, &b).await;
    let r = match (ka, kb) {
        (Some(true), Some(true)) => d.dirs(ctx, &a, &b, true).await,
        (Some(true), Some(false)) => {
            let a2 = join(&a, basename(&b));
            d.files(ctx, &a2, &b, false).await
        }
        (Some(false), Some(true)) => {
            let b2 = join(&b, basename(&a));
            d.files(ctx, &a, &b2, false).await
        }
        _ => d.files(ctx, &a, &b, false).await,
    };
    match r {
        Ok(()) => d.status,
        Err(e) => common::write_failed(ctx, &e).await,
    }
}

/// `Some(true)` for a directory, `Some(false)` for another file, `None` when missing
/// (stdin is a file).
async fn kind(ctx: &UtilCtx<'_>, name: &[u8]) -> Option<bool> {
    if name == b"-" {
        return Some(false);
    }
    ctx.stat(name, true)
        .await
        .ok()
        .map(|m| m.kind == FileKind::Dir)
}

fn basename(p: &[u8]) -> &[u8] {
    let t = p.strip_suffix(b"/").unwrap_or(p);
    t.rsplit(|&b| b == b'/').next().unwrap_or(t)
}

fn join(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut p = dir.to_vec();
    if !p.ends_with(b"/") {
        p.push(b'/');
    }
    p.extend_from_slice(name);
    p
}

/// A file's lines (without their newlines) and whether the last one lacks one.
struct Text {
    data: Vec<u8>,
    lines: Vec<(usize, usize)>,
    no_newline: bool,
    mtime_ns: i64,
}

impl Text {
    fn line(&self, i: usize) -> &[u8] {
        let (s, e) = self.lines[i];
        &self.data[s..e]
    }

    fn split(data: Vec<u8>, mtime_ns: i64) -> Text {
        let mut lines = Vec::new();
        let mut start = 0;
        for (i, &b) in data.iter().enumerate() {
            if b == b'\n' {
                lines.push((start, i));
                start = i + 1;
            }
        }
        let no_newline = start < data.len();
        if no_newline {
            lines.push((start, data.len()));
        }
        Text {
            data,
            lines,
            no_newline,
            mtime_ns,
        }
    }
}

type LineIds<'a> = HashMap<(Cow<'a, [u8]>, bool), u32, crate::hasher::SeededState>;

struct Differ {
    o: Opts,
    status: i32,
}

const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const RESET: &str = "\x1b[0m";

impl Differ {
    fn trouble(&mut self) {
        self.status = 2;
    }

    fn differs(&mut self) {
        self.status = self.status.max(1);
    }

    async fn read(&mut self, ctx: &mut UtilCtx<'_>, name: &[u8]) -> io::Result<Text> {
        let now = i64::try_from(ctx.now().as_nanos()).unwrap_or(0);
        let mtime = if name == b"-" {
            now
        } else {
            ctx.stat(name, true).await?.mtime_ns
        };
        let input = Input::open(ctx, name).await?;
        let mut data = Vec::new();
        input.read_to_end(ctx, &mut data).await?;
        Ok(Text::split(data, mtime))
    }

    /// The comparison key of a line under -i -w -b --strip-trailing-cr.
    fn key<'a>(&self, line: &'a [u8]) -> Cow<'a, [u8]> {
        if !(self.o.strip_cr || self.o.all_space || self.o.space_change || self.o.icase) {
            return Cow::Borrowed(line);
        }
        let mut l = line;
        if self.o.strip_cr {
            l = l.strip_suffix(b"\r").unwrap_or(l);
        }
        let ws = |b: u8| matches!(b, b' ' | b'\t' | b'\r' | b'\x0b' | b'\x0c');
        let mut k: Vec<u8> = if self.o.all_space {
            l.iter().copied().filter(|&b| !ws(b)).collect()
        } else if self.o.space_change {
            let mut out = Vec::with_capacity(l.len());
            let mut in_ws = false;
            for &b in l {
                if ws(b) {
                    in_ws = true;
                } else {
                    // A run before a non-blank is one space; a trailing run is none.
                    if in_ws {
                        out.push(b' ');
                    }
                    in_ws = false;
                    out.push(b);
                }
            }
            out
        } else {
            l.to_vec()
        };
        if self.o.icase {
            k.make_ascii_lowercase();
        }
        Cow::Owned(k)
    }

    fn line_ids<'a>(&self, t: &'a Text, ids: &mut LineIds<'a>) -> Vec<u32> {
        (0..t.lines.len())
            .map(|i| {
                let last = t.no_newline && i + 1 == t.lines.len();
                let k = (self.key(t.line(i)), last);
                let next = ids.len() as u32;
                *ids.entry(k).or_insert(next)
            })
            .collect()
    }

    fn ignorable_line(&self, line: &[u8]) -> bool {
        (self.o.blank_lines && line.iter().all(|b| b.is_ascii_whitespace()))
            || (!self.o.ignore_re.is_empty() && self.o.ignore_re.iter().any(|r| r.is_match(line)))
    }

    fn ignorable(&self, ta: &Text, tb: &Text, c: &Change) -> bool {
        if !self.o.blank_lines && self.o.ignore_re.is_empty() {
            return false;
        }
        (c.a.0..c.a.1).all(|i| self.ignorable_line(ta.line(i)))
            && (c.b.0..c.b.1).all(|i| self.ignorable_line(tb.line(i)))
    }

    fn label(&self, i: usize, name: &[u8], t: &Text, ctx: &UtilCtx<'_>) -> Vec<u8> {
        if let Some(l) = self.o.labels.get(i) {
            return l.clone();
        }
        let mut out = name.to_vec();
        out.push(b'\t');
        let tm = timefmt::local(ctx, t.mtime_ns);
        let secs = t.mtime_ns.div_euclid(timefmt::NS);
        out.extend_from_slice(&timefmt::format(
            b"%Y-%m-%d %H:%M:%S.%N %z",
            &tm,
            &ctx.zone_abbr(secs),
            secs,
        ));
        out
    }

    /// Compares two files (either may be missing with -N); `in_dir` prints the `diff
    /// OPTS a b` line before a difference.
    async fn files(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        a: &[u8],
        b: &[u8],
        in_dir: bool,
    ) -> io::Result<()> {
        let mut texts = Vec::with_capacity(2);
        for name in [a, b] {
            match self.read(ctx, name).await {
                Ok(t) => texts.push(Some(t)),
                Err(e) if self.o.new_file && e.kind() == io::ErrorKind::NotFound => {
                    texts.push(None)
                }
                Err(e) => {
                    common::file_error(ctx, name, &e).await;
                    self.trouble();
                    return Ok(());
                }
            }
        }
        if a == b"-" && b == b"-" {
            texts[1] = texts[0]
                .as_ref()
                .map(|t| Text::split(t.data.clone(), t.mtime_ns));
        }
        let tb = texts
            .pop()
            .unwrap()
            .unwrap_or_else(|| Text::split(Vec::new(), 0));
        let ta = texts
            .pop()
            .unwrap()
            .unwrap_or_else(|| Text::split(Vec::new(), 0));
        if ta.data == tb.data {
            if self.o.report_same {
                let msg = format!(
                    "Files {} and {} are identical\n",
                    String::from_utf8_lossy(a),
                    String::from_utf8_lossy(b)
                );
                ctx.write(msg.as_bytes()).await?;
            }
            return Ok(());
        }
        let binary = !self.o.text
            && (ta.data[..ta.data.len().min(32768)].contains(&0)
                || tb.data[..tb.data.len().min(32768)].contains(&0));
        if binary || (self.o.brief && !self.normalizes()) {
            self.differs();
            let what = if self.o.brief {
                "Files"
            } else {
                "Binary files"
            };
            let msg = format!(
                "{what} {} and {} differ\n",
                String::from_utf8_lossy(a),
                String::from_utf8_lossy(b)
            );
            return ctx.write(msg.as_bytes()).await;
        }
        // Line ids: equal keys, and the missing newline of a last line counts.
        let mut ids = LineIds::default();
        let sa = self.line_ids(&ta, &mut ids);
        let sb = self.line_ids(&tb, &mut ids);
        ctx.check().await;
        let changes = diff_algo::diff(&sa, &sb);
        let shown_flags: Vec<bool> = changes
            .iter()
            .map(|c| !self.ignorable(&ta, &tb, c))
            .collect();
        let shown: Vec<Change> = changes
            .iter()
            .zip(&shown_flags)
            .filter(|(_, s)| **s)
            .map(|(c, _)| *c)
            .collect();
        if shown.is_empty() {
            if self.o.report_same {
                let msg = format!(
                    "Files {} and {} are identical\n",
                    String::from_utf8_lossy(a),
                    String::from_utf8_lossy(b)
                );
                ctx.write(msg.as_bytes()).await?;
            }
            return Ok(());
        }
        self.differs();
        if self.o.brief {
            let msg = format!(
                "Files {} and {} differ\n",
                String::from_utf8_lossy(a),
                String::from_utf8_lossy(b)
            );
            return ctx.write(msg.as_bytes()).await;
        }
        let mut out = Vec::new();
        if in_dir {
            out.extend_from_slice(b"diff ");
            out.extend_from_slice(&self.o.header_opts);
            out.extend_from_slice(a);
            out.push(b' ');
            out.extend_from_slice(b);
            out.push(b'\n');
        }
        match self.o.style {
            Style::Normal => self.normal(&mut out, &ta, &tb, &shown),
            Style::Unified(n) | Style::Context(n) => {
                let unified = matches!(self.o.style, Style::Unified(_));
                let (h1, h2) = if unified {
                    ("--- ", "+++ ")
                } else {
                    ("*** ", "--- ")
                };
                for (i, (h, name, t)) in [(h1, a, &ta), (h2, b, &tb)].into_iter().enumerate() {
                    let label = self.label(i, name, t, ctx);
                    self.colored(&mut out, BOLD, |o| {
                        o.extend_from_slice(h.as_bytes());
                        o.extend_from_slice(&label);
                    });
                }
                for hunk in hunks(&changes, &shown_flags, n, ta.lines.len(), tb.lines.len()) {
                    if unified {
                        self.unified(&mut out, &ta, &tb, &hunk);
                    } else {
                        self.context(&mut out, &ta, &tb, &hunk);
                    }
                }
            }
            Style::Side => self.side(&mut out, &ta, &tb, &changes),
        }
        ctx.write(&out).await
    }

    /// Whether lines compare other than by bytes (then `-q` must diff to know).
    fn normalizes(&self) -> bool {
        self.o.icase
            || self.o.all_space
            || self.o.space_change
            || self.o.blank_lines
            || self.o.strip_cr
            || !self.o.ignore_re.is_empty()
    }

    fn colored(&self, out: &mut Vec<u8>, color: &str, f: impl FnOnce(&mut Vec<u8>)) {
        let paint = self.o.color && !color.is_empty();
        if paint {
            out.extend_from_slice(color.as_bytes());
        }
        f(out);
        if paint {
            out.extend_from_slice(RESET.as_bytes());
        }
        out.push(b'\n');
    }

    /// A line with its prefix, and the marker of a missing newline.
    fn line(&self, out: &mut Vec<u8>, color: &str, prefix: &[u8], t: &Text, i: usize) {
        self.colored(out, color, |o| {
            o.extend_from_slice(prefix);
            o.extend_from_slice(t.line(i));
        });
        if t.no_newline && i + 1 == t.lines.len() {
            out.extend_from_slice(b"\\ No newline at end of file\n");
        }
    }

    fn normal(&self, out: &mut Vec<u8>, ta: &Text, tb: &Text, changes: &[Change]) {
        let range = |(s, e): (usize, usize)| -> String {
            if e - s == 1 {
                format!("{}", s + 1)
            } else if e == s {
                format!("{s}")
            } else {
                format!("{},{}", s + 1, e)
            }
        };
        for c in changes {
            let op = match (c.a.0 == c.a.1, c.b.0 == c.b.1) {
                (true, _) => 'a',
                (_, true) => 'd',
                _ => 'c',
            };
            let head = format!("{}{op}{}", range(c.a), range(c.b));
            self.colored(out, CYAN, |o| o.extend_from_slice(head.as_bytes()));
            for i in c.a.0..c.a.1 {
                self.line(out, RED, b"< ", ta, i);
            }
            if op == 'c' {
                out.extend_from_slice(b"---\n");
            }
            for i in c.b.0..c.b.1 {
                self.line(out, GREEN, b"> ", tb, i);
            }
        }
    }

    /// The `-p`/`-F` text for a hunk that starts at line `start` of the first file.
    fn function(&self, ta: &Text, start: usize) -> Option<Vec<u8>> {
        let re = self.o.function_re.as_ref()?;
        (0..start).rev().find_map(|i| {
            let l = ta.line(i);
            re.is_match(l).then(|| {
                let mut f = l[..l.len().min(40)].to_vec();
                while f.last().is_some_and(u8::is_ascii_whitespace) {
                    f.pop();
                }
                f
            })
        })
    }

    fn unified(&self, out: &mut Vec<u8>, ta: &Text, tb: &Text, h: &Hunk) {
        let range = |s: usize, e: usize| -> String {
            match e - s {
                0 => format!("{s},0"),
                1 => format!("{}", s + 1),
                n => format!("{},{n}", s + 1),
            }
        };
        let mut head = format!("@@ -{} +{} @@", range(h.a.0, h.a.1), range(h.b.0, h.b.1));
        if let Some(f) = self.function(ta, h.a.0) {
            head.push(' ');
            head.push_str(&String::from_utf8_lossy(&f));
        }
        self.colored(out, CYAN, |o| o.extend_from_slice(head.as_bytes()));
        let mut i = h.a.0;
        for c in &h.changes {
            while i < c.a.0 {
                self.line(out, "", b" ", ta, i);
                i += 1;
            }
            for k in c.a.0..c.a.1 {
                self.line(out, RED, b"-", ta, k);
            }
            for k in c.b.0..c.b.1 {
                self.line(out, GREEN, b"+", tb, k);
            }
            i = c.a.1;
        }
        while i < h.a.1 {
            self.line(out, "", b" ", ta, i);
            i += 1;
        }
    }

    fn context(&self, out: &mut Vec<u8>, ta: &Text, tb: &Text, h: &Hunk) {
        let range = |s: usize, e: usize| -> String {
            match e - s {
                0 => format!("{s}"),
                1 => format!("{}", s + 1),
                _ => format!("{},{}", s + 1, e),
            }
        };
        let mut stars = b"***************".to_vec();
        if let Some(f) = self.function(ta, h.a.0) {
            stars.push(b' ');
            stars.extend_from_slice(&f);
        }
        out.extend_from_slice(&stars);
        out.push(b'\n');
        let head = format!("*** {} ****", range(h.a.0, h.a.1));
        self.colored(out, CYAN, |o| o.extend_from_slice(head.as_bytes()));
        if h.changes.iter().any(|c| c.a.1 > c.a.0) {
            self.context_side(out, ta, h, true);
        }
        let head = format!("--- {} ----", range(h.b.0, h.b.1));
        self.colored(out, CYAN, |o| o.extend_from_slice(head.as_bytes()));
        if h.changes.iter().any(|c| c.b.1 > c.b.0) {
            self.context_side(out, tb, h, false);
        }
    }

    /// One side of a context hunk: `  ` common, `- `/`+ ` alone, `! ` changed.
    fn context_side(&self, out: &mut Vec<u8>, t: &Text, h: &Hunk, first: bool) {
        let color = if first { RED } else { GREEN };
        let (lo, hi) = if first { h.a } else { h.b };
        let mut i = lo;
        for c in &h.changes {
            let (s, e) = if first { c.a } else { c.b };
            while i < s {
                self.line(out, color, b"  ", t, i);
                i += 1;
            }
            let both = c.a.1 > c.a.0 && c.b.1 > c.b.0;
            let mark: &[u8] = match (both, first) {
                (true, _) => b"! ",
                (false, true) => b"- ",
                (false, false) => b"+ ",
            };
            for k in s..e {
                self.line(out, color, mark, t, k);
            }
            i = e;
        }
        while i < hi {
            self.line(out, color, b"  ", t, i);
            i += 1;
        }
    }

    /// `-y`: the two files in columns (D1205 has the geometry).
    fn side(&self, out: &mut Vec<u8>, ta: &Text, tb: &Text, changes: &[Change]) {
        let w = self.o.width;
        let off = (w + 11) / 2 / 8 * 8;
        let hw = (off.saturating_sub(3)).min(w.saturating_sub(off));
        let marker_col = (hw + off).saturating_sub(1) / 2;
        let row = |out: &mut Vec<u8>, left: Option<&[u8]>, mark: u8, right: Option<&[u8]>| {
            let mut col = 0;
            if let Some(l) = left {
                col = put_clipped(out, l, hw);
            }
            if mark == b' ' || mark == b'(' {
                if mark == b'(' {
                    pad_to(out, col, marker_col);
                    out.push(b'(');
                } else if let Some(r) = right {
                    pad_to(out, col, off);
                    put_clipped(out, r, hw);
                }
            } else {
                pad_to(out, col, marker_col);
                out.push(mark);
                if let Some(r) = right {
                    pad_to(out, marker_col + 1, off);
                    put_clipped(out, r, hw);
                }
            }
            out.push(b'\n');
        };
        let (mut i, mut j) = (0, 0);
        let common = |out: &mut Vec<u8>, i: usize, j: usize| {
            if self.o.suppress_common {
                return;
            }
            if self.o.left_column {
                row(out, Some(ta.line(i)), b'(', None);
            } else {
                row(out, Some(ta.line(i)), b' ', Some(tb.line(j)));
            }
        };
        for c in changes {
            while i < c.a.0 {
                common(out, i, j);
                i += 1;
                j += 1;
            }
            let (na, nb) = (c.a.1 - c.a.0, c.b.1 - c.b.0);
            for k in 0..na.max(nb) {
                match (k < na, k < nb) {
                    (true, true) => {
                        let start = out.len();
                        row(
                            out,
                            Some(ta.line(c.a.0 + k)),
                            b'|',
                            Some(tb.line(c.b.0 + k)),
                        );
                        self.paint(out, start, GREEN);
                    }
                    (true, false) => {
                        let start = out.len();
                        row(out, Some(ta.line(c.a.0 + k)), b'<', None);
                        self.paint(out, start, RED);
                    }
                    (false, _) => {
                        let start = out.len();
                        row(out, None, b'>', Some(tb.line(c.b.0 + k)));
                        self.paint(out, start, GREEN);
                    }
                }
            }
            i = c.a.1;
            j = c.b.1;
        }
        while i < ta.lines.len() {
            common(out, i, j);
            i += 1;
            j += 1;
        }
    }

    /// Wraps the row written since `start` (ending in a newline) in a color.
    fn paint(&self, out: &mut Vec<u8>, start: usize, color: &str) {
        if !self.o.color {
            return;
        }
        out.pop();
        out.splice(start..start, color.bytes());
        out.extend_from_slice(RESET.as_bytes());
        out.push(b'\n');
    }

    /// Compares two directories (`top`: the operands themselves).
    async fn dirs(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        a: &[u8],
        b: &[u8],
        _top: bool,
    ) -> io::Result<()> {
        let list = |r: io::Result<Vec<psh_engine::host::DirEntry>>| -> Option<Vec<Vec<u8>>> {
            r.ok().map(|v| v.into_iter().map(|e| e.name).collect())
        };
        let la = list(ctx.read_dir(a).await);
        let lb = list(ctx.read_dir(b).await);
        let (la, lb) = match (la, lb) {
            (Some(x), Some(y)) => (x, y),
            (x, y) => {
                // A side that is missing (with -N) is an empty directory.
                for (side, dir) in [(&x, a), (&y, b)] {
                    if side.is_none() && !self.o.new_file {
                        let e =
                            io::Error::new(io::ErrorKind::NotFound, "No such file or directory");
                        common::file_error(ctx, dir, &e).await;
                        self.trouble();
                        return Ok(());
                    }
                }
                (x.unwrap_or_default(), y.unwrap_or_default())
            }
        };
        let mut names: Vec<Vec<u8>> = la.iter().chain(lb.iter()).cloned().collect();
        names.sort();
        names.dedup();
        names.retain(|n| {
            let s = String::from_utf8_lossy(n);
            !self.o.exclude.iter().any(|g| g.is_match(s.as_ref()))
        });
        for name in names {
            ctx.check().await;
            let (pa, pb) = (join(a, &name), join(b, &name));
            let ka = if la.contains(&name) {
                kind(ctx, &pa).await
            } else {
                None
            };
            let kb = if lb.contains(&name) {
                kind(ctx, &pb).await
            } else {
                None
            };
            match (ka, kb) {
                (Some(true), Some(true)) => {
                    if self.o.recursive {
                        Box::pin(self.dirs(ctx, &pa, &pb, false)).await?;
                    } else {
                        let msg = format!(
                            "Common subdirectories: {} and {}\n",
                            String::from_utf8_lossy(&pa),
                            String::from_utf8_lossy(&pb)
                        );
                        ctx.write(msg.as_bytes()).await?;
                    }
                }
                (Some(false), Some(false)) => self.files(ctx, &pa, &pb, true).await?,
                (Some(da), Some(db)) => {
                    self.differs();
                    let what = |d: bool| if d { "directory" } else { "regular file" };
                    let msg = format!(
                        "File {} is a {} while file {} is a {}\n",
                        String::from_utf8_lossy(&pa),
                        what(da),
                        String::from_utf8_lossy(&pb),
                        what(db)
                    );
                    ctx.write(msg.as_bytes()).await?;
                }
                (Some(d), None) | (None, Some(d)) if self.o.new_file => {
                    if d {
                        if self.o.recursive {
                            Box::pin(self.dirs(ctx, &pa, &pb, false)).await?;
                        }
                    } else {
                        self.files(ctx, &pa, &pb, true).await?;
                    }
                }
                (Some(_), None) => self.only(ctx, a, &name).await?,
                (None, Some(_)) => self.only(ctx, b, &name).await?,
                (None, None) => {}
            }
        }
        Ok(())
    }

    async fn only(&mut self, ctx: &mut UtilCtx<'_>, dir: &[u8], name: &[u8]) -> io::Result<()> {
        self.differs();
        let d = dir
            .strip_suffix(b"/")
            .filter(|d| !d.is_empty())
            .unwrap_or(dir);
        let msg = format!(
            "Only in {}: {}\n",
            String::from_utf8_lossy(d),
            String::from_utf8_lossy(name)
        );
        ctx.write(msg.as_bytes()).await
    }
}

/// Writes `s` clipped to `width` display columns (tabs to the next multiple of 8);
/// returns the column reached.
fn put_clipped(out: &mut Vec<u8>, s: &[u8], width: usize) -> usize {
    let mut col = 0;
    for &b in s {
        let next = if b == b'\t' {
            (col / 8 + 1) * 8
        } else {
            col + 1
        };
        if next > width {
            break;
        }
        out.push(b);
        col = next;
    }
    col
}

/// Pads from column `col` to `to` with tabs where they fit, then spaces.
fn pad_to(out: &mut Vec<u8>, mut col: usize, to: usize) {
    while (col / 8 + 1) * 8 <= to {
        out.push(b'\t');
        col = (col / 8 + 1) * 8;
    }
    while col < to {
        out.push(b' ');
        col += 1;
    }
}

/// A unified/context hunk: its ranges and the changes inside.
struct Hunk {
    a: (usize, usize),
    b: (usize, usize),
    changes: Vec<Change>,
}

/// Groups the shown changes into hunks with `n` lines of context, merging hunks whose
/// gap is at most `2n` lines. A hunk takes in every change within its range, shown or
/// ignorable (`-B`, `-I`), so that both files' ranges stay aligned.
fn hunks(all: &[Change], shown: &[bool], n: usize, la: usize, lb: usize) -> Vec<Hunk> {
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for (i, c) in all.iter().enumerate() {
        if !shown[i] {
            continue;
        }
        match groups.last_mut() {
            Some(g) if c.a.0 - all[g.1].a.1 <= 2 * n => g.1 = i,
            _ => groups.push((i, i)),
        }
    }
    let _ = lb;
    groups
        .into_iter()
        .map(|(mut f, mut l)| {
            while f > 0 && all[f - 1].a.1 > all[f].a.0.saturating_sub(n) {
                f -= 1;
            }
            while l + 1 < all.len() && all[l + 1].a.0 < all[l].a.1 + n {
                l += 1;
            }
            let (first, last) = (all[f], all[l]);
            let a0 = first.a.0.saturating_sub(n);
            let b0 = first.b.0 - (first.a.0 - a0);
            let a1 = (last.a.1 + n).min(la);
            let b1 = last.b.1 + (a1 - last.a.1);
            Hunk {
                a: (a0, a1),
                b: (b0, b1),
                changes: all[f..=l].to_vec(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        assert_eq!(shell_word(b"-ru"), b"-ru");
        assert_eq!(shell_word(b"*.log"), b"'*.log'");
        assert_eq!(shell_word(b"--exclude=b*"), b"'--exclude=b*'");
    }

    #[test]
    fn columns() {
        let mut out = Vec::new();
        pad_to(&mut out, 1, 14);
        assert_eq!(out, b"\t      ");
        let mut out = Vec::new();
        assert_eq!(put_clipped(&mut out, b"longlonglonglong", 13), 13);
    }
}
