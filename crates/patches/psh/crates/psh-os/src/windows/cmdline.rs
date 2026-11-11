//! Command lines for `CreateProcessW`: the argv quoting of the Microsoft C runtime, and
//! the hardened quoting for batch files run through `cmd.exe` (BatBadBut,
//! CVE-2024-24576).
//!
//! Both follow the Rust standard library's `sys/args/windows.rs` (MIT OR Apache-2.0,
//! see NOTICE): the same rules, so a program sees the arguments std's `Command` would
//! give it, and a batch file sees what it can be given safely. Arguments that cannot
//! reach a batch file intact (`\r`, `\n`) are refused.

use std::io;

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.to_string())
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn no_nul(arg: &str) -> io::Result<()> {
    if arg.contains('\0') {
        return Err(invalid("argument contains a NUL character"));
    }
    Ok(())
}

/// Appends one argument with MSVCRT quoting: quoted when empty or containing a space
/// or tab; backslashes doubled before a quote and before the closing quote.
fn append_arg(cmd: &mut Vec<u16>, arg: &str) -> io::Result<()> {
    no_nul(arg)?;
    let quote = arg.is_empty() || arg.contains([' ', '\t']);
    if quote {
        cmd.push(u16::from(b'"'));
    }
    let mut backslashes = 0usize;
    for c in arg.encode_utf16() {
        if c == u16::from(b'\\') {
            backslashes += 1;
        } else {
            if c == u16::from(b'"') {
                // n backslashes before a quote become 2n + 1.
                cmd.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes + 1));
            }
            backslashes = 0;
        }
        cmd.push(c);
    }
    if quote {
        cmd.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
        cmd.push(u16::from(b'"'));
    }
    Ok(())
}

/// The command line of a program: every argument, `argv[0]` included, quoted.
pub(crate) fn command_line(argv: &[&str]) -> io::Result<Vec<u16>> {
    let mut cmd = Vec::new();
    for (i, a) in argv.iter().enumerate() {
        if i > 0 {
            cmd.push(u16::from(b' '));
        }
        append_arg(&mut cmd, a)?;
    }
    Ok(cmd)
}

/// One argument of a batch file. Anything but letters, digits and a few safe symbols is
/// quoted; inside quotes `"` is doubled, and `%` becomes `%%cd:~,%`, an empty substring
/// of `%cd%` that keeps cmd from expanding `%NAME%` in the argument.
fn append_bat_arg(cmd: &mut Vec<u16>, arg: &str) -> io::Result<()> {
    no_nul(arg)?;
    let mut quote = arg.is_empty() || arg.ends_with('\\');
    for c in arg.chars() {
        const UNQUOTED: &str = r"#$*+-./:?@\_";
        let ascii_needs = c.is_ascii() && !(c.is_ascii_alphanumeric() || UNQUOTED.contains(c));
        if ascii_needs || c.is_control() {
            quote = true;
        }
    }
    if quote {
        cmd.push(u16::from(b'"'));
    }
    let mut backslashes = 0usize;
    for c in arg.encode_utf16() {
        if c == u16::from(b'\\') {
            backslashes += 1;
        } else {
            if c == u16::from(b'"') {
                cmd.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
                cmd.push(u16::from(b'"'));
            } else if c == u16::from(b'%') || c == u16::from(b'\r') {
                cmd.extend(utf16("%%cd:~,"));
            }
            backslashes = 0;
        }
        cmd.push(c);
    }
    if quote {
        cmd.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
        cmd.push(u16::from(b'"'));
    }
    Ok(())
}

/// The command line that runs batch file `script` (a native path) with `args` through
/// `cmd.exe`, which is the application to start. `/e:ON` keeps command extensions on
/// (the `%` guard needs them), `/v:OFF` delayed expansion off, `/d` skips AutoRun.
pub(crate) fn batch_command_line(script: &str, args: &[&str]) -> io::Result<Vec<u16>> {
    if script.contains('"') || script.ends_with('\\') {
        return Err(invalid(
            "a batch file's path may not contain `\"` or end with `\\`",
        ));
    }
    let mut cmd = utf16("cmd.exe /e:ON /v:OFF /d /c \"\"");
    cmd.extend(utf16(script));
    cmd.push(u16::from(b'"'));
    for a in args {
        if a.contains(['\r', '\n']) {
            return Err(invalid(
                "batch file arguments cannot contain line breaks (cmd.exe would cut them)",
            ));
        }
        cmd.push(u16::from(b' '));
        append_bat_arg(&mut cmd, a)?;
    }
    cmd.push(u16::from(b'"'));
    Ok(cmd)
}

/// Whether `path` names a batch file (`.bat`, `.cmd`, any case).
pub(crate) fn is_batch(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".bat") || lower.ends_with(".cmd")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(w: Vec<u16>) -> String {
        String::from_utf16(&w).unwrap()
    }

    #[test]
    fn msvcrt_quoting() {
        let line = |a: &[&str]| s(command_line(a).unwrap());
        assert_eq!(line(&["prog", "a", "b c", ""]), r#"prog a "b c" """#);
        assert_eq!(line(&["p", r#"a"b"#]), r#"p a\"b"#);
        assert_eq!(line(&["p", r"a\b"]), r"p a\b");
        assert_eq!(line(&["p", r#"a\"b"#]), r#"p a\\\"b"#);
        assert_eq!(line(&["p", r"a b\"]), r#"p "a b\\""#);
        assert_eq!(
            line(&[r"C:\Program Files\x.exe"]),
            r#""C:\Program Files\x.exe""#
        );
        assert!(command_line(&["p", "a\0b"]).is_err());
    }

    #[test]
    fn batch_quoting() {
        let line = |a: &[&str]| s(batch_command_line(r"C:\x\npm.cmd", a).unwrap());
        assert_eq!(
            line(&["install", "a b"]),
            r#"cmd.exe /e:ON /v:OFF /d /c ""C:\x\npm.cmd" install "a b"""#
        );
        assert_eq!(
            line(&["a&b"]),
            r#"cmd.exe /e:ON /v:OFF /d /c ""C:\x\npm.cmd" "a&b"""#
        );
        assert_eq!(
            line(&["%PATH%"]),
            r#"cmd.exe /e:ON /v:OFF /d /c ""C:\x\npm.cmd" "%%cd:~,%PATH%%cd:~,%"""#
        );
        assert_eq!(
            line(&[r#"say "hi""#]),
            r#"cmd.exe /e:ON /v:OFF /d /c ""C:\x\npm.cmd" "say ""hi"""""#
        );
        assert_eq!(
            line(&[""]),
            r#"cmd.exe /e:ON /v:OFF /d /c ""C:\x\npm.cmd" """"#
        );
        assert!(batch_command_line(r"C:\x\a.cmd", &["a\nb"]).is_err());
        assert!(batch_command_line(r#"C:\x"\a.cmd"#, &[]).is_err());
        assert!(is_batch(r"C:\x\NPM.CMD") && is_batch("a.bat") && !is_batch("a.exe"));
    }
}
