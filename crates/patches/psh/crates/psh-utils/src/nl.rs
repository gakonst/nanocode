//! `nl` with `-b -h -f` styles `a t n` (`pREGEX` falls through), `-n ln|rn|rz`, `-w -s
//! -v -i -l -p -d` and logical pages. Written for psh after GNU's documented behavior.
//! Where GNU and BSD differ, GNU (D016): an unnumbered line is indented by the width
//! plus the separator's length in spaces, section delimiter lines print as empty lines,
//! and a last line without a newline gets one.

use psh_engine::util::UtilCtx;

use crate::common;
use crate::opts::{Spec, both};
use crate::textio::{self, LineReader};

pub const SPEC: Spec = Spec {
    name: "nl",
    opts: &[
        both('b', "body-numbering").value(),
        both('d', "section-delimiter").value(),
        both('f', "footer-numbering").value(),
        both('h', "header-numbering").value(),
        both('i', "line-increment").value(),
        both('l', "join-blank-lines").value(),
        both('n', "number-format").value(),
        both('p', "no-renumber"),
        both('s', "number-separator").value(),
        both('v', "starting-line-number").value(),
        both('w', "number-width").value(),
    ],
    posix_order: false,
};

#[derive(Clone, Copy, PartialEq)]
enum Style {
    All,
    NonEmpty,
    None,
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Left,
    Right,
    Zeros,
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    // header, body, footer
    let mut styles = [Style::None, Style::NonEmpty, Style::None];
    let mut format = Format::Right;
    let mut width: usize = 6;
    let mut sep = b"\t".to_vec();
    let mut start: i64 = 1;
    let mut incr: i64 = 1;
    let mut join: u64 = 1;
    let mut delim = b"\\:".to_vec();
    for (k, v) in p.iter(&SPEC) {
        let v = v.unwrap_or_default();
        let text = common::quote(ctx, v);
        let section = match k.as_str() {
            "header-numbering" => Some((0, "header")),
            "body-numbering" => Some((1, "body")),
            "footer-numbering" => Some((2, "footer")),
            _ => None,
        };
        if let Some((i, what)) = section {
            styles[i] = match v {
                b"a" => Style::All,
                b"t" => Style::NonEmpty,
                b"n" => Style::None,
                _ if v.starts_with(b"p") => {
                    return textio::fall_through(ctx, argv, "numbering style 'pREGEX'").await;
                }
                _ => {
                    return common::usage(ctx, &format!("invalid {what} numbering style: {text}"))
                        .await;
                }
            };
            continue;
        }
        let num = |what: &str, min: i64| -> Result<i64, String> {
            match String::from_utf8_lossy(v).parse::<i64>() {
                Ok(n) if n >= min => Ok(n),
                _ => Err(format!("invalid {what}: {text}")),
            }
        };
        let r = match k.as_str() {
            "number-format" => {
                format = match v {
                    b"ln" => Format::Left,
                    b"rn" => Format::Right,
                    b"rz" => Format::Zeros,
                    _ => {
                        return common::usage(
                            ctx,
                            &format!("invalid line numbering format: {text}"),
                        )
                        .await;
                    }
                };
                Ok(())
            }
            "number-width" => num("line number field width", 1).map(|n| width = n as usize),
            "number-separator" => {
                sep = v.to_vec();
                Ok(())
            }
            "starting-line-number" => num("starting line number", i64::MIN).map(|n| start = n),
            "line-increment" => num("line number increment", i64::MIN).map(|n| incr = n),
            "join-blank-lines" => num("line number of blank lines", 1).map(|n| join = n as u64),
            "section-delimiter" => {
                // One character keeps the second (`:`).
                delim = match v.len() {
                    0 => Vec::new(),
                    1 => vec![v[0], b':'],
                    _ => v.to_vec(),
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
    let renumber = !p.has(&SPEC, "no-renumber");
    let blank_indent = vec![b' '; width + sep.len()];
    let delims = [delim.repeat(3), delim.repeat(2), delim.clone()];

    let mut number = start;
    let mut section = 1;
    let mut blanks: u64 = 0;
    let mut status = 0;
    let mut line = Vec::new();
    let mut out = Vec::new();
    for name in common::operands_or_stdin(&p) {
        let Some(input) = textio::open_or_report(ctx, &name).await else {
            status = 1;
            continue;
        };
        let mut reader = LineReader::new(b'\n');
        loop {
            match reader.next(ctx, &input, &mut line).await {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => {
                    common::file_error(ctx, &name, &e).await;
                    status = 1;
                    break;
                }
            }
            out.clear();
            if !delim.is_empty()
                && let Some(s) = delims.iter().position(|d| *d == line)
            {
                section = s;
                if renumber {
                    number = start;
                }
                blanks = 0;
                out.push(b'\n');
            } else {
                let numbered = match styles[section] {
                    Style::All if line.is_empty() => {
                        blanks += 1;
                        if blanks >= join {
                            blanks = 0;
                            true
                        } else {
                            false
                        }
                    }
                    Style::All => {
                        blanks = 0;
                        true
                    }
                    Style::NonEmpty => !line.is_empty(),
                    Style::None => false,
                };
                if numbered {
                    let n = number;
                    let s = match format {
                        Format::Left => format!("{n:<width$}"),
                        Format::Right => format!("{n:>width$}"),
                        Format::Zeros if n < 0 => format!("-{:0>w$}", -n, w = width - 1),
                        Format::Zeros => format!("{n:0>width$}"),
                    };
                    out.extend_from_slice(s.as_bytes());
                    out.extend_from_slice(&sep);
                    number = number.wrapping_add(incr);
                } else {
                    out.extend_from_slice(&blank_indent);
                }
                out.extend_from_slice(&line);
                out.push(b'\n');
            }
            if let Err(e) = ctx.write(&out).await {
                return common::write_failed(ctx, &e).await;
            }
        }
    }
    status
}
