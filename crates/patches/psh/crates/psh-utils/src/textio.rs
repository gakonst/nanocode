//! Shared by the text filters (`sort uniq cut tr paste nl tee seq comm`): reading
//! lines from an input, and falling through on forms psh does not implement.

use std::io;

use psh_engine::util::UtilCtx;

use crate::common::{self, Input};

/// Reads delimited lines (`\n`, or NUL with `-z`) from one input, in chunks.
pub struct LineReader {
    buf: Vec<u8>,
    pos: usize,
    eof: bool,
    delim: u8,
}

impl LineReader {
    pub fn new(delim: u8) -> LineReader {
        LineReader {
            buf: Vec::new(),
            pos: 0,
            eof: false,
            delim,
        }
    }

    /// The next line, without its delimiter, into `line` (cleared first). `Ok(None)` at
    /// the end; `Ok(Some(false))` for a last line without a delimiter.
    pub async fn next(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        input: &Input,
        line: &mut Vec<u8>,
    ) -> io::Result<Option<bool>> {
        line.clear();
        loop {
            if let Some(i) = self.buf[self.pos..].iter().position(|&b| b == self.delim) {
                line.extend_from_slice(&self.buf[self.pos..self.pos + i]);
                self.pos += i + 1;
                return Ok(Some(true));
            }
            if self.eof {
                if self.pos < self.buf.len() {
                    line.extend_from_slice(&self.buf[self.pos..]);
                    self.pos = self.buf.len();
                    return Ok(Some(false));
                }
                return Ok(None);
            }
            self.buf.drain(..self.pos);
            self.pos = 0;
            let start = self.buf.len();
            self.buf.resize(start + 64 * 1024, 0);
            let n = input.read(ctx, &mut self.buf[start..]).await;
            let n = match n {
                Ok(n) => n,
                Err(e) => {
                    self.buf.truncate(start);
                    return Err(e);
                }
            };
            self.buf.truncate(start + n);
            if n == 0 {
                self.eof = true;
            }
            ctx.check().await;
        }
    }
}

/// A form psh does not implement (`nl -b pREGEX`): the real program runs if there is
/// one, else `NAME: unsupported WHAT (psh built-in NAME)` and status 2 (D010).
pub async fn fall_through(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>], what: &str) -> i32 {
    if let Some(status) = ctx.run_program(argv.to_vec()).await {
        return status;
    }
    let name = ctx.name().to_string();
    ctx.error(&format!("unsupported {what} (psh built-in {name})"))
        .await;
    2
}

/// Opens an operand, reporting a failure as `NAME: FILE: reason`.
pub async fn open_or_report(ctx: &mut UtilCtx<'_>, name: &[u8]) -> Option<Input> {
    match Input::open(ctx, name).await {
        Ok(i) => Some(i),
        Err(e) => {
            common::file_error(ctx, name, &e).await;
            None
        }
    }
}

/// Decodes one UTF-8 character at the start of `s`: the character and its length, or
/// `None` and 1 for a byte that does not start a valid sequence.
pub fn decode_char(s: &[u8]) -> (Option<char>, usize) {
    let b = s[0];
    if b < 0x80 {
        return (Some(b as char), 1);
    }
    let len = match b {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return (None, 1),
    };
    match s.get(..len).map(std::str::from_utf8) {
        Some(Ok(t)) => (t.chars().next(), len),
        _ => (None, 1),
    }
}

/// The lengths of the characters of `s`: UTF-8 sequences, and single bytes for
/// anything invalid.
pub fn char_lengths(s: &[u8]) -> impl Iterator<Item = usize> + '_ {
    let mut i = 0;
    std::iter::from_fn(move || {
        if i >= s.len() {
            return None;
        }
        let (_, n) = decode_char(&s[i..]);
        i += n;
        Some(n)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chars() {
        let v: Vec<usize> = char_lengths("aé\u{20ac}\u{1f600}".as_bytes()).collect();
        assert_eq!(v, [1, 2, 3, 4]);
        let v: Vec<usize> = char_lengths(b"\xc3a\xff").collect();
        assert_eq!(v, [1, 1, 1]);
    }
}
