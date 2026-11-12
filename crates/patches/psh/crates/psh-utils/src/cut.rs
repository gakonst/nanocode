//! `cut -b|-c|-f LIST` with `-d -s -n -z --complement --output-delimiter` and BSD's
//! `-w`. Written for psh after GNU's documented behavior.
//!
//! `-c` counts UTF-8 characters, in every locale (D1161): agents mean characters, and
//! GNU's `-c` (bytes, like `-b`) splits multibyte characters. BSD's `cut` in a UTF-8
//! locale agrees with psh. Bytes that are not valid UTF-8 count as one character each.

use psh_engine::util::UtilCtx;

use crate::common;
use crate::opts::{Spec, both, flag, long};
use crate::textio::{self, LineReader, char_lengths};

pub const SPEC: Spec = Spec {
    name: "cut",
    opts: &[
        both('b', "bytes").value(),
        both('c', "characters").value(),
        both('f', "fields").value(),
        both('d', "delimiter").value(),
        both('s', "only-delimited"),
        flag('n'),
        both('z', "zero-terminated"),
        long("complement"),
        long("output-delimiter").value(),
        flag('w'),
    ],
    posix_order: false,
};

/// 1-based inclusive ranges, sorted and merged; `usize::MAX` is an open end.
type Ranges = Vec<(usize, usize)>;

fn parse_list(list: &[u8], fields: bool, utf8: bool) -> Result<Ranges, String> {
    let text = String::from_utf8_lossy(list);
    let mut out = Vec::new();
    let what = if fields {
        "fields are"
    } else {
        "byte/character positions are"
    };
    for part in text.split([',', ' ', '\t']) {
        if part.is_empty() {
            continue;
        }
        let num = |s: &str| -> Result<usize, String> {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                let kind = if fields {
                    "field value"
                } else {
                    "byte/character position"
                };
                return Err(format!(
                    "invalid {kind} {}",
                    common::quote_in(utf8, part.as_bytes())
                ));
            }
            match s.parse::<usize>() {
                Ok(0) => Err(format!("{what} numbered from 1")),
                Ok(n) => Ok(n),
                Err(_) => Ok(usize::MAX - 1),
            }
        };
        let (lo, hi) = match part.split_once('-') {
            None => {
                let n = num(part)?;
                (n, n)
            }
            Some(("", "")) => return Err("invalid range with no endpoint: -".into()),
            Some(("", h)) => (1, num(h)?),
            Some((l, "")) => (num(l)?, usize::MAX),
            Some((l, h)) => {
                let (l, h) = (num(l)?, num(h)?);
                if h < l {
                    return Err("invalid decreasing range".into());
                }
                (l, h)
            }
        };
        out.push((lo, hi));
    }
    if out.is_empty() {
        return Err(format!("{what} numbered from 1"));
    }
    out.sort_unstable();
    let mut merged: Ranges = Vec::new();
    for (lo, hi) in out {
        match merged.last_mut() {
            Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
            _ => merged.push((lo, hi)),
        }
    }
    Ok(merged)
}

/// The positions not in `r`.
fn complement(r: &Ranges) -> Ranges {
    let mut out = Vec::new();
    let mut next = 1;
    for &(lo, hi) in r {
        if lo > next {
            out.push((next, lo - 1));
        }
        if hi == usize::MAX {
            return out;
        }
        next = hi + 1;
    }
    out.push((next, usize::MAX));
    out
}

/// The range index containing position `pos`, if any.
fn range_of(r: &Ranges, pos: usize) -> Option<usize> {
    r.iter().position(|&(lo, hi)| lo <= pos && pos <= hi)
}

enum Mode {
    Bytes,
    Chars,
    Fields,
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let mut mode = None;
    let mut list = Vec::new();
    for (key, value) in p.iter(&SPEC) {
        let m = match key.as_str() {
            "bytes" => Mode::Bytes,
            "characters" => Mode::Chars,
            "fields" => Mode::Fields,
            _ => continue,
        };
        if mode.is_some() {
            return common::usage(ctx, "only one list may be specified").await;
        }
        mode = Some(m);
        list = value.unwrap_or_default().to_vec();
    }
    let Some(mode) = mode else {
        return common::usage(
            ctx,
            "you must specify a list of bytes, characters, or fields",
        )
        .await;
    };
    let fields = matches!(mode, Mode::Fields);
    let whitespace = p.has(&SPEC, "w");
    let delim = p.last(&SPEC, "delimiter").map(<[u8]>::to_vec);
    if !fields && (delim.is_some() || whitespace) {
        return common::usage(
            ctx,
            "an input delimiter may be specified only when operating on fields",
        )
        .await;
    }
    if !fields && p.has(&SPEC, "only-delimited") {
        return common::usage(
            ctx,
            "suppressing non-delimited lines makes sense\n\tonly when operating on fields",
        )
        .await;
    }
    let delim = match delim {
        // An empty delimiter is NUL, as in GNU.
        Some(d) if d.is_empty() => vec![0],
        Some(d) => {
            if char_lengths(&d).count() != 1 {
                return common::usage(ctx, "the delimiter must be a single character").await;
            }
            d
        }
        None => vec![b'\t'],
    };
    let mut ranges = match parse_list(&list, fields, ctx.utf8()) {
        Ok(r) => r,
        Err(e) => return common::usage(ctx, &e).await,
    };
    if p.has(&SPEC, "complement") {
        ranges = complement(&ranges);
    }
    let out_delim: Option<Vec<u8>> = p.last(&SPEC, "output-delimiter").map(<[u8]>::to_vec);
    let only_delimited = p.has(&SPEC, "only-delimited");
    let eol = if p.has(&SPEC, "zero-terminated") {
        0
    } else {
        b'\n'
    };
    let mut status = 0;
    let mut out = Vec::new();
    let tty = ctx.is_terminal(1);
    for name in common::operands_or_stdin(&p) {
        let Some(input) = textio::open_or_report(ctx, &name).await else {
            status = 1;
            continue;
        };
        let mut reader = LineReader::new(eol);
        loop {
            let line = match reader.buffered() {
                Some(r) => reader.get(r),
                None => match reader.next_ref(ctx, &input).await {
                    Ok(Some((line, _))) => line,
                    Ok(None) => break,
                    Err(e) => {
                        common::file_error(ctx, &name, &e).await;
                        status = 1;
                        break;
                    }
                },
            };
            let mark = out.len();
            let keep = match mode {
                Mode::Bytes => {
                    cut_bytes(line, &ranges, out_delim.as_deref(), &mut out);
                    true
                }
                Mode::Chars if line.is_ascii() => {
                    cut_bytes(line, &ranges, out_delim.as_deref(), &mut out);
                    true
                }
                Mode::Chars => {
                    cut_units(
                        line,
                        char_lengths(line),
                        &ranges,
                        out_delim.as_deref(),
                        &mut out,
                    );
                    true
                }
                Mode::Fields => {
                    let od: &[u8] = match (&out_delim, whitespace) {
                        (Some(d), _) => d,
                        (None, true) => b"\t",
                        (None, false) => &delim,
                    };
                    cut_fields(
                        line,
                        &delim,
                        whitespace,
                        &ranges,
                        od,
                        only_delimited,
                        &mut out,
                    )
                }
            };
            if keep {
                out.push(eol);
            } else {
                out.truncate(mark);
            }
            if (tty || out.len() >= 64 * 1024) && !out.is_empty() {
                if let Err(e) = ctx.write(&out).await {
                    return common::write_failed(ctx, &e).await;
                }
                out.clear();
            }
        }
    }
    if !out.is_empty()
        && let Err(e) = ctx.write(&out).await
    {
        return common::write_failed(ctx, &e).await;
    }
    status
}

/// Bytes or characters: the units of `line` (by their lengths) in the ranges, with the
/// output delimiter between separate ranges.
fn cut_units(
    line: &[u8],
    lens: impl Iterator<Item = usize>,
    ranges: &Ranges,
    od: Option<&[u8]>,
    out: &mut Vec<u8>,
) {
    let mut at = 0;
    let mut last_range = None;
    for (i, len) in lens.enumerate() {
        let unit = &line[at..at + len];
        at += len;
        if let Some(r) = range_of(ranges, i + 1) {
            if let (Some(d), Some(prev)) = (od, last_range)
                && prev != r
            {
                out.extend_from_slice(d);
            }
            last_range = Some(r);
            out.extend_from_slice(unit);
        }
    }
}

fn cut_bytes(line: &[u8], ranges: &Ranges, od: Option<&[u8]>, out: &mut Vec<u8>) {
    let mut first = true;
    for &(lo, hi) in ranges {
        if lo > line.len() {
            break;
        }
        if let (Some(d), false) = (od, first) {
            out.extend_from_slice(d);
        }
        first = false;
        out.extend_from_slice(&line[lo - 1..hi.min(line.len())]);
    }
}

/// Fields: `false` when the line has no delimiter and `-s` drops it.
fn cut_fields(
    line: &[u8],
    delim: &[u8],
    whitespace: bool,
    ranges: &Ranges,
    od: &[u8],
    only_delimited: bool,
    out: &mut Vec<u8>,
) -> bool {
    if let (false, &[d]) = (whitespace, delim) {
        if !line.contains(&d) {
            if only_delimited {
                return false;
            }
            out.extend_from_slice(line);
            return true;
        }
        let last = ranges.last().map_or(0, |r| r.1);
        let mut first = true;
        for (i, f) in line.split(|&b| b == d).enumerate() {
            if i >= last {
                break;
            }
            if range_of(ranges, i + 1).is_some() {
                if !first {
                    out.extend_from_slice(od);
                }
                first = false;
                out.extend_from_slice(f);
            }
        }
        return true;
    }
    let parts: Vec<&[u8]> = if whitespace {
        // BSD's -w: runs of blanks separate fields; leading blanks make an empty first.
        let mut v = Vec::new();
        let mut start = 0;
        let mut i = 0;
        while i < line.len() {
            if line[i] == b' ' || line[i] == b'\t' {
                v.push(&line[start..i]);
                while i < line.len() && (line[i] == b' ' || line[i] == b'\t') {
                    i += 1;
                }
                start = i;
            } else {
                i += 1;
            }
        }
        v.push(&line[start..]);
        v
    } else {
        split_on(line, delim)
    };
    if parts.len() == 1 {
        if only_delimited {
            return false;
        }
        out.extend_from_slice(line);
        return true;
    }
    let mut first = true;
    for (i, f) in parts.iter().enumerate() {
        if range_of(ranges, i + 1).is_some() {
            if !first {
                out.extend_from_slice(od);
            }
            first = false;
            out.extend_from_slice(f);
        }
    }
    true
}

fn split_on<'a>(line: &'a [u8], delim: &[u8]) -> Vec<&'a [u8]> {
    if delim.len() == 1 {
        return line.split(|&b| b == delim[0]).collect();
    }
    let mut v = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + delim.len() <= line.len() {
        if &line[i..i + delim.len()] == delim {
            v.push(&line[start..i]);
            i += delim.len();
            start = i;
        } else {
            i += 1;
        }
    }
    v.push(&line[start..]);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists() {
        assert_eq!(parse_list(b"1,3-4,2", true, false).unwrap(), vec![(1, 4)]);
        assert_eq!(
            parse_list(b"-2,5-", false, false).unwrap(),
            vec![(1, 2), (5, usize::MAX)]
        );
        assert!(parse_list(b"0", true, false).is_err());
        assert!(parse_list(b"3-1", true, false).is_err());
        assert!(parse_list(b"x", true, false).is_err());
        assert!(parse_list(b"-", true, false).is_err());
        assert_eq!(complement(&vec![(2, 3)]), vec![(1, 1), (4, usize::MAX)]);
        assert_eq!(complement(&vec![(1, usize::MAX)]), vec![]);
    }

    #[test]
    fn units() {
        let mut out = Vec::new();
        let line = "héllo".as_bytes();
        cut_units(line, char_lengths(line), &vec![(1, 2)], None, &mut out);
        assert_eq!(out, "hé".as_bytes());
    }
}
