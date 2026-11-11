//! What every utility needs: option errors and fall-through, I/O errors, inputs, number
//! and duration parsing.

use std::io;

use psh_engine::host::{FileKind, OpenOptions, error_text};
use psh_engine::util::{UtilCtx, UtilFile};

use crate::opts::{OptError, Parsed, Spec};

/// Wraps `async fn name(ctx, argv) -> i32` into a [`psh_engine::util::UtilFn`].
macro_rules! util_fn {
    ($f:path) => {{
        fn wrap<'a>(
            ctx: &'a mut psh_engine::util::UtilCtx<'_>,
            argv: &'a [Vec<u8>],
        ) -> psh_engine::util::LocalBoxFuture<'a, i32> {
            Box::pin($f(ctx, argv))
        }
        wrap as psh_engine::util::UtilFn
    }};
}
pub(crate) use util_fn;

/// Parses argv[1..]; on an unknown option, runs the real program (D1082) or reports
/// it; on other usage errors, reports them as GNU does. `Err(status)` ends the utility.
pub async fn parse(ctx: &mut UtilCtx<'_>, spec: &Spec, argv: &[Vec<u8>]) -> Result<Parsed, i32> {
    match spec.parse(&argv[1..]) {
        Ok(p) => Ok(p),
        Err(e) => Err(option_error(ctx, spec, argv, &e).await),
    }
}

/// The status for an option error: the real program's when there is one for an unknown
/// option, else a message.
pub async fn option_error(
    ctx: &mut UtilCtx<'_>,
    spec: &Spec,
    argv: &[Vec<u8>],
    e: &OptError,
) -> i32 {
    if e.is_unknown() {
        if let Some(status) = ctx.run_program(argv.to_vec()).await {
            return status;
        }
        let name = spec.name;
        ctx.error(&format!(
            "unsupported option '{}' (psh built-in {name})",
            e.option()
        ))
        .await;
        return 2;
    }
    usage(ctx, &e.to_string()).await
}

/// A usage error in GNU's words: the message and the `--help` hint; status 1.
pub async fn usage(ctx: &mut UtilCtx<'_>, msg: &str) -> i32 {
    let name = ctx.name().to_string();
    ctx.error(msg).await;
    ctx.error_raw(format!("Try '{name} --help' for more information.\n").as_bytes())
        .await;
    1
}

/// A value quoted for a message as GNU quotes it (D1163): `‘x’` when the locale's
/// character set is UTF-8 (`LC_ALL`, `LC_CTYPE`, `LANG`), `'x'` otherwise. For values
/// and operands in messages (`invalid field value ‘x’`), not for the `NAME: FILE:
/// reason` prefix of file errors, which GNU leaves unquoted.
pub fn quote(ctx: &UtilCtx<'_>, s: &[u8]) -> String {
    quote_in(ctx.utf8(), s)
}

/// [`quote`] for code that has no context, given whether the locale is UTF-8.
pub fn quote_in(utf8: bool, s: &[u8]) -> String {
    let s = String::from_utf8_lossy(s);
    if utf8 {
        format!("\u{2018}{s}\u{2019}")
    } else {
        format!("'{s}'")
    }
}

/// The status after a failed write to stdout: a closed pipe ends the utility silently
/// like SIGPIPE ends a program (141); other errors are reported.
pub async fn write_failed(ctx: &mut UtilCtx<'_>, e: &io::Error) -> i32 {
    if e.kind() == io::ErrorKind::BrokenPipe {
        return 141;
    }
    ctx.error(&format!("write error: {}", error_text(e))).await;
    1
}

/// `NAME: FILE: reason` for a file that cannot be read.
pub async fn file_error(ctx: &mut UtilCtx<'_>, file: &[u8], e: &io::Error) {
    let text = format!("{}: {}", String::from_utf8_lossy(file), error_text(e));
    ctx.error(&text).await;
}

/// A file operand or stdin (`-`, or no operand).
pub enum Input {
    Stdin,
    File(UtilFile),
}

impl Input {
    /// Opens an operand for reading: `-` is stdin; directories fail with EISDIR.
    pub async fn open(ctx: &UtilCtx<'_>, name: &[u8]) -> io::Result<Input> {
        if name == b"-" {
            return Ok(Input::Stdin);
        }
        // The open file says what it is, where the host can tell: a second lookup of the
        // path (`stat`) only where it cannot, or where a directory does not open.
        let is_dir = async || {
            ctx.stat(name, true)
                .await
                .is_ok_and(|m| m.kind == FileKind::Dir)
        };
        let directory = || io::Error::new(io::ErrorKind::IsADirectory, "Is a directory");
        let file = match ctx.open(name, OpenOptions::read()).await {
            Ok(file) => file,
            Err(_) if is_dir().await => return Err(directory()),
            Err(e) => return Err(e),
        };
        match file.kind() {
            Some(FileKind::Dir) => Err(directory()),
            Some(_) => Ok(Input::File(file)),
            None if is_dir().await => Err(directory()),
            None => Ok(Input::File(file)),
        }
    }

    pub async fn read(&self, ctx: &mut UtilCtx<'_>, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Input::Stdin => ctx.read(buf).await,
            Input::File(f) => f.read(buf).await,
        }
    }

    pub async fn read_to_end(&self, ctx: &mut UtilCtx<'_>, out: &mut Vec<u8>) -> io::Result<()> {
        match self {
            Input::Stdin => ctx.read_to_end(out).await,
            Input::File(f) => f.read_to_end(out).await,
        }
    }
}

/// The operands, or `-` when there are none.
pub fn operands_or_stdin(p: &Parsed) -> Vec<Vec<u8>> {
    if p.operands.is_empty() {
        vec![b"-".to_vec()]
    } else {
        p.operands.clone()
    }
}

/// A GNU duration: a number (fractions allowed) with an optional `s m h d` suffix, or
/// `inf`/`infinity`. In seconds.
pub fn parse_duration(s: &[u8]) -> Option<f64> {
    let s = std::str::from_utf8(s).ok()?.trim();
    let (num, mult) = match s.as_bytes().last()? {
        b's' => (&s[..s.len() - 1], 1.0),
        b'm' => (&s[..s.len() - 1], 60.0),
        b'h' => (&s[..s.len() - 1], 3600.0),
        b'd' => (&s[..s.len() - 1], 86400.0),
        _ => (s, 1.0),
    };
    if num.eq_ignore_ascii_case("inf") || num.eq_ignore_ascii_case("infinity") {
        return Some(f64::INFINITY);
    }
    if num.is_empty() || num.starts_with(['+', '-']) && num.len() == 1 {
        return None;
    }
    // Digits, at most one dot; a hex float or exponent is rejected (GNU accepts them,
    // agents never write them).
    if !num
        .bytes()
        .all(|b| b.is_ascii_digit() || b == b'.' || b == b'+')
        || num.bytes().filter(|&b| b == b'.').count() > 1
        || num == "."
    {
        return None;
    }
    let v: f64 = num.parse().ok()?;
    (v >= 0.0).then_some(v * mult)
}

/// A GNU count with an optional multiplier suffix: `b` (512), `kB` `KB` (1000), `K` `k`
/// `KiB` (1024), and so on for M G T P E. `None` for a malformed count; saturates.
pub fn parse_count(s: &[u8]) -> Option<u64> {
    let s = std::str::from_utf8(s).ok()?;
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let n: u64 = s[..digits].parse().unwrap_or(u64::MAX);
    let suffix = &s[digits..];
    let mult: u64 = match suffix {
        "" => 1,
        "b" => 512,
        _ => {
            let (letter, rest) = suffix.split_at(1);
            let power = match letter {
                "k" | "K" => 1,
                "M" => 2,
                "G" => 3,
                "T" => 4,
                "P" => 5,
                "E" => 6,
                _ => return None,
            };
            let base: u64 = match rest {
                "" | "iB" => 1024,
                "B" => 1000,
                _ => return None,
            };
            base.checked_pow(power).unwrap_or(u64::MAX)
        }
    };
    Some(n.saturating_mul(mult))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration(b"1.5"), Some(1.5));
        assert_eq!(parse_duration(b"2m"), Some(120.0));
        assert_eq!(parse_duration(b".5s"), Some(0.5));
        assert_eq!(parse_duration(b"1d"), Some(86400.0));
        assert_eq!(parse_duration(b"inf"), Some(f64::INFINITY));
        assert_eq!(parse_duration(b"x"), None);
        assert_eq!(parse_duration(b"-1"), None);
        assert_eq!(parse_duration(b"1x"), None);
    }

    #[test]
    fn quotes() {
        assert_eq!(quote_in(true, b"x"), "\u{2018}x\u{2019}");
        assert_eq!(quote_in(false, b"x"), "'x'");
    }

    #[test]
    fn counts() {
        assert_eq!(parse_count(b"10"), Some(10));
        assert_eq!(parse_count(b"2b"), Some(1024));
        assert_eq!(parse_count(b"1K"), Some(1024));
        assert_eq!(parse_count(b"1kB"), Some(1000));
        assert_eq!(parse_count(b"1MiB"), Some(1 << 20));
        assert_eq!(parse_count(b"3x"), None);
        assert_eq!(parse_count(b""), None);
    }
}
