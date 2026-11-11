//! `cmp [-b] [-l] [-s] [-i SKIP[:SKIP2]] [-n LIMIT] FILE1 [FILE2 [SKIP1 [SKIP2]]]`:
//! compares two files byte by byte. GNU's output (D016): `differ: byte N, line L`
//! (BSD says `char`), `EOF on FILE after byte N`, `-l` offsets padded to the width of
//! the longest possible one. Status 0 same, 1 different, 2 trouble. Written for psh
//! after the GNU diffutils manual.

use psh_engine::host::FileKind;
use psh_engine::util::UtilCtx;

use crate::common::{self, Input};
use crate::opts::{Spec, both, long};

pub const SPEC: Spec = Spec {
    name: "cmp",
    opts: &[
        both('b', "print-bytes"),
        both('i', "ignore-initial").value(),
        both('l', "verbose"),
        both('n', "bytes").value(),
        both('s', "quiet"),
        long("silent"),
    ],
    posix_order: false,
};

/// diffutils' usage error: status 2.
async fn usage(ctx: &mut UtilCtx<'_>, msg: &str) -> i32 {
    ctx.error(msg).await;
    ctx.error("Try 'cmp --help' for more information.").await;
    2
}

/// A buffered input.
struct Reader {
    input: Input,
    buf: Vec<u8>,
    pos: usize,
    len: usize,
    eof: bool,
}

impl Reader {
    /// The unread bytes, refilled when empty; empty at EOF.
    async fn fill(&mut self, ctx: &mut UtilCtx<'_>) -> std::io::Result<&[u8]> {
        if self.pos == self.len && !self.eof {
            let n = self.input.read(ctx, &mut self.buf).await?;
            self.pos = 0;
            self.len = n;
            self.eof = n == 0;
        }
        Ok(&self.buf[self.pos..self.len])
    }

    async fn skip(&mut self, ctx: &mut UtilCtx<'_>, mut n: u64) -> std::io::Result<()> {
        while n > 0 {
            let avail = self.fill(ctx).await?.len() as u64;
            if avail == 0 {
                break;
            }
            let k = avail.min(n);
            self.pos += k as usize;
            n -= k;
        }
        Ok(())
    }
}

/// `printf %s` of a byte as GNU cmp shows it: `^X` for controls, `M-` for high bytes.
fn shown(b: u8) -> String {
    let mut s = String::new();
    let mut c = b;
    if c >= 128 {
        s.push_str("M-");
        c -= 128;
    }
    match c {
        0..=31 => {
            s.push('^');
            s.push((c + 64) as char);
        }
        127 => s.push_str("^?"),
        _ => s.push(c as char),
    }
    s
}

fn parse_skip(v: &[u8]) -> Option<(u64, u64)> {
    match v.iter().position(|&b| b == b':') {
        Some(i) => Some((
            common::parse_count(&v[..i])?,
            common::parse_count(&v[i + 1..])?,
        )),
        None => {
            let n = common::parse_count(v)?;
            Some((n, n))
        }
    }
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match SPEC.parse(&argv[1..]) {
        Ok(p) => p,
        Err(e) if e.is_unknown() => return common::option_error(ctx, &SPEC, argv, &e).await,
        Err(e) => return usage(ctx, &e.to_string()).await,
    };
    let print_bytes = p.has(&SPEC, "print-bytes");
    let verbose = p.has(&SPEC, "verbose");
    let silent = p.has(&SPEC, "quiet") || p.has(&SPEC, "silent");
    if verbose && silent {
        return usage(ctx, "options -l and -s are incompatible").await;
    }
    let mut skip = (0u64, 0u64);
    if let Some(v) = p.last(&SPEC, "ignore-initial") {
        match parse_skip(v) {
            Some(s) => skip = s,
            None => {
                let q = common::quote(ctx, v);
                return usage(ctx, &format!("invalid --ignore-initial value {q}")).await;
            }
        }
    }
    let mut limit: Option<u64> = None;
    if let Some(v) = p.last(&SPEC, "bytes") {
        match common::parse_count(v) {
            Some(n) => limit = Some(n),
            None => {
                let q = common::quote(ctx, v);
                return usage(ctx, &format!("invalid --bytes value {q}")).await;
            }
        }
    }
    let ops = &p.operands;
    if ops.is_empty() {
        return usage(ctx, "missing operand after 'cmp'").await;
    }
    if ops.len() > 4 {
        let q = common::quote(ctx, &ops[4]);
        return usage(ctx, &format!("extra operand {q}")).await;
    }
    for (i, slot) in [(2, 0), (3, 1)] {
        if let Some(v) = ops.get(i) {
            match common::parse_count(v) {
                Some(n) if slot == 0 => skip.0 = n,
                Some(n) => skip.1 = n,
                None => {
                    let q = common::quote(ctx, v);
                    return usage(ctx, &format!("invalid --ignore-initial value {q}")).await;
                }
            }
        }
    }
    let names = [
        ops[0].clone(),
        ops.get(1).cloned().unwrap_or_else(|| b"-".to_vec()),
    ];
    let mut readers = Vec::with_capacity(2);
    // What is left of each file after its skip, when the file says (a regular file).
    let mut sizes: Vec<Option<u64>> = Vec::with_capacity(2);
    for (i, name) in names.iter().enumerate() {
        let input = match Input::open(ctx, name).await {
            Ok(input) => input,
            Err(e) => {
                if !silent {
                    common::file_error(ctx, name, &e).await;
                }
                return 2;
            }
        };
        let sk = if i == 0 { skip.0 } else { skip.1 };
        let size = match &input {
            Input::File(_) => match ctx.stat(name, true).await {
                Ok(m) if m.kind == FileKind::File => Some(m.size.saturating_sub(sk)),
                _ => None,
            },
            Input::Stdin => None,
        };
        sizes.push(size);
        let mut r = Reader {
            input,
            buf: vec![0u8; 64 * 1024],
            pos: 0,
            len: 0,
            eof: false,
        };
        if let Err(e) = r.skip(ctx, sk).await {
            if !silent {
                common::file_error(ctx, name, &e).await;
            }
            return 2;
        }
        readers.push(r);
    }
    // The width of `-l` offsets: the largest offset that can be printed.
    let width = {
        let most = sizes
            .iter()
            .flatten()
            .copied()
            .chain(limit)
            .min()
            .unwrap_or(i64::MAX as u64);
        most.max(1).to_string().len()
    };
    let (mut a, mut b) = {
        let mut it = readers.into_iter();
        (it.next().unwrap(), it.next().unwrap())
    };
    let mut offset: u64 = 0;
    let mut lines: u64 = 0;
    // The last byte compared was a newline: an EOF reports the line it ended.
    let mut last_nl = false;
    let mut differed = false;
    let mut out = Vec::new();
    loop {
        if limit.is_some_and(|l| offset >= l) {
            break;
        }
        let x = match a.fill(ctx).await {
            Ok(x) => x.len(),
            Err(e) => return read_failed(ctx, &names[0], &e, silent).await,
        };
        let y = match b.fill(ctx).await {
            Ok(y) => y.len(),
            Err(e) => return read_failed(ctx, &names[1], &e, silent).await,
        };
        if x == 0 || y == 0 {
            if x == 0 && y == 0 {
                break;
            }
            // One file ended first.
            let which = if x == 0 { &names[0] } else { &names[1] };
            if !silent {
                if let Err(e) = flush(ctx, &mut out).await {
                    return common::write_failed(ctx, &e).await;
                }
                let name = String::from_utf8_lossy(which);
                let msg = if offset == 0 {
                    format!("EOF on {name} which is empty")
                } else if verbose {
                    format!("EOF on {name} after byte {offset}")
                } else {
                    let line = if last_nl { lines } else { lines + 1 };
                    format!("EOF on {name} after byte {offset}, line {line}")
                };
                ctx.error(&msg).await;
            }
            return 1;
        }
        let mut n = x.min(y);
        if let Some(l) = limit {
            n = n.min((l - offset) as usize);
        }
        let sa = &a.buf[a.pos..a.pos + n];
        let sb = &b.buf[b.pos..b.pos + n];
        let mut i = 0;
        while i < n {
            // Fast path over equal runs.
            let same = sa[i..]
                .iter()
                .zip(&sb[i..])
                .take_while(|(p, q)| p == q)
                .count();
            for &c in &sa[i..i + same] {
                if c == b'\n' {
                    lines += 1;
                }
            }
            if same > 0 {
                last_nl = sa[i + same - 1] == b'\n';
            }
            i += same;
            if i == n {
                break;
            }
            let pos = offset + i as u64 + 1;
            differed = true;
            if silent {
                return 1;
            }
            let (c1, c2) = (sa[i], sb[i]);
            if !verbose {
                let mut line = format!(
                    "{} {} differ: byte {pos}, line {}",
                    String::from_utf8_lossy(&names[0]),
                    String::from_utf8_lossy(&names[1]),
                    lines + 1
                );
                if print_bytes {
                    line.push_str(&format!(" is {c1:3o} {} {c2:3o} {}", shown(c1), shown(c2)));
                }
                line.push('\n');
                if let Err(e) = ctx.write(line.as_bytes()).await {
                    return common::write_failed(ctx, &e).await;
                }
                return 1;
            }
            let line = if print_bytes {
                format!(
                    "{pos:>width$} {c1:3o} {:<4} {c2:3o} {}\n",
                    shown(c1),
                    shown(c2)
                )
            } else {
                format!("{pos:>width$} {c1:3o} {c2:3o}\n")
            };
            out.extend_from_slice(line.as_bytes());
            if c1 == b'\n' {
                lines += 1;
            }
            last_nl = c1 == b'\n';
            i += 1;
        }
        a.pos += n;
        b.pos += n;
        offset += n as u64;
        if out.len() > 32 * 1024
            && let Err(e) = flush(ctx, &mut out).await
        {
            return common::write_failed(ctx, &e).await;
        }
        ctx.check().await;
    }
    if let Err(e) = flush(ctx, &mut out).await {
        return common::write_failed(ctx, &e).await;
    }
    i32::from(differed)
}

async fn flush(ctx: &mut UtilCtx<'_>, out: &mut Vec<u8>) -> std::io::Result<()> {
    if out.is_empty() {
        return Ok(());
    }
    let r = ctx.write(out).await;
    out.clear();
    r
}

async fn read_failed(ctx: &mut UtilCtx<'_>, name: &[u8], e: &std::io::Error, silent: bool) -> i32 {
    if !silent {
        common::file_error(ctx, name, e).await;
    }
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_shown() {
        assert_eq!(shown(1), "^A");
        assert_eq!(shown(0x80), "M-^@");
        assert_eq!(shown(0xff), "M-^?");
        assert_eq!(shown(b'a'), "a");
    }

    #[test]
    fn skips() {
        assert_eq!(parse_skip(b"2"), Some((2, 2)));
        assert_eq!(parse_skip(b"1:3"), Some((1, 3)));
        assert_eq!(parse_skip(b"1k"), Some((1024, 1024)));
        assert_eq!(parse_skip(b"x"), None);
    }
}
