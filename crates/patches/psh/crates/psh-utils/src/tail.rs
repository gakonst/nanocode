//! `tail`: the last lines or bytes, GNU options (`-n [+]N`, `-c [+]N`, `-f`, `-F`,
//! `--follow[=name|descriptor]`, `--retry`, `-s`, `--pid`, `-q`, `-v`, `-z`), the old
//! `-N`/`+N` forms and BSD's `-r` (reverse). Following is a timer loop on the host and
//! ends only when the command is cancelled. Written for psh after GNU's and BSD's
//! documented behavior.

use std::io::{self, SeekFrom};
use std::time::Duration;

use psh_engine::host::FileKind;
use psh_engine::util::{UtilCtx, UtilFile};

use crate::common::{self, Input};
use crate::opts::{Spec, both, flag, long};

pub const SPEC: Spec = Spec {
    name: "tail",
    opts: &[
        both('n', "lines").value(),
        both('c', "bytes").value(),
        flag('f'),
        long("follow").optional(),
        flag('F'),
        long("retry"),
        both('s', "sleep-interval").value(),
        long("pid").value(),
        both('q', "quiet"),
        long("silent"),
        both('v', "verbose"),
        both('z', "zero-terminated"),
        flag('r'),
    ],
    posix_order: false,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Amount {
    LastLines(u64),
    FromLine(u64),
    LastBytes(u64),
    FromByte(u64),
}

/// `-5`, `+5`, `-5c`, `+5c`, `-5r` (obsolete forms, first argument only) as options.
fn rewrite_obsolete(argv: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = argv.to_vec();
    let Some(a) = argv.get(1) else { return out };
    if a.len() < 2 || !(a[0] == b'-' || a[0] == b'+') || !a[1].is_ascii_digit() {
        return out;
    }
    let digits = a[1..].iter().take_while(|b| b.is_ascii_digit()).count();
    let num = &a[1..1 + digits];
    let rest = &a[1 + digits..];
    let mut value = Vec::new();
    if a[0] == b'+' {
        value.push(b'+');
    }
    value.extend_from_slice(num);
    let mut new = match rest {
        b"" | b"l" => vec![b"-n".to_vec(), value],
        b"c" => vec![b"-c".to_vec(), value],
        b"f" => vec![b"-n".to_vec(), value, b"-f".to_vec()],
        b"r" => vec![b"-n".to_vec(), value, b"-r".to_vec()],
        _ => return out,
    };
    out.splice(1..2, new.drain(..));
    out
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let argv = rewrite_obsolete(argv);
    let p = match common::parse(ctx, &SPEC, &argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let mut amount = Amount::LastLines(10);
    let (mut quiet, mut verbose, mut zero, mut follow, mut reverse) =
        (false, false, false, false, false);
    let mut interval = Duration::from_secs(1);
    let mut explicit = false;
    for (key, value) in p.iter(&SPEC) {
        match key.as_str() {
            "lines" | "bytes" => {
                explicit = true;
                let v = value.unwrap_or_default();
                let (from, num) = match v.strip_prefix(b"+") {
                    Some(n) => (true, n),
                    None => (false, v.strip_prefix(b"-").unwrap_or(v)),
                };
                let Some(n) = common::parse_count(num) else {
                    let what = if key == "lines" { "lines" } else { "bytes" };
                    let msg = format!("invalid number of {what}: {}", common::quote(ctx, v));
                    ctx.error(&msg).await;
                    return 1;
                };
                amount = match (key.as_str(), from) {
                    ("lines", false) => Amount::LastLines(n),
                    ("lines", true) => Amount::FromLine(n),
                    (_, false) => Amount::LastBytes(n),
                    (_, true) => Amount::FromByte(n),
                };
            }
            "f" | "follow" | "F" => follow = true,
            "sleep-interval" => match common::parse_duration(value.unwrap_or_default()) {
                Some(d) if d.is_finite() => interval = Duration::from_secs_f64(d),
                _ => {
                    let msg = format!(
                        "invalid number of seconds: {}",
                        common::quote(ctx, value.unwrap_or_default())
                    );
                    ctx.error(&msg).await;
                    return 1;
                }
            },
            "quiet" | "silent" => (quiet, verbose) = (true, false),
            "verbose" => (quiet, verbose) = (false, true),
            "zero-terminated" => zero = true,
            "r" => reverse = true,
            _ => {}
        }
    }
    // BSD: `-r` without a count reverses the whole input.
    if reverse && !explicit {
        amount = Amount::FromLine(1);
    }
    let delim = if zero { 0 } else { b'\n' };
    let files = common::operands_or_stdin(&p);
    let headers = verbose || (files.len() > 1 && !quiet);
    let mut status = 0;
    // Files to follow, with how much of each was printed.
    let mut followed: Vec<(Vec<u8>, usize)> = Vec::new();
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
        let last = match &input {
            Input::File(file) => read_last(file, amount, delim).await,
            Input::Stdin => None,
        };
        let (data, size) = match last {
            Some(Ok(last)) => last,
            Some(Err(e)) => {
                common::file_error(ctx, name, &e).await;
                status = 1;
                continue;
            }
            None => {
                let mut data = Vec::new();
                if let Err(e) = input.read_to_end(ctx, &mut data).await {
                    common::file_error(ctx, name, &e).await;
                    status = 1;
                    continue;
                }
                let size = data.len();
                (data, size)
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
        let start = start_of(&data, amount, delim);
        let out = if reverse {
            reverse_lines(&data[start..], delim)
        } else {
            data[start..].to_vec()
        };
        if let Err(e) = ctx.write(&out).await {
            return common::write_failed(ctx, &e).await;
        }
        // Only regular files are followed: a pipe has nothing more after its end.
        if follow
            && name != b"-"
            && matches!(ctx.stat(name, true).await, Ok(m) if m.kind == FileKind::File)
        {
            followed.push((name.clone(), size));
        }
    }
    if followed.is_empty() {
        return status;
    }
    if let Err(e) = ctx.flush().await {
        return common::write_failed(ctx, &e).await;
    }
    // Follow until cancelled: print what was appended; start over after truncation.
    let mut last_shown: Option<usize> = (followed.len() == 1).then_some(0);
    loop {
        ctx.sleep(interval).await;
        ctx.check().await;
        for (i, entry) in followed.iter_mut().enumerate() {
            let (name, done) = entry.clone();
            let Ok(input) = Input::open(ctx, &name).await else {
                continue;
            };
            let mut data = Vec::new();
            if input.read_to_end(ctx, &mut data).await.is_err() {
                continue;
            }
            let from = if data.len() < done { 0 } else { done };
            if data.len() > from {
                if headers && last_shown != Some(i) {
                    let h = format!("\n==> {} <==\n", String::from_utf8_lossy(&name));
                    if let Err(e) = ctx.write(h.as_bytes()).await {
                        return common::write_failed(ctx, &e).await;
                    }
                    last_shown = Some(i);
                }
                if let Err(e) = ctx.write(&data[from..]).await {
                    return common::write_failed(ctx, &e).await;
                }
                if let Err(e) = ctx.flush().await {
                    return common::write_failed(ctx, &e).await;
                }
            }
            entry.1 = data.len();
        }
    }
}

async fn read_last(
    file: &UtilFile,
    amount: Amount,
    delim: u8,
) -> Option<io::Result<(Vec<u8>, usize)>> {
    let want = match amount {
        Amount::LastLines(n) | Amount::LastBytes(n) => n,
        Amount::FromLine(_) | Amount::FromByte(_) => return None,
    };
    if file.kind() != Some(FileKind::File) {
        return None;
    }
    let Ok(len) = file.seek(SeekFrom::End(0))? else {
        return None;
    };
    let mut from = len;
    let mut data = Vec::new();
    loop {
        let next = match amount {
            Amount::LastBytes(n) => len.saturating_sub(n),
            _ => from.saturating_sub((len - from).max(64 * 1024)),
        };
        if let Err(e) = file.seek(SeekFrom::Start(next))? {
            return Some(Err(e));
        }
        let mut chunk = vec![0u8; (from - next) as usize];
        let mut got = 0;
        while got < chunk.len() {
            match file.read(&mut chunk[got..]).await {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) => return Some(Err(e)),
            }
        }
        if got < chunk.len() {
            return match file.seek(SeekFrom::Start(0))? {
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            };
        }
        chunk.extend_from_slice(&data);
        data = chunk;
        from = next;
        if from == 0
            || matches!(amount, Amount::LastBytes(_))
            || memchr::memchr_iter(delim, &data).count() as u64 > want
        {
            return Some(Ok((data, len as usize)));
        }
    }
}

/// Where the output starts.
fn start_of(data: &[u8], amount: Amount, delim: u8) -> usize {
    match amount {
        Amount::LastBytes(n) => data.len().saturating_sub(n as usize),
        Amount::FromByte(n) => (n.saturating_sub(1) as usize).min(data.len()),
        Amount::LastLines(n) => crate::head::cut_last_lines(data, n, delim),
        Amount::FromLine(n) => {
            // Line n (1-based; 0 counts as 1) starts after n-1 delimiters.
            let mut need = n.saturating_sub(1);
            if need == 0 {
                return 0;
            }
            for (i, &b) in data.iter().enumerate() {
                if b == delim {
                    need -= 1;
                    if need == 0 {
                        return i + 1;
                    }
                }
            }
            data.len()
        }
    }
}

/// BSD `tail -r`: the lines in reverse order (a last line without a delimiter gets one).
fn reverse_lines(data: &[u8], delim: u8) -> Vec<u8> {
    let mut lines: Vec<&[u8]> = data.split_inclusive(|&b| b == delim).collect();
    lines.reverse();
    let mut out = Vec::with_capacity(data.len() + 1);
    for l in lines {
        out.extend_from_slice(l);
        if l.last() != Some(&delim) {
            out.push(delim);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts() {
        let d = b"a\nb\nc\n";
        assert_eq!(start_of(d, Amount::LastLines(1), b'\n'), 4);
        assert_eq!(start_of(d, Amount::FromLine(2), b'\n'), 2);
        assert_eq!(start_of(d, Amount::FromLine(0), b'\n'), 0);
        assert_eq!(start_of(d, Amount::FromLine(9), b'\n'), 6);
        assert_eq!(start_of(d, Amount::LastBytes(2), b'\n'), 4);
        assert_eq!(start_of(d, Amount::FromByte(2), b'\n'), 1);
    }

    #[test]
    fn obsolete_and_reverse() {
        let a = |v: &[&str]| v.iter().map(|s| s.as_bytes().to_vec()).collect::<Vec<_>>();
        assert_eq!(
            rewrite_obsolete(&a(&["tail", "+3", "f"])),
            a(&["tail", "-n", "+3", "f"])
        );
        assert_eq!(
            rewrite_obsolete(&a(&["tail", "-2r"])),
            a(&["tail", "-n", "2", "-r"])
        );
        assert_eq!(reverse_lines(b"a\nb\nc", b'\n'), b"c\nb\na\n");
    }
}
