//! `paste [-s] [-d LIST] [-z] FILE...`. Written for psh after GNU's documented
//! behavior. Where GNU and BSD differ, GNU (D016): every output line ends in a newline,
//! `-s` of an empty file prints an empty line, and `-d ''` means no delimiter.

use psh_engine::util::UtilCtx;

use crate::common::{self, Input};
use crate::opts::{Spec, both};
use crate::textio::{LineReader, char_lengths};

pub const SPEC: Spec = Spec {
    name: "paste",
    opts: &[
        both('d', "delimiters").value(),
        both('s', "serial"),
        both('z', "zero-terminated"),
    ],
    posix_order: false,
};

/// The delimiter list: each entry is one character, or empty (`\0`).
fn parse_delims(d: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < d.len() {
        if d[i] == b'\\' {
            let Some(&c) = d.get(i + 1) else {
                return Err(
                    "delimiter list ends with an unescaped backslash: ".to_string()
                        + &String::from_utf8_lossy(d),
                );
            };
            out.push(match c {
                b'0' => Vec::new(),
                b'n' => vec![b'\n'],
                b't' => vec![b'\t'],
                b'b' => vec![8],
                b'f' => vec![12],
                b'r' => vec![13],
                b'v' => vec![11],
                _ => vec![c],
            });
            i += 2;
            continue;
        }
        let n = char_lengths(&d[i..]).next().unwrap_or(1);
        out.push(d[i..i + n].to_vec());
        i += n;
    }
    if out.is_empty() {
        out.push(Vec::new());
    }
    Ok(out)
}

/// One operand: stdin is shared by every `-` (they take lines in turn).
struct Source {
    input: Option<Input>,
    stdin: bool,
    reader: LineReader,
    done: bool,
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let delims = match parse_delims(p.last(&SPEC, "delimiters").unwrap_or(b"\t")) {
        Ok(d) => d,
        Err(e) => {
            ctx.error(&e).await;
            return 1;
        }
    };
    let eol = if p.has(&SPEC, "zero-terminated") {
        0
    } else {
        b'\n'
    };
    let names = common::operands_or_stdin(&p);
    let mut sources = Vec::new();
    for name in &names {
        if name == b"-" {
            sources.push(Source {
                input: None,
                stdin: true,
                reader: LineReader::new(eol),
                done: false,
            });
            continue;
        }
        match Input::open(ctx, name).await {
            Ok(i) => sources.push(Source {
                input: Some(i),
                stdin: false,
                reader: LineReader::new(eol),
                done: false,
            }),
            Err(e) => {
                // GNU stops before any output when an operand cannot be opened.
                common::file_error(ctx, name, &e).await;
                return 1;
            }
        }
    }
    let mut stdin_reader = LineReader::new(eol);
    let stdin_input = Input::Stdin;
    let mut line = Vec::new();
    let mut out = Vec::new();
    let mut status = 0;

    // The next line of a source, or `None` at its end.
    macro_rules! next_line {
        ($s:expr, $name:expr) => {{
            let s: &mut Source = $s;
            if s.done {
                None
            } else {
                let r = if s.stdin {
                    stdin_reader.next(ctx, &stdin_input, &mut line).await
                } else {
                    let input = s.input.as_ref().expect("opened");
                    s.reader.next(ctx, input, &mut line).await
                };
                match r {
                    Ok(Some(_)) => Some(()),
                    Ok(None) => {
                        s.done = true;
                        None
                    }
                    Err(e) => {
                        common::file_error(ctx, $name, &e).await;
                        status = 1;
                        s.done = true;
                        None
                    }
                }
            }
        }};
    }

    if p.has(&SPEC, "serial") {
        for (k, s) in sources.iter_mut().enumerate() {
            out.clear();
            let mut n = 0;
            while next_line!(s, &names[k]).is_some() {
                if n > 0 {
                    out.extend_from_slice(&delims[(n - 1) % delims.len()]);
                }
                out.extend_from_slice(&line);
                n += 1;
                if out.len() > 32 * 1024 {
                    if let Err(e) = ctx.write(&out).await {
                        return common::write_failed(ctx, &e).await;
                    }
                    out.clear();
                }
            }
            out.push(eol);
            if let Err(e) = ctx.write(&out).await {
                return common::write_failed(ctx, &e).await;
            }
        }
        return status;
    }
    loop {
        out.clear();
        let mut any = false;
        for k in 0..sources.len() {
            if k > 0 {
                out.extend_from_slice(&delims[(k - 1) % delims.len()]);
            }
            if next_line!(&mut sources[k], &names[k]).is_some() {
                any = true;
                out.extend_from_slice(&line);
            }
        }
        if !any {
            break;
        }
        out.push(eol);
        if let Err(e) = ctx.write(&out).await {
            return common::write_failed(ctx, &e).await;
        }
        ctx.check().await;
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delimiters() {
        assert_eq!(
            parse_delims(b"+-").unwrap(),
            vec![b"+".to_vec(), b"-".to_vec()]
        );
        assert_eq!(parse_delims(b"\\0\\t").unwrap(), vec![vec![], vec![b'\t']]);
        assert_eq!(parse_delims(b"").unwrap(), vec![Vec::<u8>::new()]);
        assert!(parse_delims(b"\\").is_err());
        assert_eq!(parse_delims("é,".as_bytes()).unwrap().len(), 2);
    }
}
