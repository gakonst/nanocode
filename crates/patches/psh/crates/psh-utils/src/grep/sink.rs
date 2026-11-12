//! GNU grep's output (D1137): a searcher sink that prints selected and context
//! lines as GNU does (`FILE:LINE:OFFSET:text`, `-` for context, `--` between groups,
//! `-o` spans, GREP_COLORS), and the buffered stdout it writes to.

use std::cell::RefCell;
use std::io::{self, Write};

use psh_engine::util::SyncWriter;

use super::pattern::{Pats, char_len};
use super::{BinaryFiles, Colors, Config};
use crate::searcher::{Searcher, Sink, SinkContext, SinkMatch};

/// stdout through a buffer (flushed when full, at every line with `--line-buffered`
/// or on a terminal, and before stderr is written, D1081), and stderr.
pub(crate) struct Out {
    stdout: SyncWriter,
    stderr: SyncWriter,
    buf: Vec<u8>,
    line_flush: bool,
    failed: Option<io::ErrorKind>,
    /// Some group of lines was printed (in any file): the next one is separated.
    pub printed_group: bool,
}

const BUFFER: usize = 64 * 1024;

impl Out {
    pub(crate) fn new(stdout: SyncWriter, stderr: SyncWriter, line_flush: bool) -> Out {
        Out {
            stdout,
            stderr,
            buf: Vec::new(),
            line_flush,
            failed: None,
            printed_group: false,
        }
    }

    pub(crate) fn write(&mut self, data: &[u8]) -> io::Result<()> {
        if let Some(kind) = self.failed {
            return Err(kind.into());
        }
        self.buf.extend_from_slice(data);
        if self.buf.len() >= BUFFER {
            self.flush()?;
        }
        Ok(())
    }

    fn end_line(&mut self) -> io::Result<()> {
        if self.line_flush {
            self.flush()?;
        }
        Ok(())
    }

    pub(crate) fn flush(&mut self) -> io::Result<()> {
        if let Some(kind) = self.failed {
            return Err(kind.into());
        }
        if self.buf.is_empty() {
            return Ok(());
        }
        let data = std::mem::take(&mut self.buf);
        self.stdout.write_all(&data).inspect_err(|e| {
            self.failed = Some(e.kind());
        })
    }

    /// `grep: MSG` on stderr, after what stdout has.
    pub(crate) fn error(&mut self, msg: &str) {
        let _ = self.flush();
        let _ = self.stderr.write_all(format!("grep: {msg}\n").as_bytes());
    }

    /// The first failed write to stdout.
    pub(crate) fn failed(&self) -> Option<io::ErrorKind> {
        self.failed
    }

    /// `text` in the color `cap` (none when empty or colors are off).
    pub(crate) fn colored(
        &mut self,
        colors: Option<&Colors>,
        cap: &str,
        text: &[u8],
    ) -> io::Result<()> {
        match colors {
            Some(c) if !cap.is_empty() => {
                self.sgr_start(c, cap)?;
                self.write(text)?;
                self.sgr_end(c)
            }
            _ => self.write(text),
        }
    }

    fn sgr_start(&mut self, c: &Colors, cap: &str) -> io::Result<()> {
        let k = if c.ne { "" } else { "\x1b[K" };
        self.write(format!("\x1b[{cap}m{k}").as_bytes())
    }

    fn sgr_end(&mut self, c: &Colors) -> io::Result<()> {
        self.write(if c.ne { b"\x1b[m" } else { b"\x1b[m\x1b[K" })
    }
}

/// One file's search.
pub(crate) struct GrepSink<'a> {
    cfg: &'a Config,
    spans: Option<&'a Pats>,
    out: &'a RefCell<Out>,
    name: &'a [u8],
    show_name: bool,
    /// Selected lines.
    pub count: u64,
    /// A NUL was seen: nothing more is printed.
    pub binary_seen: bool,
    /// `-I` and the file is binary: it does not match.
    pub binary_skip: bool,
    /// Output was left out (binary data): `binary file matches` follows.
    pub suppressed: bool,
    last_line: Option<u64>,
}

impl<'a> GrepSink<'a> {
    pub(crate) fn new(
        cfg: &'a Config,
        spans: Option<&'a Pats>,
        out: &'a RefCell<Out>,
        name: &'a [u8],
        show_name: bool,
    ) -> GrepSink<'a> {
        GrepSink {
            cfg,
            spans,
            out,
            name,
            show_name,
            count: 0,
            binary_seen: false,
            binary_skip: false,
            suppressed: false,
            last_line: None,
        }
    }

    fn term(&self) -> u8 {
        if self.cfg.null_data { b'\0' } else { b'\n' }
    }

    /// A selected or context line, with its terminator.
    fn line(
        &mut self,
        selected: bool,
        bytes: &[u8],
        lnum: Option<u64>,
        offset: u64,
    ) -> io::Result<()> {
        let cfg = self.cfg;
        let term = self.term();
        let raw = bytes.strip_suffix(&[term]).unwrap_or(bytes);
        let mut out = self.out.borrow_mut();
        let colors = cfg.colors.as_ref();
        if cfg.context {
            let adjacent = matches!((self.last_line, lnum), (Some(l), Some(n)) if n == l + 1);
            if !adjacent
                && out.printed_group
                && let Some(sep) = &cfg.group_separator
            {
                out.colored(colors, colors.map_or("", |c| &c.se), sep)?;
                out.write(b"\n")?;
            }
            self.last_line = lnum;
            out.printed_group = true;
        }
        let checks_encoding = cfg.utf8 && cfg.binary == BinaryFiles::Binary;
        let sep = if selected { b':' } else { b'-' };
        if cfg.only_matching {
            if !selected {
                return Ok(());
            }
            let pats = self.spans.expect("-o has spans");
            let content = pats.content(raw);
            for (s, e) in spans_of(pats, content, cfg.utf8) {
                let text = &content[s..e];
                if checks_encoding && std::str::from_utf8(text).is_err() {
                    self.suppressed = true;
                    continue;
                }
                self.prefix(&mut out, sep, lnum, offset + s as u64)?;
                out.colored(colors, colors.map_or("", |c| &c.ms), text)?;
                out.write(&[term])?;
                out.end_line()?;
            }
            return Ok(());
        }
        if checks_encoding && std::str::from_utf8(raw).is_err() {
            self.suppressed = true;
            return Ok(());
        }
        self.prefix(&mut out, sep, lnum, offset)?;
        match (colors, self.spans) {
            (Some(c), Some(pats)) => self.highlighted(&mut out, c, pats, selected, raw)?,
            _ => out.write(raw)?,
        }
        out.write(&[term])?;
        out.end_line()
    }

    /// `FILE:LINE:OFFSET:` (`-` for context), as asked for.
    fn prefix(&self, out: &mut Out, sep: u8, lnum: Option<u64>, offset: u64) -> io::Result<()> {
        let cfg = self.cfg;
        let colors = cfg.colors.as_ref();
        let cap = |f: fn(&Colors) -> &str| colors.map_or("", f);
        if self.show_name {
            out.colored(colors, cap(|c| &c.fname), self.name)?;
            if cfg.null {
                out.write(b"\0")?;
            } else {
                out.colored(colors, cap(|c| &c.se), &[sep])?;
            }
        }
        if cfg.line_number {
            let n = lnum.unwrap_or(0).to_string();
            out.colored(colors, cap(|c| &c.ln), n.as_bytes())?;
            out.colored(colors, cap(|c| &c.se), &[sep])?;
        }
        if cfg.byte_offset {
            out.colored(colors, cap(|c| &c.bn), offset.to_string().as_bytes())?;
            out.colored(colors, cap(|c| &c.se), &[sep])?;
        }
        Ok(())
    }

    /// A line with its matches colored (in selected lines, or in context lines with
    /// `-v`, where those are the matching ones) and the line's own color around.
    fn highlighted(
        &self,
        out: &mut Out,
        c: &Colors,
        pats: &Pats,
        selected: bool,
        raw: &[u8],
    ) -> io::Result<()> {
        let invert = self.cfg.invert;
        let (line_cap, match_cap) = if selected {
            (if invert && c.rv { &c.cx } else { &c.sl }, &c.ms)
        } else {
            (if invert && c.rv { &c.sl } else { &c.cx }, &c.mc)
        };
        let content = pats.content(raw);
        let tail = &raw[content.len()..];
        if !line_cap.is_empty() {
            out.sgr_start(c, line_cap)?;
        }
        let mut pos = 0;
        if selected != invert && !match_cap.is_empty() {
            for (s, e) in spans_of(pats, content, self.cfg.utf8) {
                out.write(&content[pos..s])?;
                out.sgr_start(c, match_cap)?;
                out.write(&content[s..e])?;
                out.sgr_end(c)?;
                if !line_cap.is_empty() && e < content.len() {
                    out.sgr_start(c, line_cap)?;
                }
                pos = e;
            }
        }
        out.write(&content[pos..])?;
        if !line_cap.is_empty() && (pos < content.len() || pos == 0) {
            out.sgr_end(c)?;
        }
        out.write(tail)
    }
}

/// The non-empty matches of a line, GNU's way: after a match the search goes on at
/// its end; an empty match is skipped by one character.
fn spans_of(pats: &Pats, line: &[u8], utf8: bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some((s, e)) = pats.find_at(line, at) {
        if s == e {
            at = s + char_len(line, s, utf8);
            if at > line.len() {
                break;
            }
            continue;
        }
        out.push((s, e));
        at = e;
    }
    out
}

impl Sink for GrepSink<'_> {
    fn matched(&mut self, _: &Searcher, m: &SinkMatch<'_>) -> Result<bool, io::Error> {
        self.count += 1;
        let cfg = self.cfg;
        if cfg.quiet || cfg.discard || cfg.list.is_some() {
            return Ok(false);
        }
        if cfg.count {
            return Ok(true);
        }
        if self.binary_seen {
            self.suppressed = true;
            return Ok(false);
        }
        self.line(true, m.bytes(), m.line_number(), m.absolute_byte_offset())?;
        Ok(true)
    }

    fn context(&mut self, _: &Searcher, c: &SinkContext<'_>) -> Result<bool, io::Error> {
        if self.binary_seen {
            return Ok(true);
        }
        self.line(false, c.bytes(), c.line_number(), c.absolute_byte_offset())?;
        Ok(true)
    }

    fn binary_data(&mut self, _: &Searcher, _offset: u64) -> Result<bool, io::Error> {
        match self.cfg.binary {
            BinaryFiles::Binary => {
                self.binary_seen = true;
                Ok(true)
            }
            BinaryFiles::WithoutMatch => {
                self.binary_skip = true;
                Ok(false)
            }
            BinaryFiles::Text => Ok(true),
        }
    }
}
