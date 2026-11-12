//! `head`: the first lines or bytes, GNU options (`-n N`, `-n -N`, `-c N`, `-c -N`,
//! `-q`, `-v`, `-z`) and the old `-N` form. Written for psh after GNU's documented
//! behavior.

use psh_engine::util::UtilCtx;

use crate::common::{self, Input};
use crate::opts::{Spec, both, long};

pub const SPEC: Spec = Spec {
    name: "head",
    opts: &[
        both('n', "lines").value(),
        both('c', "bytes").value(),
        both('q', "quiet"),
        long("silent"),
        both('v', "verbose"),
        both('z', "zero-terminated"),
    ],
    posix_order: false,
};

/// What to print of each input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Amount {
    FirstLines(u64),
    AllButLastLines(u64),
    FirstBytes(u64),
    AllButLastBytes(u64),
}

/// `-5` and `-5c`/`-5l` (the obsolete form) as `-n 5` / `-c 5`, when first.
pub fn rewrite_obsolete(argv: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = argv.to_vec();
    if let Some(a) = argv.get(1)
        && a.len() > 1
        && a[0] == b'-'
        && a[1].is_ascii_digit()
    {
        let digits = a[1..].iter().take_while(|b| b.is_ascii_digit()).count();
        let num = a[1..1 + digits].to_vec();
        let rest = &a[1 + digits..];
        let flag = match rest {
            b"" | b"l" => b"-n".to_vec(),
            b"c" => b"-c".to_vec(),
            _ => return out,
        };
        out.splice(1..2, [flag, num]);
    }
    out
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let argv = rewrite_obsolete(argv);
    let p = match common::parse(ctx, &SPEC, &argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let mut amount = Amount::FirstLines(10);
    let (mut quiet, mut verbose, mut zero) = (false, false, false);
    for (key, value) in p.iter(&SPEC) {
        match key.as_str() {
            "lines" | "bytes" => {
                let v = value.unwrap_or_default();
                let (neg, num) = match v.strip_prefix(b"-") {
                    Some(n) => (true, n),
                    None => (false, v.strip_prefix(b"+").unwrap_or(v)),
                };
                let Some(n) = common::parse_count(num) else {
                    let what = if key == "lines" { "lines" } else { "bytes" };
                    let msg = format!("invalid number of {what}: {}", common::quote(ctx, v));
                    ctx.error(&msg).await;
                    return 1;
                };
                amount = match (key.as_str(), neg) {
                    ("lines", false) => Amount::FirstLines(n),
                    ("lines", true) => Amount::AllButLastLines(n),
                    (_, false) => Amount::FirstBytes(n),
                    (_, true) => Amount::AllButLastBytes(n),
                };
            }
            "quiet" | "silent" => (quiet, verbose) = (true, false),
            "verbose" => (quiet, verbose) = (false, true),
            "zero-terminated" => zero = true,
            _ => {}
        }
    }
    let delim = if zero { 0 } else { b'\n' };
    let files = common::operands_or_stdin(&p);
    let headers = verbose || (files.len() > 1 && !quiet);
    let mut status = 0;
    for (i, name) in files.iter().enumerate() {
        let input = match Input::open(ctx, name).await {
            Ok(i) => i,
            Err(e) => {
                ctx.error(&format!(
                    "cannot open '{}' for reading: {}",
                    String::from_utf8_lossy(name),
                    psh_engine::host::error_text(&e)
                ))
                .await;
                status = 1;
                continue;
            }
        };
        if headers {
            let shown = if name == b"-" {
                "standard input".to_string()
            } else {
                String::from_utf8_lossy(name).into_owned()
            };
            let sep = if i > 0 { "\n" } else { "" };
            if let Err(e) = ctx
                .write(format!("{sep}==> {shown} <==\n").as_bytes())
                .await
            {
                return common::write_failed(ctx, &e).await;
            }
        }
        match copy(ctx, &input, amount, delim).await {
            Ok(()) => {}
            Err(Failure::Write(e)) => return common::write_failed(ctx, &e).await,
            Err(Failure::Read(e)) => {
                ctx.error(&format!(
                    "error reading '{}': {}",
                    String::from_utf8_lossy(name),
                    psh_engine::host::error_text(&e)
                ))
                .await;
                status = 1;
            }
        }
    }
    status
}

pub enum Failure {
    Read(std::io::Error),
    Write(std::io::Error),
}

async fn copy(
    ctx: &mut UtilCtx<'_>,
    input: &Input,
    amount: Amount,
    delim: u8,
) -> Result<(), Failure> {
    let mut buf = vec![0u8; 64 * 1024];
    match amount {
        Amount::FirstLines(mut left) | Amount::FirstBytes(mut left) => {
            let lines = matches!(amount, Amount::FirstLines(_));
            while left > 0 {
                let n = input.read(ctx, &mut buf).await.map_err(Failure::Read)?;
                if n == 0 {
                    break;
                }
                let take = if lines {
                    let mut end = n;
                    for i in memchr::memchr_iter(delim, &buf[..n]) {
                        left -= 1;
                        if left == 0 {
                            end = i + 1;
                            break;
                        }
                    }
                    end
                } else {
                    let t = (n as u64).min(left) as usize;
                    left -= t as u64;
                    t
                };
                ctx.write(&buf[..take]).await.map_err(Failure::Write)?;
                ctx.check().await;
            }
            // Stop here: the rest of the input is not read, so a writer into a pipe gets
            // EPIPE once the command ends, as with GNU head.
        }
        Amount::AllButLastLines(n) | Amount::AllButLastBytes(n) => {
            let mut all = Vec::new();
            input
                .read_to_end(ctx, &mut all)
                .await
                .map_err(Failure::Read)?;
            let end = if matches!(amount, Amount::AllButLastBytes(_)) {
                all.len().saturating_sub(n as usize)
            } else {
                cut_last_lines(&all, n, delim)
            };
            ctx.write(&all[..end]).await.map_err(Failure::Write)?;
        }
    }
    Ok(())
}

/// The length of `data` without its last `n` lines (a final line without a delimiter
/// counts as a line).
pub fn cut_last_lines(data: &[u8], n: u64, delim: u8) -> usize {
    if data.is_empty() {
        return 0;
    }
    if n == 0 {
        return data.len();
    }
    let mut seen = 0;
    for i in memchr::memrchr_iter(delim, &data[..data.len() - 1]) {
        seen += 1;
        if seen == n {
            return i + 1;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obsolete_form() {
        let a = |v: &[&str]| v.iter().map(|s| s.as_bytes().to_vec()).collect::<Vec<_>>();
        assert_eq!(
            rewrite_obsolete(&a(&["head", "-5", "f"])),
            a(&["head", "-n", "5", "f"])
        );
        assert_eq!(
            rewrite_obsolete(&a(&["head", "-5c"])),
            a(&["head", "-c", "5"])
        );
        assert_eq!(
            rewrite_obsolete(&a(&["head", "-n", "5"])),
            a(&["head", "-n", "5"])
        );
    }

    #[test]
    fn all_but_last_lines() {
        assert_eq!(cut_last_lines(b"a\nb\nc\n", 1, b'\n'), 4);
        assert_eq!(cut_last_lines(b"a\nb\nc\n", 2, b'\n'), 2);
        assert_eq!(cut_last_lines(b"a\nb\nc\n", 5, b'\n'), 0);
        assert_eq!(cut_last_lines(b"a\nb\nc", 1, b'\n'), 4);
        assert_eq!(cut_last_lines(b"a\nb\nc", 2, b'\n'), 2);
        assert_eq!(cut_last_lines(b"", 1, b'\n'), 0);
        assert_eq!(cut_last_lines(b"a\n", 0, b'\n'), 2);
    }
}
