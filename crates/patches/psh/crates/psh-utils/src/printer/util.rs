use std::borrow::Cow;
use std::cell::OnceCell;
use std::io::{self, Write};
use std::ops::Range;
use std::path::Path;

use super::hyperlink::HyperlinkPath;
use crate::ansi::{ColorSpec, HyperlinkSpec, WriteColor};
use crate::matcher::{Captures, LineTerminator, Match, Matcher};
use crate::searcher::{LineIter, Searcher, SinkContext, SinkContextKind, SinkMatch};

pub(crate) const MAX_LOOK_AHEAD_BYTES: usize = 128;

const U64_DIGITS_MAX: usize = 20;

pub(crate) struct DecimalFormatter {
    digits: [u8; U64_DIGITS_MAX],
    start: usize,
}

impl DecimalFormatter {
    pub(crate) fn new(number: u64) -> DecimalFormatter {
        let mut digits = [0; U64_DIGITS_MAX];
        let mut start = U64_DIGITS_MAX;
        let mut rest = number;
        loop {
            assert!(start > 0, "{number} has more than {U64_DIGITS_MAX} digits");
            start -= 1;
            digits[start] = b'0' + (rest % 10) as u8;
            rest /= 10;
            if rest == 0 {
                return DecimalFormatter { digits, start };
            }
        }
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.digits[self.start..]
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CounterWriter<W> {
    writer: W,
    count: u64,
    total_count: u64,
}

impl<W> CounterWriter<W> {
    pub(crate) fn new(writer: W) -> CounterWriter<W> {
        CounterWriter {
            writer,
            count: 0,
            total_count: 0,
        }
    }

    #[inline]
    pub(crate) fn count(&self) -> u64 {
        self.count
    }

    #[inline]
    pub(crate) fn total_count(&self) -> u64 {
        self.total_count + self.count
    }

    #[inline]
    pub(crate) fn reset_count(&mut self) {
        self.total_count += self.count;
        self.count = 0;
    }

    #[inline]
    pub(crate) fn get_mut(&mut self) -> &mut W {
        &mut self.writer
    }
}

impl<W: Write> Write for CounterWriter<W> {
    #[inline(always)]
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.writer.write(buffer)?;
        self.count += written as u64;
        Ok(written)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl<W: WriteColor> WriteColor for CounterWriter<W> {
    #[inline]
    fn supports_color(&self) -> bool {
        self.writer.supports_color()
    }

    #[inline]
    fn supports_hyperlinks(&self) -> bool {
        self.writer.supports_hyperlinks()
    }

    #[inline]
    fn set_color(&mut self, spec: &ColorSpec) -> io::Result<()> {
        self.writer.set_color(spec)
    }

    #[inline]
    fn set_hyperlink(&mut self, link: &HyperlinkSpec<'_>) -> io::Result<()> {
        self.writer.set_hyperlink(link)
    }

    #[inline]
    fn reset(&mut self) -> io::Result<()> {
        self.writer.reset()
    }
}

struct ReplacerSpace<M: Matcher> {
    captures: M::Captures,
    destination: Vec<u8>,
    matches: Vec<Match>,
}

pub(crate) struct Replacer<M: Matcher> {
    space: Option<ReplacerSpace<M>>,
}

impl<M: Matcher> Replacer<M> {
    pub(crate) fn new() -> Replacer<M> {
        Replacer { space: None }
    }

    pub(crate) fn replace_all(
        &mut self,
        searcher: &Searcher,
        matcher: &M,
        haystack: &[u8],
        range: Range<usize>,
        replacement: &[u8],
    ) {
        let (haystack, line_terminator) = if searcher.multi_line_with_matcher(matcher) {
            (look_ahead_window(haystack, range.end), &[][..])
        } else {
            let mut line = Match::new(0, range.end);
            let line_terminator = trim_line_terminator(searcher, haystack, &mut line);
            (&haystack[..line.end()], line_terminator)
        };
        let space = self.space.get_or_insert_with(|| ReplacerSpace {
            captures: matcher.new_captures(),
            destination: Vec::new(),
            matches: Vec::new(),
        });
        space.destination.clear();
        space.matches.clear();
        let ReplacerSpace {
            captures,
            destination,
            matches,
        } = space;
        let mut last_match = range.start;
        matcher.captures_iter_at(haystack, range.start, captures, |captures| {
            let found = captures.get(0).expect("a capture match has group 0");
            if found.start() >= range.end {
                return false;
            }
            destination.extend_from_slice(&haystack[last_match..found.start()]);
            last_match = found.end();
            let start = destination.len();
            captures.interpolate(
                |name| matcher.capture_index(name),
                haystack,
                replacement,
                destination,
            );
            matches.push(Match::new(start, destination.len()));
            true
        });
        let end = if last_match > range.end {
            haystack.len()
        } else {
            haystack.len().min(range.end)
        };
        destination.extend_from_slice(&haystack[last_match..end]);
        destination.extend_from_slice(line_terminator);
    }

    pub(crate) fn replacement(&self) -> Option<(&[u8], &[Match])> {
        let space = self.space.as_ref()?;
        if space.matches.is_empty() {
            None
        } else {
            Some((&space.destination, &space.matches))
        }
    }

    pub(crate) fn clear(&mut self) {
        if let Some(space) = self.space.as_mut() {
            space.destination.clear();
            space.matches.clear();
        }
    }
}

impl<M: Matcher> std::fmt::Debug for Replacer<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (destination, matches) = self.replacement().unwrap_or((&[], &[]));
        f.debug_struct("Replacer")
            .field("destination", &destination)
            .field("matches", &matches)
            .finish()
    }
}

fn look_ahead_window(haystack: &[u8], end: usize) -> &[u8] {
    if haystack.len() - end >= MAX_LOOK_AHEAD_BYTES {
        &haystack[..end + MAX_LOOK_AHEAD_BYTES]
    } else {
        haystack
    }
}

#[derive(Debug)]
pub(crate) struct Sunk<'a> {
    bytes: &'a [u8],
    absolute_byte_offset: u64,
    line_number: Option<u64>,
    context_kind: Option<SinkContextKind>,
    matches: &'a [Match],
    original_matches: &'a [Match],
}

impl<'a> Sunk<'a> {
    #[inline]
    pub(crate) fn empty() -> Sunk<'static> {
        Sunk {
            bytes: &[],
            absolute_byte_offset: 0,
            line_number: None,
            context_kind: None,
            matches: &[],
            original_matches: &[],
        }
    }

    #[inline]
    pub(crate) fn from_sink_match(
        sunk: &SinkMatch<'a>,
        original_matches: &'a [Match],
        replacement: Option<(&'a [u8], &'a [Match])>,
    ) -> Sunk<'a> {
        let (bytes, matches) = replacement.unwrap_or((sunk.bytes(), original_matches));
        Sunk {
            bytes,
            absolute_byte_offset: sunk.absolute_byte_offset(),
            line_number: sunk.line_number(),
            context_kind: None,
            matches,
            original_matches,
        }
    }

    #[inline]
    pub(crate) fn from_sink_context(
        sunk: &SinkContext<'a>,
        original_matches: &'a [Match],
        replacement: Option<(&'a [u8], &'a [Match])>,
    ) -> Sunk<'a> {
        let (bytes, matches) = replacement.unwrap_or((sunk.bytes(), original_matches));
        Sunk {
            bytes,
            absolute_byte_offset: sunk.absolute_byte_offset(),
            line_number: sunk.line_number(),
            context_kind: Some(sunk.kind()),
            matches,
            original_matches,
        }
    }

    #[inline]
    pub(crate) fn context_kind(&self) -> Option<SinkContextKind> {
        self.context_kind
    }

    #[inline]
    pub(crate) fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    #[inline]
    pub(crate) fn matches(&self) -> &'a [Match] {
        self.matches
    }

    #[inline]
    pub(crate) fn original_matches(&self) -> &'a [Match] {
        self.original_matches
    }

    #[inline]
    pub(crate) fn lines(&self, line_terminator: u8) -> LineIter<'a> {
        LineIter::new(line_terminator, self.bytes)
    }

    #[inline]
    pub(crate) fn absolute_byte_offset(&self) -> u64 {
        self.absolute_byte_offset
    }

    #[inline]
    pub(crate) fn line_number(&self) -> Option<u64> {
        self.line_number
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PrinterPath<'a> {
    path: &'a Path,
    bytes: Cow<'a, [u8]>,
    hyperlink: OnceCell<Option<HyperlinkPath>>,
}

impl<'a> PrinterPath<'a> {
    pub(crate) fn new(path: &'a Path) -> PrinterPath<'a> {
        PrinterPath {
            path,
            bytes: path_bytes(path),
            hyperlink: OnceCell::new(),
        }
    }

    pub(crate) fn with_separator(mut self, separator: Option<u8>) -> PrinterPath<'a> {
        let Some(separator) = separator else {
            return self;
        };
        let replaced = self
            .bytes
            .iter()
            .map(|&byte| {
                if byte == b'/' || (cfg!(windows) && byte == b'\\') {
                    separator
                } else {
                    byte
                }
            })
            .collect();
        self.bytes = Cow::Owned(replaced);
        self
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn as_hyperlink(&self) -> Option<&HyperlinkPath> {
        self.hyperlink
            .get_or_init(|| HyperlinkPath::from_path(&self.as_path()))
            .as_ref()
    }

    fn as_path(&self) -> Cow<'_, Path> {
        bytes_path(&self.bytes, self.path)
    }
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    Cow::Borrowed(path.as_os_str().as_bytes())
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Cow<'_, [u8]> {
    match path.to_string_lossy() {
        Cow::Borrowed(text) => Cow::Borrowed(text.as_bytes()),
        Cow::Owned(text) => Cow::Owned(text.into_bytes()),
    }
}

#[cfg(unix)]
fn bytes_path<'p>(bytes: &'p [u8], _: &Path) -> Cow<'p, Path> {
    use std::os::unix::ffi::OsStrExt;
    Cow::Borrowed(Path::new(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn bytes_path<'p>(_: &[u8], path: &'p Path) -> Cow<'p, Path> {
    Cow::Borrowed(path)
}

pub(crate) fn trim_ascii_prefix(
    line_terminator: LineTerminator,
    slice: &[u8],
    range: Match,
) -> Match {
    let is_space = |byte: u8| matches!(byte, b'\t' | b'\n' | b'\x0B' | b'\x0C' | b'\r' | b' ');
    let count = slice[range]
        .iter()
        .take_while(|&&byte| is_space(byte) && !line_terminator.as_bytes().contains(&byte))
        .count();
    range.with_start(range.start() + count)
}

pub(crate) fn find_iter_at_in_context<M, F>(
    searcher: &Searcher,
    matcher: M,
    bytes: &[u8],
    range: Range<usize>,
    mut matched: F,
) where
    M: Matcher,
    F: FnMut(Match) -> bool,
{
    let bytes = if searcher.multi_line_with_matcher(&matcher) {
        look_ahead_window(bytes, range.end)
    } else {
        let mut line = Match::new(0, range.end);
        trim_line_terminator(searcher, bytes, &mut line);
        &bytes[..line.end()]
    };
    matcher.find_iter_at(bytes, range.start, |found| {
        if found.start() >= range.end {
            return false;
        }
        matched(found)
    });
}

pub(crate) fn trim_line_terminator<'b>(
    searcher: &Searcher,
    buffer: &'b [u8],
    line: &mut Match,
) -> &'b [u8] {
    let line_terminator = searcher.line_terminator();
    if !line_terminator.is_suffix(&buffer[*line]) {
        return &[];
    }
    let mut end = line.end() - 1;
    if line_terminator.is_crlf() && end > 0 && buffer.get(end - 1) == Some(&b'\r') {
        end -= 1;
    }
    let original_end = line.end();
    *line = line.with_end(end);
    &buffer[end..original_end]
}
