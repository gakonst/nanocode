// The JSON printer below (`JSONBuilder`, `JSON`, `JSONSink`, `SubMatches`, the message
// types and `base64_standard`) is copied from grep-printer 0.3.1 (`src/json.rs`,
// `src/jsont.rs`), with `CounterWriter` (`src/counter.rs`) and `Replacer`,
// `find_iter_at_in_context`, `trim_line_terminator` and
// `replace_with_captures_in_context` (`src/util.rs`), in ripgrep
// (https://github.com/BurntSushi/ripgrep): Copyright (c) 2015 Andrew Gallant.
// Licensed under the MIT License (see NOTICE).

//! `rg --json` (D029, D1136): grep-printer's JSON printer, whose serialization needs
//! serde, with the messages built as a small JSON tree written by deser-json. The
//! output is grep-printer's byte for byte: the same fields in the same order, compact,
//! `\n` after each message, `{"text":..}` or base64 `{"bytes":..}` data.

use std::{
    borrow::Cow,
    fmt,
    io::{self, Write},
    path::Path,
    sync::Arc,
    time::Duration,
};

use deser_core::ser::{
    Emit, SeqEmitter, Serialize, SerializeHandle, StructEmitter,
};
use deser_core::{Atom, Error, State};
use grep_matcher::{Match, Matcher};
use grep_printer::Stats;
use grep_searcher::{
    Searcher, Sink, SinkContext, SinkError, SinkFinish, SinkMatch,
};

/// grep-printer's bound on how far past a match a multi-line search looks.
const MAX_LOOK_AHEAD: usize = 128;

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
        let s = deser_json::to_string(self).map_err(io::Error::other)?;
        w.write_all(s.as_bytes())?;
        w.write_all(b"\n")
    }
}

impl Serialize for Json {
    fn serialize<'a>(
        value: &'a Self,
        state: &mut State,
    ) -> Result<Emit<'a>, Error> {
        Ok(match value {
            Json::Null => Emit::Atom(Atom::Null),
            Json::Num(n) => Emit::Atom(Atom::U64(*n)),
            Json::Str(s) => return String::serialize(s, state),
            Json::Arr(items) => Emit::seq(Items { items, index: 0 }, state),
            Json::Obj(fields) => {
                Emit::structure(Fields { fields, index: 0 }, state)
            }
        })
    }
}

struct Items<'a> {
    items: &'a [Json],
    index: usize,
}

impl SeqEmitter for Items<'_> {
    fn next(
        &mut self,
        _state: &mut State,
    ) -> Result<Option<SerializeHandle<'_>>, Error> {
        let item = self.items.get(self.index);
        self.index += 1;
        Ok(item.map(SerializeHandle::to))
    }
}

struct Fields<'a> {
    fields: &'a [(&'static str, Json)],
    index: usize,
}

impl StructEmitter for Fields<'_> {
    fn next(
        &mut self,
        _state: &mut State,
    ) -> Result<Option<(Cow<'_, str>, SerializeHandle<'_>)>, Error> {
        let field = self.fields.get(self.index);
        self.index += 1;
        Ok(field.map(|(k, v)| (Cow::Borrowed(*k), SerializeHandle::to(v))))
    }
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
        )?;
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
            )?;
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
    type Error = io::Error;

    fn matched(
        &mut self,
        searcher: &Searcher,
        mat: &SinkMatch<'_>,
    ) -> Result<bool, io::Error> {
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
    ) -> Result<bool, io::Error> {
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
    ) -> Result<bool, io::Error> {
        Ok(true)
    }

    fn begin(&mut self, _searcher: &Searcher) -> Result<bool, io::Error> {
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
    ) -> Result<(), io::Error> {
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

/// A writer that counts the number of bytes that have been successfully
/// written.
#[derive(Clone, Debug)]
struct CounterWriter<W> {
    wtr: W,
    count: u64,
    total_count: u64,
}

impl<W: Write> CounterWriter<W> {
    fn new(wtr: W) -> CounterWriter<W> {
        CounterWriter { wtr, count: 0, total_count: 0 }
    }
}

impl<W> CounterWriter<W> {
    /// Returns the total number of bytes written since construction or the
    /// last time `reset` was called.
    fn count(&self) -> u64 {
        self.count
    }

    /// Resets the number of bytes written to `0`.
    fn reset_count(&mut self) {
        self.total_count += self.count;
        self.count = 0;
    }

    fn get_mut(&mut self) -> &mut W {
        &mut self.wtr
    }
}

impl<W: Write> Write for CounterWriter<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, io::Error> {
        let n = self.wtr.write(buf)?;
        self.count += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> Result<(), io::Error> {
        self.wtr.flush()
    }
}

/// A type for handling replacements while amortizing allocation.
struct Replacer<M: Matcher> {
    space: Option<Space<M>>,
}

struct Space<M: Matcher> {
    /// The place to store capture locations.
    caps: M::Captures,
    /// The place to write a replacement to.
    dst: Vec<u8>,
    /// The place to store match offsets in terms of `dst`.
    matches: Vec<Match>,
}

impl<M: Matcher> fmt::Debug for Replacer<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (dst, matches) = self.replacement().unwrap_or((&[], &[]));
        f.debug_struct("Replacer")
            .field("dst", &dst)
            .field("matches", &matches)
            .finish()
    }
}

impl<M: Matcher> Replacer<M> {
    /// Create a new replacer for use with a particular matcher.
    fn new() -> Replacer<M> {
        Replacer { space: None }
    }

    /// Executes a replacement on the given haystack string by replacing all
    /// matches with the given replacement. To access the result of the
    /// replacement, use the `replacement` method.
    ///
    /// This can fail if the underlying matcher reports an error.
    fn replace_all<'a>(
        &'a mut self,
        searcher: &Searcher,
        matcher: &M,
        mut haystack: &[u8],
        range: std::ops::Range<usize>,
        replacement: &[u8],
    ) -> io::Result<()> {
        // See the giant comment in 'find_iter_at_in_context' below for why we
        // do this dance.
        let is_multi_line = searcher.multi_line_with_matcher(&matcher);
        // Get the line_terminator that was removed (if any) so we can add it
        // back.
        let line_terminator = if is_multi_line {
            if haystack[range.end..].len() >= MAX_LOOK_AHEAD {
                haystack = &haystack[..range.end + MAX_LOOK_AHEAD];
            }
            &[]
        } else {
            // When searching a single line, we should remove the line
            // terminator. Otherwise, it's possible for the regex (via
            // look-around) to observe the line terminator and not match
            // because of it.
            let mut m = Match::new(0, range.end);
            let line_terminator =
                trim_line_terminator(searcher, haystack, &mut m);
            haystack = &haystack[..m.end()];
            line_terminator
        };
        {
            let &mut Space { ref mut dst, ref mut caps, ref mut matches } =
                self.allocate(matcher)?;
            dst.clear();
            matches.clear();

            replace_with_captures_in_context(
                matcher,
                haystack,
                line_terminator,
                range.clone(),
                caps,
                dst,
                |caps, dst| {
                    let start = dst.len();
                    grep_matcher::Captures::interpolate(
                        caps,
                        |name| matcher.capture_index(name),
                        haystack,
                        replacement,
                        dst,
                    );
                    let end = dst.len();
                    matches.push(Match::new(start, end));
                    true
                },
            )
            .map_err(io::Error::error_message)?;
        }
        Ok(())
    }

    /// Return the result of the prior replacement and the match offsets for
    /// all replacement occurrences within the returned replacement buffer.
    ///
    /// If no replacement has occurred then `None` is returned.
    fn replacement<'a>(&'a self) -> Option<(&'a [u8], &'a [Match])> {
        match self.space {
            None => None,
            Some(ref space) => {
                if space.matches.is_empty() {
                    None
                } else {
                    Some((&space.dst, &space.matches))
                }
            }
        }
    }

    /// Clear space used for performing a replacement.
    fn clear(&mut self) {
        if let Some(ref mut space) = self.space {
            space.dst.clear();
            space.matches.clear();
        }
    }

    /// Allocate space for replacements when used with the given matcher and
    /// return a mutable reference to that space.
    fn allocate(&mut self, matcher: &M) -> io::Result<&mut Space<M>> {
        if self.space.is_none() {
            let caps =
                matcher.new_captures().map_err(io::Error::error_message)?;
            self.space = Some(Space { caps, dst: vec![], matches: vec![] });
        }
        Ok(self.space.as_mut().unwrap())
    }
}

/// Like `Matcher::find_iter_at`, but accepts an end bound (see grep-printer's
/// comment there: the regex may look past the match in multi-line mode, so it
/// gets the rest of the buffer, capped).
fn find_iter_at_in_context<M, F>(
    searcher: &Searcher,
    matcher: M,
    mut bytes: &[u8],
    range: std::ops::Range<usize>,
    mut matched: F,
) -> io::Result<()>
where
    M: Matcher,
    F: FnMut(Match) -> bool,
{
    let is_multi_line = searcher.multi_line_with_matcher(&matcher);
    if is_multi_line {
        if bytes[range.end..].len() >= MAX_LOOK_AHEAD {
            bytes = &bytes[..range.end + MAX_LOOK_AHEAD];
        }
    } else {
        // When searching a single line, we should remove the line terminator.
        // Otherwise, it's possible for the regex (via look-around) to observe
        // the line terminator and not match because of it.
        let mut m = Match::new(0, range.end);
        // No need to rember the line terminator as we aren't doing a replace
        // here.
        trim_line_terminator(searcher, bytes, &mut m);
        bytes = &bytes[..m.end()];
    }
    matcher
        .find_iter_at(bytes, range.start, |m| {
            if m.start() >= range.end {
                return false;
            }
            matched(m)
        })
        .map_err(io::Error::error_message)
}

/// Given a buf and some bounds, if there is a line terminator at the end of
/// the given bounds in buf, then the bounds are trimmed to remove the line
/// terminator, returning the slice of the removed line terminator (if any).
fn trim_line_terminator<'b>(
    searcher: &Searcher,
    buf: &'b [u8],
    line: &mut Match,
) -> &'b [u8] {
    let lineterm = searcher.line_terminator();
    if lineterm.is_suffix(&buf[*line]) {
        let mut end = line.end() - 1;
        if lineterm.is_crlf() && end > 0 && buf.get(end - 1) == Some(&b'\r') {
            end -= 1;
        }
        let orig_end = line.end();
        *line = line.with_end(end);
        &buf[end..orig_end]
    } else {
        &[]
    }
}

/// Like `Matcher::replace_with_captures_at`, but accepts an end bound.
///
/// See also: `find_iter_at_in_context` for why we need this.
fn replace_with_captures_in_context<M, F>(
    matcher: M,
    bytes: &[u8],
    line_terminator: &[u8],
    range: std::ops::Range<usize>,
    caps: &mut M::Captures,
    dst: &mut Vec<u8>,
    mut append: F,
) -> Result<(), M::Error>
where
    M: Matcher,
    F: FnMut(&M::Captures, &mut Vec<u8>) -> bool,
{
    use grep_matcher::Captures;

    let mut last_match = range.start;
    matcher.captures_iter_at(bytes, range.start, caps, |caps| {
        let m = caps.get(0).unwrap();
        if m.start() >= range.end {
            return false;
        }
        dst.extend(&bytes[last_match..m.start()]);
        last_match = m.end();
        append(caps, dst)
    })?;
    let end = if last_match > range.end {
        bytes.len()
    } else {
        std::cmp::min(bytes.len(), range.end)
    };
    dst.extend(&bytes[last_match..end]);
    // Add back any line terminator.
    dst.extend(line_terminator);
    Ok(())
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
