//! `uniq [-cdDiuz] [-f N] [-s N] [-w N] [INPUT [OUTPUT]]` with `--all-repeated` and
//! `--group`. Written for psh after GNU's documented behavior. Where GNU and BSD
//! differ, GNU (D016): `-c` pads counts to 7 columns (BSD 4), and a last line without a
//! newline gets one. `-s` and `-w` count UTF-8 characters.

use psh_engine::host::OpenOptions;
use psh_engine::util::{UtilCtx, UtilFile};

use crate::common::{self, Input};
use crate::opts::{Spec, both, flag, long};
use crate::textio::{self, LineReader, char_lengths};

pub const SPEC: Spec = Spec {
    name: "uniq",
    opts: &[
        both('c', "count"),
        both('d', "repeated"),
        flag('D'),
        long("all-repeated").optional(),
        long("group").optional(),
        both('f', "skip-fields").value(),
        both('s', "skip-chars").value(),
        both('w', "check-chars").value(),
        both('i', "ignore-case"),
        both('u', "unique"),
        both('z', "zero-terminated"),
    ],
    posix_order: false,
};

/// Separators between groups.
#[derive(Clone, Copy, PartialEq)]
enum Delimit {
    None,
    Prepend,
    Separate,
    Append,
    Both,
}

/// What is printed.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// One line per group, filtered by `-d`/`-u`.
    First,
    /// Every line of the groups with duplicates (`-D`).
    AllRepeated(Delimit),
    /// Every line, groups separated (`--group`).
    Group(Delimit),
}

struct Key {
    fields: usize,
    chars: usize,
    check: Option<usize>,
    fold: bool,
}

impl Key {
    /// The part of `line` that is compared.
    fn of<'a>(&self, line: &'a [u8]) -> &'a [u8] {
        if self.fields == 0 && self.chars == 0 && self.check.is_none() {
            return line;
        }
        let mut i = 0;
        for _ in 0..self.fields {
            while i < line.len() && (line[i] == b' ' || line[i] == b'\t') {
                i += 1;
            }
            while i < line.len() && line[i] != b' ' && line[i] != b'\t' {
                i += 1;
            }
        }
        let rest = &line[i..];
        let skip: usize = char_lengths(rest).take(self.chars).sum();
        let rest = &rest[skip..];
        match self.check {
            Some(n) => &rest[..char_lengths(rest).take(n).sum::<usize>()],
            None => rest,
        }
    }

    fn equal(&self, a: &[u8], b: &[u8]) -> bool {
        let (a, b) = (self.of(a), self.of(b));
        if self.fold {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    }
}

/// Where the output goes: stdout or the OUTPUT operand.
struct Out {
    file: Option<UtilFile>,
    buf: Vec<u8>,
}

impl Out {
    async fn flush(&mut self, ctx: &mut UtilCtx<'_>) -> std::io::Result<()> {
        let r = match &self.file {
            Some(f) => f.write_all(&self.buf).await,
            None => ctx.write(&self.buf).await,
        };
        self.buf.clear();
        r
    }
}

fn parse_n(v: &[u8], what: &str) -> Result<usize, String> {
    let s = String::from_utf8_lossy(v);
    match s.parse::<u64>() {
        Ok(n) => Ok(n.min(usize::MAX as u64) as usize),
        Err(_) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => Ok(usize::MAX),
        Err(_) => Err(format!("{s}: invalid number of {what}")),
    }
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let count = p.has(&SPEC, "count");
    let (mut repeated, unique) = (p.has(&SPEC, "repeated"), p.has(&SPEC, "unique"));
    let mut key = Key {
        fields: 0,
        chars: 0,
        check: None,
        fold: p.has(&SPEC, "ignore-case"),
    };
    let mut mode = Mode::First;
    for (k, v) in p.iter(&SPEC) {
        let v = v.unwrap_or_default();
        let r = match k.as_str() {
            "skip-fields" => parse_n(v, "fields to skip").map(|n| key.fields = n),
            "skip-chars" => parse_n(v, "bytes to skip").map(|n| key.chars = n),
            "check-chars" => parse_n(v, "bytes to compare").map(|n| key.check = Some(n)),
            "D" => {
                mode = Mode::AllRepeated(Delimit::None);
                Ok(())
            }
            "all-repeated" | "group" => {
                let all = k == "all-repeated";
                let d = match (v, all) {
                    (b"" | b"none", true) => Some(Delimit::None),
                    (b"prepend", _) => Some(Delimit::Prepend),
                    (b"separate", _) | (b"", false) => Some(Delimit::Separate),
                    (b"append", false) => Some(Delimit::Append),
                    (b"both", false) => Some(Delimit::Both),
                    _ => None,
                };
                let Some(d) = d else {
                    let v = common::quote(ctx, v);
                    let opt = common::quote(ctx, format!("--{k}").as_bytes());
                    return common::usage(ctx, &format!("invalid argument {v} for {opt}")).await;
                };
                mode = if all {
                    Mode::AllRepeated(d)
                } else {
                    Mode::Group(d)
                };
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = r {
            ctx.error(&e).await;
            return 1;
        }
    }
    if matches!(mode, Mode::Group(_)) && (count || repeated || unique || p.has(&SPEC, "D")) {
        return common::usage(ctx, "--group is mutually exclusive with -c/-d/-D/-u").await;
    }
    if matches!(mode, Mode::AllRepeated(_)) {
        if count {
            return common::usage(
                ctx,
                "printing all duplicated lines and repeat counts is meaningless",
            )
            .await;
        }
        repeated = true;
    }
    if p.operands.len() > 2 {
        let o = common::quote(ctx, &p.operands[2]);
        return common::usage(ctx, &format!("extra operand {o}")).await;
    }
    let eol = if p.has(&SPEC, "zero-terminated") {
        0
    } else {
        b'\n'
    };
    let in_name = p.operands.first().cloned().unwrap_or_else(|| b"-".to_vec());
    let Some(input) = textio::open_or_report(ctx, &in_name).await else {
        return 1;
    };
    let mut out = Out {
        file: None,
        buf: Vec::new(),
    };
    if let Some(o) = p.operands.get(1)
        && o != b"-"
    {
        match ctx.open(o, OpenOptions::write()).await {
            Ok(f) => out.file = Some(f),
            Err(e) => {
                common::file_error(ctx, o, &e).await;
                return 1;
            }
        }
    }
    let status = filter(
        ctx, &input, &in_name, &mut out, &key, mode, count, repeated, unique, eol,
    )
    .await;
    if status != 0 {
        return status;
    }
    if let Err(e) = out.flush(ctx).await {
        return common::write_failed(ctx, &e).await;
    }
    0
}

#[allow(clippy::too_many_arguments)]
async fn filter(
    ctx: &mut UtilCtx<'_>,
    input: &Input,
    in_name: &[u8],
    out: &mut Out,
    key: &Key,
    mode: Mode,
    count: bool,
    repeated: bool,
    unique: bool,
    eol: u8,
) -> i32 {
    let mut reader = LineReader::new(eol);
    let mut steps: u32 = 0;
    let mut first: Option<Vec<u8>> = None;
    let mut n: u64 = 0;
    let mut groups: u64 = 0;
    // Prints the group that just ended (one-line modes).
    let finish = |out: &mut Out, first: &[u8], n: u64| {
        if (repeated && n < 2) || (unique && n > 1) {
            return;
        }
        if count {
            out.buf.extend_from_slice(format!("{n:>7} ").as_bytes());
        }
        out.buf.extend_from_slice(first);
        out.buf.push(eol);
    };
    loop {
        let line = match reader.buffered() {
            Some(r) => reader.get(r),
            None => match reader.next_ref(ctx, input).await {
                Ok(Some((line, _))) => line,
                Ok(None) => break,
                Err(e) => {
                    common::file_error(ctx, in_name, &e).await;
                    return 1;
                }
            },
        };
        let same = first.as_deref().is_some_and(|f| key.equal(f, line));
        if same {
            n += 1;
            match mode {
                Mode::AllRepeated(d) => {
                    if n == 2 {
                        let sep = match d {
                            Delimit::Prepend => true,
                            Delimit::Separate => groups > 0,
                            _ => false,
                        };
                        if sep {
                            out.buf.push(eol);
                        }
                        groups += 1;
                        out.buf
                            .extend_from_slice(first.as_deref().unwrap_or_default());
                        out.buf.push(eol);
                    }
                    out.buf.extend_from_slice(line);
                    out.buf.push(eol);
                }
                Mode::Group(_) => {
                    out.buf.extend_from_slice(line);
                    out.buf.push(eol);
                }
                Mode::First => {}
            }
        } else {
            if let Some(f) = &first {
                if mode == Mode::First {
                    finish(out, f, n);
                }
                if let Mode::Group(Delimit::Append | Delimit::Both) = mode {
                    out.buf.push(eol);
                }
            }
            if let Mode::Group(d) = mode {
                let sep = match d {
                    Delimit::Prepend | Delimit::Both => true,
                    Delimit::Separate => first.is_some(),
                    _ => false,
                };
                if sep {
                    out.buf.push(eol);
                }
                out.buf.extend_from_slice(line);
                out.buf.push(eol);
            }
            match &mut first {
                Some(f) => {
                    f.clear();
                    f.extend_from_slice(line);
                }
                None => first = Some(line.to_vec()),
            }
            n = 1;
        }
        if out.buf.len() >= 32 * 1024
            && let Err(e) = out.flush(ctx).await
        {
            return common::write_failed(ctx, &e).await;
        }
        steps = steps.wrapping_add(1);
        if steps.is_multiple_of(256) {
            ctx.check().await;
        }
    }
    if let Some(f) = &first {
        if mode == Mode::First {
            finish(out, f, n);
        }
        if let Mode::Group(Delimit::Append | Delimit::Both) = mode {
            out.buf.push(eol);
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        let k = Key {
            fields: 1,
            chars: 1,
            check: Some(2),
            fold: false,
        };
        // -s counts from the end of the skipped fields, blanks included.
        assert_eq!(k.of(b"  one  two three"), b" t");
        assert_eq!(k.of(b"x"), b"");
        let k = Key {
            fields: 0,
            chars: 0,
            check: None,
            fold: true,
        };
        assert!(k.equal(b"ABC", b"abc"));
    }
}
