//! awk's input and output: records split by RS from stdin, files and commands, and the
//! output streams of `print > file`, `>> file` and `| command`.

use std::io;
use std::rc::Rc;

use psh_engine::util::{ShellChild, UtilCtx, UtilFile};
use psh_regex::{ExecOptions, Regex};

/// How RS separates records.
#[derive(Clone)]
pub enum Rs {
    /// One byte (the default newline).
    Byte(u8),
    /// One multibyte character.
    Seq(Vec<u8>),
    /// RS = "": blank lines; newlines also separate fields.
    Paragraph,
    /// Longer RS: a regex (gawk).
    Regex(Rc<Regex>),
}

pub enum Src {
    Stdin,
    File(UtilFile),
    Cmd(ShellChild),
}

const CHUNK: usize = 64 * 1024;

pub struct RecordReader {
    pub src: Src,
    buf: Vec<u8>,
    pos: usize,
    eof: bool,
    /// The terminator given last: the next record shares it while RS finds the same.
    rt: Rc<[u8]>,
}

impl RecordReader {
    pub fn new(src: Src) -> RecordReader {
        RecordReader {
            src,
            buf: Vec::new(),
            pos: 0,
            eof: false,
            rt: Rc::from(&b""[..]),
        }
    }

    /// Reads more; false at end of input.
    async fn fill(&mut self, ctx: &mut UtilCtx<'_>) -> io::Result<bool> {
        if self.eof {
            return Ok(false);
        }
        if self.pos > 0 && self.pos * 2 >= self.buf.len() {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        let old = self.buf.len();
        self.buf.resize(old + CHUNK, 0);
        let r = match &mut self.src {
            Src::Stdin => ctx.read(&mut self.buf[old..]).await,
            Src::File(f) => f.read(&mut self.buf[old..]).await,
            Src::Cmd(c) => match &c.stdout {
                Some(out) => out.read(&mut self.buf[old..]).await,
                None => Ok(0),
            },
        };
        match r {
            Ok(n) => {
                self.buf.truncate(old + n);
                if n == 0 {
                    self.eof = true;
                }
                Ok(n > 0)
            }
            Err(e) => {
                self.buf.truncate(old);
                self.eof = true;
                Err(e)
            }
        }
    }

    /// [`Self::read`] for a record that ends within what is already read, when RS is
    /// one byte: there is nothing to wait for, so no future is made for the record.
    pub fn buffered(&mut self, rs: &Rs, rec: &mut Vec<u8>) -> Option<Rc<[u8]>> {
        let Rs::Byte(sep) = rs else {
            return None;
        };
        let at = self.pos + bstr::ByteSlice::find_byte(&self.buf[self.pos..], *sep)?;
        rec.clear();
        rec.extend_from_slice(&self.buf[self.pos..at]);
        self.pos = at + 1;
        if !matches!(&*self.rt, [had] if had == sep) {
            self.rt = Rc::from(&[*sep][..]);
        }
        Some(self.rt.clone())
    }

    /// The next record into `rec` (what it held is replaced), giving its terminator
    /// (RT); None at the end. The caller's buffer is filled, so a record costs no
    /// allocation of its own.
    pub async fn read(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        rs: &Rs,
        rec: &mut Vec<u8>,
    ) -> io::Result<Option<Rc<[u8]>>> {
        rec.clear();
        match rs {
            Rs::Byte(b) => self.read_seq(ctx, &[*b], rec).await,
            Rs::Seq(s) => self.read_seq(ctx, s, rec).await,
            Rs::Paragraph => self.read_paragraph(ctx, rec).await,
            Rs::Regex(re) => self.read_regex(ctx, re, rec).await,
        }
    }

    async fn read_seq(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        sep: &[u8],
        rec: &mut Vec<u8>,
    ) -> io::Result<Option<Rc<[u8]>>> {
        let mut searched = self.pos;
        loop {
            let hay = &self.buf[searched..];
            let found = if sep.len() == 1 {
                bstr::ByteSlice::find_byte(hay, sep[0])
            } else {
                hay.windows(sep.len()).position(|w| w == sep)
            };
            if let Some(i) = found {
                let at = searched + i;
                rec.extend_from_slice(&self.buf[self.pos..at]);
                self.pos = at + sep.len();
                // One byte against one byte needs no call.
                let same = match (&*self.rt, sep) {
                    ([had], [found]) => had == found,
                    (had, found) => had == found,
                };
                if !same {
                    self.rt = Rc::from(sep);
                }
                return Ok(Some(self.rt.clone()));
            }
            searched = self.buf.len().saturating_sub(sep.len() - 1).max(self.pos);
            let base = self.pos;
            if !self.fill(ctx).await? {
                if self.pos < self.buf.len() {
                    rec.extend_from_slice(&self.buf[self.pos..]);
                    self.pos = self.buf.len();
                    return Ok(Some(Rc::from(&b""[..])));
                }
                return Ok(None);
            }
            // `fill` may have moved the data to the front.
            searched -= base - self.pos;
        }
    }

    async fn read_paragraph(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        rec: &mut Vec<u8>,
    ) -> io::Result<Option<Rc<[u8]>>> {
        // Leading newlines are skipped.
        loop {
            while self.pos < self.buf.len() && self.buf[self.pos] == b'\n' {
                self.pos += 1;
            }
            if self.pos < self.buf.len() {
                break;
            }
            if !self.fill(ctx).await? {
                return Ok(None);
            }
        }
        let mut searched = self.pos;
        loop {
            let hay = &self.buf[searched..];
            if let Some(i) = hay.windows(2).position(|w| w == b"\n\n") {
                let at = searched + i;
                // The terminator is the whole run of newlines: read on until it ends.
                let mut end = at;
                loop {
                    while end < self.buf.len() && self.buf[end] == b'\n' {
                        end += 1;
                    }
                    if end < self.buf.len() {
                        break;
                    }
                    let base = self.pos;
                    let more = self.fill(ctx).await?;
                    let moved = base - self.pos;
                    end -= moved;
                    searched -= moved;
                    if !more {
                        break;
                    }
                }
                let at = searched + i;
                rec.extend_from_slice(&self.buf[self.pos..at]);
                let rt = Rc::from(&self.buf[at..end]);
                self.pos = end;
                return Ok(Some(rt));
            }
            searched = self.buf.len().saturating_sub(1).max(self.pos);
            let base = self.pos;
            if !self.fill(ctx).await? {
                rec.extend_from_slice(&self.buf[self.pos..]);
                self.pos = self.buf.len();
                let mut rt = Vec::new();
                while rec.last() == Some(&b'\n') {
                    rec.pop();
                    rt.push(b'\n');
                }
                return Ok(Some(Rc::from(rt)));
            }
            searched -= base - self.pos;
        }
    }

    async fn read_regex(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        re: &Regex,
        rec: &mut Vec<u8>,
    ) -> io::Result<Option<Rc<[u8]>>> {
        loop {
            let hay = &self.buf[self.pos..];
            let m =
                find_nonempty(re, &self.buf, self.pos).map(|(s, e)| (s - self.pos, e - self.pos));
            match m {
                // A match that reaches the end of the data may grow with more input.
                Some((s, e)) if self.pos + e < self.buf.len() || self.eof => {
                    rec.extend_from_slice(&hay[..s]);
                    let rt = Rc::from(&hay[s..e]);
                    self.pos += e;
                    return Ok(Some(rt));
                }
                _ => {}
            }
            if !self.fill(ctx).await? {
                if let Some((s, e)) = find_nonempty(re, &self.buf, self.pos)
                    .map(|(s, e)| (s - self.pos, e - self.pos))
                {
                    let hay = &self.buf[self.pos..];
                    rec.extend_from_slice(&hay[..s]);
                    let rt = Rc::from(&hay[s..e]);
                    self.pos += e;
                    return Ok(Some(rt));
                }
                if self.pos < self.buf.len() {
                    rec.extend_from_slice(&self.buf[self.pos..]);
                    self.pos = self.buf.len();
                    return Ok(Some(Rc::from(&b""[..])));
                }
                return Ok(None);
            }
        }
    }
}

/// The first match of `re` in `text` at `start` or later that is not empty.
pub fn find_nonempty(re: &Regex, text: &[u8], start: usize) -> Option<(usize, usize)> {
    let mut from = start;
    while from <= text.len() {
        let (s, e) = re.find_at(text, from, ExecOptions::default())?;
        if e > s {
            return Some((s, e));
        }
        from = s + 1;
    }
    None
}

pub enum OutKind {
    File(UtilFile),
    Cmd(ShellChild),
}

pub struct Output {
    pub kind: OutKind,
    pub buf: Vec<u8>,
}

impl Output {
    pub async fn flush(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let data = std::mem::take(&mut self.buf);
        match &mut self.kind {
            OutKind::File(f) => f.write_all(&data).await,
            OutKind::Cmd(c) => match &c.stdin {
                Some(w) => w.write_all(&data).await,
                None => Ok(()),
            },
        }
    }

    pub async fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.buf.extend_from_slice(data);
        if self.buf.len() >= CHUNK {
            self.flush().await?;
        }
        Ok(())
    }

    /// Flushes and closes: a command's exit status, 0 for a file (-1 on error).
    pub async fn close(mut self) -> i32 {
        let flushed = self.flush().await;
        match self.kind {
            OutKind::File(f) => {
                drop(f);
                if flushed.is_ok() { 0 } else { -1 }
            }
            OutKind::Cmd(mut c) => {
                c.stdin = None;
                c.wait().await
            }
        }
    }
}
