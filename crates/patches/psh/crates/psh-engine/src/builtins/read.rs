//! `read [-ers] [-a array] [-d delim] [-i text] [-n nchars] [-N nchars] [-p prompt]
//! [-t timeout] [-u fd] [name ...]`: one line (or up to a delimiter, or a number of
//! characters) from a descriptor, split by `IFS` into variables.
//!
//! Reads ahead from engine pipes that are not OS pipes and from seekable files and gives
//! back what follows the delimiter, one byte at a time from anything else, so the bytes
//! after the delimiter stay in the descriptor for the next reader (bash does the same).

use std::future::poll_fn;
use std::task::Poll;
use std::time::Duration;

use crate::fd::Fd;
use crate::host::{HostFuture, error_text};
use crate::state::{R, Vp, is_name};
use crate::vars::Value;

use super::s;

const USAGE: &[u8] = b"read: usage: read [-Eers] [-a array] [-d delim] [-i text] [-n nchars] [-N nchars] [-p prompt] [-t timeout] [-u fd] [name ...]\n";

/// About 100 years: longer timeouts are clamped (the host's timer must be able to add
/// it to the current time).
const MAX_TIMEOUT_SECS: u64 = 100 * 365 * 24 * 3600;

/// `128 + SIGALRM`: what bash returns when `read` times out.
const TIMEOUT_STATUS: i32 = 142;

struct Opts {
    raw: bool,
    delim: u8,
    /// `-n` (stop early at the delimiter) or `-N` (exactly, delimiter not special).
    count: Option<(usize, bool)>,
    prompt: Option<Vec<u8>>,
    timeout: Option<Duration>,
    fd: u32,
    array: Option<Vec<u8>>,
}

/// A line into `REPLY`, as `read` without arguments reads it: `select`'s answer.
pub(crate) async fn reply(vp: &mut Vp) -> R {
    read(vp, &[b"read".to_vec()]).await
}

pub(super) async fn read(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let mut o = Opts {
        raw: false,
        delim: b'\n',
        count: None,
        prompt: None,
        timeout: None,
        fd: 0,
        array: None,
    };
    let mut timeout: Option<Duration> = None;
    let mut i = 1;
    'opts: while i < args.len() {
        let a = &args[i];
        if a == b"--" {
            i += 1;
            break;
        }
        if a.len() < 2 || a[0] != b'-' {
            break;
        }
        let mut j = 1;
        while j < a.len() {
            let c = a[j];
            j += 1;
            match c {
                b'r' => o.raw = true,
                b's' | b'e' | b'E' => {}
                b'a' | b'd' | b'i' | b'n' | b'N' | b'p' | b't' | b'u' => {
                    // The value is the rest of this word, or the next word.
                    let value = if j < a.len() {
                        a[j..].to_vec()
                    } else {
                        i += 1;
                        match args.get(i) {
                            Some(v) => v.clone(),
                            None => {
                                vp.error(&format!(
                                    "read: -{}: option requires an argument",
                                    c as char
                                ))
                                .await;
                                vp.error_raw(USAGE).await;
                                return Ok(2);
                            }
                        }
                    };
                    match c {
                        b'a' => o.array = Some(value),
                        b'd' => o.delim = value.first().copied().unwrap_or(0),
                        b'i' => {}
                        b'n' | b'N' => match parse_count(&value) {
                            Some(n) => o.count = Some((n, c == b'N')),
                            None => {
                                vp.error(&format!("read: {}: invalid number", s(&value)))
                                    .await;
                                return Ok(1);
                            }
                        },
                        b'p' => o.prompt = Some(value),
                        b't' => match parse_timeout(&value) {
                            Some(d) => timeout = Some(d),
                            None => {
                                vp.error(&format!(
                                    "read: {}: invalid timeout specification",
                                    s(&value)
                                ))
                                .await;
                                return Ok(1);
                            }
                        },
                        _ => match parse_count(&value).and_then(|n| u32::try_from(n).ok()) {
                            Some(n) if vp.fd(n).is_some() => o.fd = n,
                            Some(n) => {
                                vp.error(&format!(
                                    "read: {n}: invalid file descriptor: Bad file descriptor"
                                ))
                                .await;
                                return Ok(1);
                            }
                            None => {
                                vp.error(&format!(
                                    "read: {}: invalid file descriptor specification",
                                    s(&value)
                                ))
                                .await;
                                return Ok(1);
                            }
                        },
                    }
                    i += 1;
                    continue 'opts;
                }
                _ => {
                    vp.error(&format!("read: -{}: invalid option", c as char))
                        .await;
                    vp.error_raw(USAGE).await;
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    let names = &args[i..];

    // A `-t` argument; else `TMOUT`, if it is a positive number.
    let mut zero_timeout = false;
    match timeout {
        Some(d) if d.is_zero() => zero_timeout = true,
        Some(d) => o.timeout = Some(d),
        None => {
            if let Some(d) = vp.var("TMOUT").and_then(parse_timeout)
                && !d.is_zero()
            {
                o.timeout = Some(d);
            }
        }
    }

    // `-a NAME`: checked before anything is read.
    let array = match &o.array {
        None => None,
        Some(a) => {
            if !is_name(a) {
                vp.error(&format!("read: `{}': not a valid identifier", s(a)))
                    .await;
                return Ok(1);
            }
            let name = vp.resolve(&s(a)).into_owned();
            if let Some(v) = vp.vars.get(&name) {
                if v.readonly {
                    vp.error(&format!("{name}: readonly variable")).await;
                    return Ok(1);
                }
                if v.is_assoc() {
                    vp.error(&format!("read: {name}: not an indexed array"))
                        .await;
                    return Ok(1);
                }
            }
            Some(name)
        }
    };

    let Some(fd) = vp.fd(o.fd) else {
        vp.error(&format!("read: {}: read error: Bad file descriptor", o.fd))
            .await;
        return Ok(1);
    };
    if zero_timeout {
        // Whether input is available, without reading any.
        return Ok(0);
    }
    if let Some(p) = &o.prompt
        && fd_is_terminal(&fd)
    {
        let _ = super::write_fd(vp, 2, "read", p).await?;
    }

    let utf8 = vp.utf8();
    let mut input = Input {
        fd,
        timer: o.timeout.map(|d| vp.host().sleep(d)),
        back: None,
        buf: Vec::new(),
        pos: 0,
        ahead: None,
        size: 128,
        delim: o.delim,
    };
    // The bytes read, each with whether a backslash escaped it.
    let mut line: Vec<(u8, bool)> = Vec::new();
    let mut chars = 0usize;
    let mut escape = false;
    let mut status = 0;
    let exact = matches!(o.count, Some((_, true)));
    let crlf = o.delim == b'\n' && o.count.is_none() && vp.host().strips_crlf();
    loop {
        if let Some((n, _)) = o.count
            && chars >= n
        {
            break;
        }
        // A signal for the shell runs its trap while the read goes on (bash outside
        // POSIX mode); a fatal one ends the shell (D1306).
        let got = crate::signals::or_signal(vp, crate::signals::Gives::Trapped, input.next()).await;
        let b = match got.unwrap_or(Got::Signal) {
            Got::Signal => {
                input.give_back();
                super::trap::take_signals(vp).await?;
                continue;
            }
            Got::Byte(b) => b,
            Got::Eof => {
                status = 1;
                break;
            }
            Got::Timeout => {
                status = TIMEOUT_STATUS;
                break;
            }
            Got::Err(e) => {
                input.give_back();
                vp.error(&format!("read: {}: read error: {}", o.fd, error_text(&e)))
                    .await;
                return Ok(1);
            }
        };
        // NUL bytes are dropped, unless they are the delimiter.
        if b == 0 && !(o.delim == 0 && !exact) {
            continue;
        }
        if escape {
            escape = false;
            if b == b'\n' {
                // A line continuation.
                continue;
            }
            line.push((b, true));
        } else if !o.raw && b == b'\\' {
            escape = true;
            continue;
        } else if !exact && b == o.delim {
            // A CRLF line from a host whose lines end so: the `\r` goes too, only
            // for whole lines (no -d, -n, -N), with or without -r (D1292).
            if crlf && line.last() == Some(&(b'\r', false)) {
                line.pop();
            }
            break;
        } else {
            line.push((b, false));
        }
        // A multibyte character counts once: take its continuation bytes along.
        if utf8 && b >= 0xc0 {
            let more = if b >= 0xf0 {
                3
            } else if b >= 0xe0 {
                2
            } else {
                1
            };
            let escaped = line.last().is_some_and(|l| l.1);
            for _ in 0..more {
                match input.next().await {
                    Got::Byte(c) if (0x80..0xc0).contains(&c) => line.push((c, escaped)),
                    Got::Byte(c) => {
                        input.back = Some(c);
                        break;
                    }
                    _ => break,
                }
            }
        }
        chars += 1;
    }
    input.give_back();

    let ifs = vp.var("IFS").map(<[u8]>::to_vec);
    let ifs = ifs.as_deref().unwrap_or(b" \t\n");
    let text = |l: &[(u8, bool)]| l.iter().map(|b| b.0).collect::<Vec<u8>>();
    if let Some(name) = array {
        // `-N` keeps the input whole; otherwise every field is an element.
        let fields = if exact {
            vec![text(&line)]
        } else {
            split_all(&line, ifs)
        };
        let var = vp.vars.entry(&name);
        var.value = Value::Indexed(
            fields
                .into_iter()
                .enumerate()
                .map(|(k, f)| (k as i64, f))
                .collect(),
        );
        return Ok(status);
    }
    let values: Vec<Vec<u8>> = if names.is_empty() {
        vec![text(&line)]
    } else if exact {
        // `-N` puts everything into the first variable, unsplit.
        let mut v = vec![text(&line)];
        v.resize(names.len(), Vec::new());
        v
    } else {
        split(&line, ifs, names.len())
    };
    let targets: Vec<String> = if names.is_empty() {
        vec!["REPLY".into()]
    } else {
        names.iter().map(|n| s(n).into_owned()).collect()
    };
    for (n, v) in targets.iter().zip(values) {
        // Names are checked as they are assigned: the ones before a bad one are set.
        if !is_name(n.as_bytes()) {
            vp.error(&format!("read: `{n}': not a valid identifier"))
                .await;
            return Ok(1);
        }
        // Attributes apply (`-i`, `-l`, `-u`, readonly, namerefs).
        if let Err(msg) = vp.assign(n, v) {
            vp.error(&msg).await;
            return Ok(1);
        }
    }
    Ok(status)
}

/// Splits a line into `n` values as `read` does: IFS whitespace around fields is
/// dropped, the last variable gets the rest of the line.
fn split(line: &[(u8, bool)], ifs: &[u8], n: usize) -> Vec<Vec<u8>> {
    let is_ifs = |&(b, esc): &(u8, bool)| !esc && ifs.contains(&b);
    let is_ws =
        |&(b, esc): &(u8, bool)| !esc && ifs.contains(&b) && matches!(b, b' ' | b'\t' | b'\n');
    let text = |r: &[(u8, bool)]| r.iter().map(|l| l.0).collect::<Vec<u8>>();
    // After a field: IFS whitespace, at most one other IFS char, IFS whitespace.
    let skip_sep = |mut p: usize| {
        while p < line.len() && is_ws(&line[p]) {
            p += 1;
        }
        if p < line.len() && is_ifs(&line[p]) && !is_ws(&line[p]) {
            p += 1;
            while p < line.len() && is_ws(&line[p]) {
                p += 1;
            }
        }
        p
    };
    let mut out = Vec::with_capacity(n);
    let mut p = 0;
    while p < line.len() && is_ws(&line[p]) {
        p += 1;
    }
    for _ in 0..n.saturating_sub(1) {
        let start = p;
        while p < line.len() && !is_ifs(&line[p]) {
            p += 1;
        }
        out.push(text(&line[start..p]));
        p = skip_sep(p);
    }
    // The last variable: the rest, without trailing IFS whitespace; a single field
    // followed by one separator loses the separator.
    let mut end = line.len();
    while end > p && is_ws(&line[end - 1]) {
        end -= 1;
    }
    let rest = &line[p..end];
    let mut f = 0;
    while f < rest.len() && !is_ifs(&rest[f]) {
        f += 1;
    }
    let after = {
        let mut q = f;
        while q < rest.len() && is_ws(&rest[q]) {
            q += 1;
        }
        if q < rest.len() && is_ifs(&rest[q]) && !is_ws(&rest[q]) {
            q += 1;
        }
        while q < rest.len() && is_ws(&rest[q]) {
            q += 1;
        }
        q
    };
    if after == rest.len() {
        out.push(text(&rest[..f]));
    } else {
        out.push(text(rest));
    }
    out
}

/// Splits a line into all its fields, as `read -a` does: like word splitting, a
/// separator at the end makes no empty field.
fn split_all(line: &[(u8, bool)], ifs: &[u8]) -> Vec<Vec<u8>> {
    let is_ifs = |&(b, esc): &(u8, bool)| !esc && ifs.contains(&b);
    let is_ws =
        |&(b, esc): &(u8, bool)| !esc && ifs.contains(&b) && matches!(b, b' ' | b'\t' | b'\n');
    let mut out = Vec::new();
    let mut p = 0;
    while p < line.len() && is_ws(&line[p]) {
        p += 1;
    }
    while p < line.len() {
        let start = p;
        while p < line.len() && !is_ifs(&line[p]) {
            p += 1;
        }
        out.push(line[start..p].iter().map(|l| l.0).collect());
        while p < line.len() && is_ws(&line[p]) {
            p += 1;
        }
        if p < line.len() && is_ifs(&line[p]) && !is_ws(&line[p]) {
            p += 1;
            while p < line.len() && is_ws(&line[p]) {
                p += 1;
            }
        }
    }
    out
}

/// A non-negative decimal, with blanks around it and a `+` allowed.
fn parse_count(v: &[u8]) -> Option<usize> {
    let t = std::str::from_utf8(v).ok()?.trim_matches([' ', '\t', '\n']);
    let t = t.strip_prefix('+').unwrap_or(t);
    if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.parse().ok()
}

/// `-t` and `TMOUT`: seconds with an optional fraction.
fn parse_timeout(v: &[u8]) -> Option<Duration> {
    let t = std::str::from_utf8(v).ok()?;
    let t = t.strip_prefix('+').unwrap_or(t);
    let (int, frac) = t.split_once('.').unwrap_or((t, ""));
    // An empty timeout (`''`, `.`) is zero.
    if !int.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Too long to represent: as good as forever.
    let secs: u64 = int.parse::<u64>().unwrap_or(0).min(MAX_TIMEOUT_SECS);
    if int.len() > 18 {
        return Some(Duration::from_secs(MAX_TIMEOUT_SECS));
    }
    let mut nanos = 0u32;
    for (k, d) in frac.bytes().take(9).enumerate() {
        nanos += u32::from(d - b'0') * 10u32.pow(8 - k as u32);
    }
    Some(Duration::new(secs, nanos))
}

fn fd_is_terminal(fd: &Fd) -> bool {
    match &**fd {
        crate::fd::FdObj::File(f) => f.is_terminal(),
        _ => false,
    }
}

enum Got {
    Byte(u8),
    /// A signal for the shell came first.
    Signal,
    Eof,
    Timeout,
    Err(std::io::Error),
}

struct Input {
    fd: Fd,
    timer: Option<HostFuture<()>>,
    /// A byte read ahead (an incomplete multibyte character).
    back: Option<u8>,
    buf: Vec<u8>,
    pos: usize,
    ahead: Option<bool>,
    size: usize,
    delim: u8,
}

impl Input {
    fn chunk(&mut self) -> usize {
        let ahead = *self.ahead.get_or_insert_with(|| match &*self.fd {
            // An OS pipe is read a byte at a time, as bash reads any pipe: what a
            // read-ahead gave back would sit in front of what an external reading
            // the OS end takes next.
            crate::fd::FdObj::PipeRead(r) => !r.is_os(),
            crate::fd::FdObj::File(f) => {
                matches!(f.seek(std::io::SeekFrom::Current(0)), Some(Ok(_)))
            }
            crate::fd::FdObj::PipeWrite(_) => false,
        });
        if !ahead {
            return 1;
        }
        if let crate::fd::FdObj::PipeRead(r) = &*self.fd
            && let Some(n) = r.line_len(self.delim)
        {
            return n.min(4096);
        }
        let n = self.size;
        self.size = (n * 2).min(4096);
        n
    }

    fn give_back(&mut self) {
        let mut rest = Vec::new();
        rest.extend(self.back.take());
        rest.extend_from_slice(&self.buf[self.pos..]);
        self.buf.clear();
        self.pos = 0;
        if rest.is_empty() {
            return;
        }
        match &*self.fd {
            crate::fd::FdObj::PipeRead(r) => r.unread(&rest),
            crate::fd::FdObj::File(f) => {
                let _ = f.seek(std::io::SeekFrom::Current(-(rest.len() as i64)));
            }
            crate::fd::FdObj::PipeWrite(_) => {}
        }
    }

    async fn next(&mut self) -> Got {
        if let Some(b) = self.back.take() {
            return Got::Byte(b);
        }
        if self.pos < self.buf.len() {
            self.pos += 1;
            return Got::Byte(self.buf[self.pos - 1]);
        }
        let n = self.chunk();
        self.buf.clear();
        self.buf.resize(n, 0);
        self.pos = 0;
        let buf = &mut self.buf;
        let fd = &self.fd;
        let timer = &mut self.timer;
        let got = poll_fn(|cx| {
            // The pipe may have become an OS pipe while this read waited.
            let n = match &**fd {
                crate::fd::FdObj::PipeRead(r) if r.is_os() => 1,
                _ => buf.len(),
            };
            if let Poll::Ready(r) = fd.poll_read(cx, &mut buf[..n]) {
                return Poll::Ready(Some(r));
            }
            if let Some(t) = timer
                && t.as_mut().poll(cx).is_ready()
            {
                return Poll::Ready(None);
            }
            Poll::Pending
        })
        .await;
        match got {
            Some(Ok(0)) => {
                self.buf.clear();
                Got::Eof
            }
            Some(Ok(n)) => {
                self.buf.truncate(n);
                self.pos = 1;
                Got::Byte(self.buf[0])
            }
            Some(Err(e)) => {
                self.buf.clear();
                Got::Err(e)
            }
            None => {
                self.buf.clear();
                Got::Timeout
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(line: &str, ifs: &str, n: usize) -> Vec<String> {
        let l: Vec<(u8, bool)> = line.bytes().map(|b| (b, false)).collect();
        split(&l, ifs.as_bytes(), n)
            .into_iter()
            .map(|v| String::from_utf8(v).unwrap())
            .collect()
    }

    #[test]
    fn splitting_like_bash() {
        assert_eq!(sp("1:2:", ":", 2), ["1", "2"]);
        assert_eq!(sp("1:2:3:", ":", 2), ["1", "2:3:"]);
        assert_eq!(sp("1:2::", ":", 2), ["1", "2::"]);
        assert_eq!(sp("x:", ":", 1), ["x"]);
        assert_eq!(sp("x::", ":", 1), ["x::"]);
        assert_eq!(sp("1 : 2 : ", ": ", 2), ["1", "2"]);
        assert_eq!(sp("1 : 2 :  3 ", ": ", 2), ["1", "2 :  3"]);
        assert_eq!(sp(":x", ":", 3), ["", "x", ""]);
        assert_eq!(sp("  x y  ", "", 2), ["  x y  ", ""]);
        assert_eq!(sp("A\t\tB C D E ", " \t\n", 3), ["A", "B", "C D E"]);
        let all = |line: &str, ifs: &str| -> Vec<String> {
            let l: Vec<(u8, bool)> = line.bytes().map(|b| (b, false)).collect();
            split_all(&l, ifs.as_bytes())
                .into_iter()
                .map(|v| String::from_utf8(v).unwrap())
                .collect()
        };
        assert_eq!(all("a b x ", "x "), ["a", "b"]);
        assert_eq!(all("a b x x", "x "), ["a", "b", ""]);
        assert_eq!(all(":a::b", ":"), ["", "a", "", "b"]);
        assert!(all("   ", " ").is_empty());
    }

    #[test]
    fn timeouts() {
        assert_eq!(parse_timeout(b"1.5"), Some(Duration::from_millis(1500)));
        assert_eq!(parse_timeout(b".25"), Some(Duration::from_millis(250)));
        assert_eq!(parse_timeout(b"-1"), None);
        assert_eq!(parse_timeout(b"abc"), None);
        assert_eq!(parse_timeout(b""), Some(Duration::ZERO));
        assert_eq!(parse_timeout(b"."), Some(Duration::ZERO));
        assert_eq!(parse_timeout(b"1e1"), None);
        assert!(parse_timeout(b"99999999999999999999").is_some());
    }
}
