// The JSON printer below (`JSONBuilder`, `JSON`, `JSONSink`, `SubMatches`, the message
// types and `base64_standard`) is copied from grep-printer 0.3.1 (`src/json.rs`,
// `src/jsont.rs`) in ripgrep (https://github.com/BurntSushi/ripgrep): Copyright (c)
// 2015 Andrew Gallant. Licensed under the MIT License (see NOTICE).

//! `rg --json` (D029, D1136): grep-printer's JSON printer, whose serialization needs
//! serde, with the messages built as a small JSON tree written here. The
//! output is grep-printer's byte for byte: the same fields in the same order, compact,
//! `\n` after each message, `{"text":..}` or base64 `{"bytes":..}` data.

use std::{
    io::{self, Write},
    path::Path,
    sync::Arc,
    time::Duration,
};

use crate::matcher::{Match, Matcher};
use crate::printer::Stats;
use crate::printer::util::{CounterWriter, Replacer, find_iter_at_in_context};
use crate::searcher::{Searcher, Sink, SinkContext, SinkFinish, SinkMatch};

/// A JSON value; objects keep their keys in order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Json {
    Null,
    Num(u64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(&'static str, Json)>),
}

impl Json {
    fn obj() -> Json {
        Json::Obj(Vec::new())
    }

    fn set(mut self, key: &'static str, value: Json) -> Json {
        if let Json::Obj(fields) = &mut self {
            fields.push((key, value));
        }
        self
    }

    /// The compact form and a newline (serde_json's `to_writer` and the `\n`
    /// grep-printer adds).
    pub(crate) fn write_line<W: Write>(&self, mut w: W) -> io::Result<()> {
        w.write_all(self.compact().as_bytes())?;
        w.write_all(b"\n")
    }
}

enum Open<'a> {
    Items { rest: std::slice::Iter<'a, Json>, first: bool },
    Fields { rest: std::slice::Iter<'a, (&'static str, Json)>, first: bool },
}

impl Json {
    fn compact(&self) -> String {
        let mut out = String::new();
        let mut stack: Vec<Open<'_>> = Vec::new();
        let mut next = Some(self);
        loop {
            match next.take() {
                Some(Json::Null) => out.push_str("null"),
                Some(Json::Num(n)) => out.push_str(&n.to_string()),
                Some(Json::Str(s)) => write_string(s, &mut out),
                Some(Json::Arr(items)) => {
                    out.push('[');
                    stack
                        .push(Open::Items { rest: items.iter(), first: true });
                }
                Some(Json::Obj(fields)) => {
                    out.push('{');
                    stack.push(Open::Fields {
                        rest: fields.iter(),
                        first: true,
                    });
                }
                None => {}
            }
            let Some(top) = stack.last_mut() else {
                break;
            };
            match top {
                Open::Items { rest, first } => match rest.next() {
                    Some(item) => {
                        if !std::mem::replace(first, false) {
                            out.push(',');
                        }
                        next = Some(item);
                    }
                    None => {
                        out.push(']');
                        stack.pop();
                    }
                },
                Open::Fields { rest, first } => match rest.next() {
                    Some((key, value)) => {
                        if !std::mem::replace(first, false) {
                            out.push(',');
                        }
                        write_string(key, &mut out);
                        out.push(':');
                        next = Some(value);
                    }
                    None => {
                        out.push('}');
                        stack.pop();
                    }
                },
            }
        }
        out
    }
}

fn write_string(s: &str, out: &mut String) {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if c < ' ' => {
                let byte = c as u8;
                out.push_str("\\u00");
                out.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
                out.push(char::from(HEX_DIGITS[usize::from(byte & 0xf)]));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn opt_num(n: Option<u64>) -> Json {
    n.map_or(Json::Null, Json::Num)
}

/// grep-printer's `NiceDuration`.
fn duration(d: Duration) -> Json {
    let fractional = (d.subsec_nanos() as f64) / 1_000_000_000.0;
    let secs = d.as_secs() as f64 + fractional;
    Json::obj()
        .set("secs", Json::Num(d.as_secs()))
        .set("nanos", Json::Num(u64::from(d.subsec_nanos())))
        .set("human", Json::Str(format!("{secs:0.6}s")))
}

/// grep-printer's `Stats` serialization.
pub(crate) fn stats(s: &Stats) -> Json {
    Json::obj()
        .set("elapsed", duration(s.elapsed()))
        .set("searches", Json::Num(s.searches()))
        .set("searches_with_match", Json::Num(s.searches_with_match()))
        .set("bytes_searched", Json::Num(s.bytes_searched()))
        .set("bytes_printed", Json::Num(s.bytes_printed()))
        .set("matched_lines", Json::Num(s.matched_lines()))
        .set("matches", Json::Num(s.matches()))
}

/// The summary message rg prints after a `--json` search. rg builds it with
/// serde_json's `json!`, whose maps sort their keys, so the keys are sorted here too.
pub(crate) fn summary(s: &Stats, elapsed: Duration) -> Json {
    sorted(summary_fields(s, elapsed))
}

/// The value with every object's keys sorted (serde_json's `Map` without
/// `preserve_order`).
fn sorted(j: Json) -> Json {
    match j {
        Json::Obj(mut fields) => {
            fields.sort_by(|a, b| a.0.cmp(b.0));
            Json::Obj(
                fields.into_iter().map(|(k, v)| (k, sorted(v))).collect(),
            )
        }
        Json::Arr(items) => Json::Arr(items.into_iter().map(sorted).collect()),
        j => j,
    }
}

fn summary_fields(s: &Stats, elapsed: Duration) -> Json {
    Json::obj().set("type", Json::Str("summary".into())).set(
        "data",
        Json::obj().set("stats", stats(s)).set(
            "elapsed_total",
            Json::obj()
                .set("secs", Json::Num(elapsed.as_secs()))
                .set("nanos", Json::Num(u64::from(elapsed.subsec_nanos())))
                .set(
                    "human",
                    Json::Str(format!("{:0.6}s", elapsed.as_secs_f64())),
                ),
        ),
    )
}

fn message(kind: &str, data: Json) -> Json {
    Json::obj().set("type", Json::Str(kind.into())).set("data", data)
}

/// `jsont::Data::from_bytes`.
fn data_bytes(bytes: &[u8]) -> Json {
    match std::str::from_utf8(bytes) {
        Ok(text) => Json::obj().set("text", Json::Str(text.into())),
        Err(_) => Json::obj().set("bytes", Json::Str(base64_standard(bytes))),
    }
}

/// `jsont::Data::from_path`.
fn data_path(path: Option<&Path>) -> Json {
    let Some(path) = path else { return Json::Null };
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        match path.to_str() {
            Some(text) => Json::obj().set("text", Json::Str(text.into())),
            None => Json::obj().set(
                "bytes",
                Json::Str(base64_standard(path.as_os_str().as_bytes())),
            ),
        }
    }
    #[cfg(not(unix))]
    {
        Json::obj().set("text", Json::Str(path.to_string_lossy().into_owned()))
    }
}

/// `jsont::SubMatch`.
struct SubMatch<'a> {
    m: &'a [u8],
    replacement: Option<&'a [u8]>,
    start: usize,
    end: usize,
}

fn submatches(subs: &[SubMatch<'_>]) -> Json {
    Json::Arr(
        subs.iter()
            .map(|s| {
                let mut o = Json::obj().set("match", data_bytes(s.m));
                if let Some(r) = s.replacement {
                    o = o.set("replacement", data_bytes(r));
                }
                o.set("start", Json::Num(s.start as u64))
                    .set("end", Json::Num(s.end as u64))
            })
            .collect(),
    )
}

/// `jsont::Match` and `jsont::Context`.
fn lines_message(
    kind: &str,
    path: Option<&Path>,
    lines: &[u8],
    line_number: Option<u64>,
    absolute_offset: u64,
    subs: &[SubMatch<'_>],
) -> Json {
    message(
        kind,
        Json::obj()
            .set("path", data_path(path))
            .set("lines", data_bytes(lines))
            .set("line_number", opt_num(line_number))
            .set("absolute_offset", Json::Num(absolute_offset))
            .set("submatches", submatches(subs)),
    )
}

#[derive(Debug, Clone)]
struct Config {
    always_begin_end: bool,
    replacement: Arc<Option<Vec<u8>>>,
}

impl Default for Config {
    fn default() -> Config {
        Config { always_begin_end: false, replacement: Arc::new(None) }
    }
}

/// A builder for a JSON lines printer.
#[derive(Clone, Debug)]
pub(crate) struct JSONBuilder {
    config: Config,
}

impl JSONBuilder {
    /// Return a new builder for configuring the JSON printer.
    pub(crate) fn new() -> JSONBuilder {
        JSONBuilder { config: Config::default() }
    }

    /// Create a JSON printer that writes results to the given writer.
    pub(crate) fn build<W: io::Write>(&self, wtr: W) -> JSON<W> {
        JSON {
            config: self.config.clone(),
            wtr: CounterWriter::new(wtr),
            matches: vec![],
        }
    }

    /// When enabled, the `begin` and `end` messages are always emitted, even
    /// when no match is found.
    ///
    /// When disabled, the `begin` and `end` messages are only shown if there
    /// is at least one `match` or `context` message.
    ///
    /// This is disabled by default.
    pub(crate) fn always_begin_end(&mut self, yes: bool) -> &mut JSONBuilder {
        self.config.always_begin_end = yes;
        self
    }

    /// Set the bytes that will be used to replace each occurrence of a match
    /// found.
    ///
    /// The replacement bytes given may include references to capturing groups,
    /// which may either be in index form (e.g., `$2`) or can reference named
    /// capturing groups if present in the original pattern (e.g., `$foo`).
    ///
    /// For documentation on the full format, please see the `Capture` trait's
    /// `interpolate` method in the
    /// [grep-printer](https://docs.rs/grep-printer) crate.
    pub(crate) fn replacement(
        &mut self,
        replacement: Option<Vec<u8>>,
    ) -> &mut JSONBuilder {
        self.config.replacement = Arc::new(replacement);
        self
    }
}

/// The JSON printer, which emits results in a JSON lines format.
#[derive(Clone, Debug)]
pub(crate) struct JSON<W> {
    config: Config,
    wtr: CounterWriter<W>,
    matches: Vec<Match>,
}

impl<W: io::Write> JSON<W> {
    /// Return an implementation of `Sink` associated with a file path.
    ///
    /// When the printer is associated with a path, then it may, depending on
    /// its configuration, print the path along with the matches found.
    pub(crate) fn sink_with_path<'p, 's, M, P>(
        &'s mut self,
        matcher: M,
        path: &'p P,
    ) -> JSONSink<'p, 's, M, W>
    where
        M: Matcher,
        P: ?Sized + AsRef<Path>,
    {
        JSONSink {
            matcher,
            replacer: Replacer::new(),
            json: self,
            path: Some(path.as_ref()),
            start_time: crate::rg::main::Started::now(),
            match_count: 0,
            binary_byte_offset: None,
            begin_printed: false,
            stats: Stats::new(),
        }
    }

    /// Write the given message followed by a new line. The new line is
    /// determined from the configuration of the given searcher.
    fn write_message(&mut self, message: &Json) -> io::Result<()> {
        message.write_line(&mut self.wtr)
    }
}

impl<W> JSON<W> {
    /// Return a mutable reference to the underlying writer.
    pub(crate) fn get_mut(&mut self) -> &mut W {
        self.wtr.get_mut()
    }
}

/// An implementation of `Sink` associated with a matcher and an optional file
/// path for the JSON printer.
#[derive(Debug)]
pub(crate) struct JSONSink<'p, 's, M: Matcher, W> {
    matcher: M,
    replacer: Replacer<M>,
    json: &'s mut JSON<W>,
    path: Option<&'p Path>,
    start_time: crate::rg::main::Started,
    match_count: u64,
    binary_byte_offset: Option<u64>,
    begin_printed: bool,
    stats: Stats,
}

impl<'p, 's, M: Matcher, W: io::Write> JSONSink<'p, 's, M, W> {
    /// Returns true if and only if this printer received a match in the
    /// previous search.
    pub(crate) fn has_match(&self) -> bool {
        self.match_count > 0
    }

    /// Return a reference to the stats produced by the printer for all
    /// searches executed on this sink.
    pub(crate) fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Execute the matcher over the given bytes and record the match
    /// locations if the current configuration demands match granularity.
    fn record_matches(
        &mut self,
        searcher: &Searcher,
        bytes: &[u8],
        range: std::ops::Range<usize>,
    ) -> io::Result<()> {
        self.json.matches.clear();
        // If printing requires knowing the location of each individual match,
        // then compute and stored those right now for use later. While this
        // adds an extra copy for storing the matches, we do amortize the
        // allocation for it and this greatly simplifies the printing logic to
        // the extent that it's easy to ensure that we never do more than
        // one search to find the matches.
        let matches = &mut self.json.matches;
        find_iter_at_in_context(
            searcher,
            &self.matcher,
            bytes,
            range.clone(),
            |m| {
                let (s, e) = (m.start() - range.start, m.end() - range.start);
                matches.push(Match::new(s, e));
                true
            },
        );
        // Don't report empty matches appearing at the end of the bytes.
        if !matches.is_empty()
            && matches.last().unwrap().is_empty()
            && matches.last().unwrap().start() >= bytes.len()
        {
            matches.pop().unwrap();
        }
        Ok(())
    }

    /// If the configuration specifies a replacement, then this executes the
    /// replacement, lazily allocating memory if necessary.
    ///
    /// To access the result of a replacement, use `replacer.replacement()`.
    fn replace(
        &mut self,
        searcher: &Searcher,
        bytes: &[u8],
        range: std::ops::Range<usize>,
    ) -> io::Result<()> {
        self.replacer.clear();
        if self.json.config.replacement.is_some() {
            let replacement =
                (*self.json.config.replacement).as_ref().map(|r| &*r).unwrap();
            self.replacer.replace_all(
                searcher,
                &self.matcher,
                bytes,
                range,
                replacement,
            );
        }
        Ok(())
    }

    /// Write the "begin" message.
    fn write_begin_message(&mut self) -> io::Result<()> {
        if self.begin_printed {
            return Ok(());
        }
        let msg =
            message("begin", Json::obj().set("path", data_path(self.path)));
        self.json.write_message(&msg)?;
        self.begin_printed = true;
        Ok(())
    }
}

impl<'p, 's, M: Matcher, W: io::Write> Sink for JSONSink<'p, 's, M, W> {
    fn matched(
        &mut self,
        searcher: &Searcher,
        mat: &SinkMatch<'_>,
    ) -> io::Result<bool> {
        self.match_count += 1;
        self.write_begin_message()?;

        self.record_matches(
            searcher,
            mat.buffer(),
            mat.bytes_range_in_buffer(),
        )?;
        self.replace(searcher, mat.buffer(), mat.bytes_range_in_buffer())?;
        self.stats.add_matches(self.json.matches.len() as u64);
        self.stats.add_matched_lines(mat.lines().count() as u64);

        let submatches = SubMatches::new(
            mat.bytes(),
            &self.json.matches,
            self.replacer.replacement(),
        );
        let msg = lines_message(
            "match",
            self.path,
            mat.bytes(),
            mat.line_number(),
            mat.absolute_byte_offset(),
            submatches.as_slice(),
        );
        self.json.write_message(&msg)?;
        Ok(true)
    }

    fn context(
        &mut self,
        searcher: &Searcher,
        ctx: &SinkContext<'_>,
    ) -> io::Result<bool> {
        self.write_begin_message()?;
        self.json.matches.clear();

        let submatches = if searcher.invert_match() {
            self.record_matches(searcher, ctx.bytes(), 0..ctx.bytes().len())?;
            self.replace(searcher, ctx.bytes(), 0..ctx.bytes().len())?;
            SubMatches::new(
                ctx.bytes(),
                &self.json.matches,
                self.replacer.replacement(),
            )
        } else {
            SubMatches::empty()
        };
        let msg = lines_message(
            "context",
            self.path,
            ctx.bytes(),
            ctx.line_number(),
            ctx.absolute_byte_offset(),
            submatches.as_slice(),
        );
        self.json.write_message(&msg)?;
        Ok(true)
    }

    fn binary_data(
        &mut self,
        _searcher: &Searcher,
        _binary_byte_offset: u64,
    ) -> io::Result<bool> {
        Ok(true)
    }

    fn begin(&mut self, _searcher: &Searcher) -> io::Result<bool> {
        self.json.wtr.reset_count();
        self.start_time = crate::rg::main::Started::now();
        self.match_count = 0;
        self.binary_byte_offset = None;

        if !self.json.config.always_begin_end {
            return Ok(true);
        }
        self.write_begin_message()?;
        Ok(true)
    }

    fn finish(
        &mut self,
        _searcher: &Searcher,
        finish: &SinkFinish,
    ) -> io::Result<()> {
        self.binary_byte_offset = finish.binary_byte_offset();
        self.stats.add_elapsed(self.start_time.elapsed());
        self.stats.add_searches(1);
        if self.match_count > 0 {
            self.stats.add_searches_with_match(1);
        }
        self.stats.add_bytes_searched(finish.byte_count());
        self.stats.add_bytes_printed(self.json.wtr.count());

        if !self.begin_printed {
            return Ok(());
        }
        let msg = message(
            "end",
            Json::obj()
                .set("path", data_path(self.path))
                .set("binary_offset", opt_num(finish.binary_byte_offset()))
                .set("stats", stats(&self.stats)),
        );
        self.json.write_message(&msg)?;
        Ok(())
    }
}

/// SubMatches represents a set of matches in a contiguous range of bytes.
///
/// A simpler representation for this would just simply be `Vec<SubMatch>`,
/// but the common case is exactly one match per range of bytes, which we
/// specialize here using a fixed size array without any allocation.
enum SubMatches<'a> {
    Empty,
    Small([SubMatch<'a>; 1]),
    Big(Vec<SubMatch<'a>>),
}

impl<'a> SubMatches<'a> {
    /// Create a new set of match ranges from a set of matches and the
    /// corresponding bytes that those matches apply to.
    fn new(
        bytes: &'a [u8],
        matches: &[Match],
        replacement: Option<(&'a [u8], &'a [Match])>,
    ) -> SubMatches<'a> {
        if matches.len() == 1 {
            let mat = matches[0];
            SubMatches::Small([SubMatch {
                m: &bytes[mat],
                replacement: replacement
                    .map(|(rbuf, rmatches)| &rbuf[rmatches[0]]),
                start: mat.start(),
                end: mat.end(),
            }])
        } else {
            let mut match_ranges = vec![];
            for (i, &mat) in matches.iter().enumerate() {
                match_ranges.push(SubMatch {
                    m: &bytes[mat],
                    replacement: replacement
                        .map(|(rbuf, rmatches)| &rbuf[rmatches[i]]),
                    start: mat.start(),
                    end: mat.end(),
                });
            }
            SubMatches::Big(match_ranges)
        }
    }

    /// Create an empty set of match ranges.
    fn empty() -> SubMatches<'static> {
        SubMatches::Empty
    }

    /// Return this set of match ranges as a slice.
    fn as_slice(&self) -> &[SubMatch<'_>] {
        match *self {
            SubMatches::Empty => &[],
            SubMatches::Small(ref x) => x,
            SubMatches::Big(ref x) => x,
        }
    }
}

/// Encodes bytes as standard base64 (with padding), as grep-printer does.
fn base64_standard(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::new();
    let mut it = bytes.chunks_exact(3);
    while let Some(chunk) = it.next() {
        let group24 = (usize::from(chunk[0]) << 16)
            | (usize::from(chunk[1]) << 8)
            | usize::from(chunk[2]);
        let index1 = (group24 >> 18) & 0b111_111;
        let index2 = (group24 >> 12) & 0b111_111;
        let index3 = (group24 >> 6) & 0b111_111;
        let index4 = (group24 >> 0) & 0b111_111;
        out.push(char::from(ALPHABET[index1]));
        out.push(char::from(ALPHABET[index2]));
        out.push(char::from(ALPHABET[index3]));
        out.push(char::from(ALPHABET[index4]));
    }
    match it.remainder() {
        &[] => {}
        &[byte0] => {
            let group8 = usize::from(byte0);
            let index1 = (group8 >> 2) & 0b111_111;
            let index2 = (group8 << 4) & 0b111_111;
            out.push(char::from(ALPHABET[index1]));
            out.push(char::from(ALPHABET[index2]));
            out.push('=');
            out.push('=');
        }
        &[byte0, byte1] => {
            let group16 = (usize::from(byte0) << 8) | usize::from(byte1);
            let index1 = (group16 >> 10) & 0b111_111;
            let index2 = (group16 >> 4) & 0b111_111;
            let index3 = (group16 << 2) & 0b111_111;
            out.push(char::from(ALPHABET[index1]));
            out.push(char::from(ALPHABET[index2]));
            out.push(char::from(ALPHABET[index3]));
            out.push('=');
        }
        _ => unreachable!("remainder must have length < 3"),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_basic() {
        let b64 = |s: &str| base64_standard(s.as_bytes());
        assert_eq!(b64(""), "");
        assert_eq!(b64("f"), "Zg==");
        assert_eq!(b64("fo"), "Zm8=");
        assert_eq!(b64("foo"), "Zm9v");
        assert_eq!(b64("foob"), "Zm9vYg==");
        assert_eq!(b64("fooba"), "Zm9vYmE=");
        assert_eq!(b64("foobar"), "Zm9vYmFy");
    }

    #[test]
    fn escapes_as_serde_json() {
        let j = Json::obj().set("a", Json::Str("q\"\\\n\t\u{1}é/".into()));
        let mut out = Vec::new();
        j.write_line(&mut out).unwrap();
        assert_eq!(out, b"{\"a\":\"q\\\"\\\\\\n\\t\\u0001\xc3\xa9/\"}\n");
    }
}
