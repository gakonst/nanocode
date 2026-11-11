//! `env [-i] [-u NAME]... [-C DIR] [-S STRING] [NAME=VALUE]... [COMMAND [ARG]...]`:
//! prints the environment, or runs a command with a changed one through the shell's
//! dispatch (D1083: standalone builtins, utilities, programs; not functions). `-0` ends
//! printed entries with NUL; `-C` runs the command in DIR; `-S` splits its string into
//! arguments as GNU's manual describes (quotes, `\_` and the other escapes, `${VAR}`,
//! `#` comments), for `#!/usr/bin/env -S` lines. The signal options and `-v` fall
//! through to the real program.
//!
//! `printenv [-0] [NAME]...`: the values of the named variables (status 1 when one is
//! not set), or the whole environment.

use std::io;

use psh_engine::host::{FileKind, error_text};
use psh_engine::util::{SpawnEnv, UtilCtx};

use crate::common;
use crate::opts::{Spec, both};

pub const SPEC: Spec = Spec {
    name: "env",
    opts: &[
        both('i', "ignore-environment"),
        both('u', "unset").value(),
        both('0', "null"),
        both('C', "chdir").value(),
        both('S', "split-string").value(),
    ],
    posix_order: true,
};

pub const PRINTENV_SPEC: Spec = Spec {
    name: "printenv",
    opts: &[both('0', "null")],
    posix_order: false,
};

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    // A lone `-` is `-i` (POSIX).
    let mut args = match expand_split(ctx, argv) {
        Ok(a) => a,
        Err(msg) => {
            ctx.error(&msg).await;
            return 125;
        }
    };
    let mut dash_i = false;
    if args.get(1).is_some_and(|a| a == b"-") {
        args.remove(1);
        dash_i = true;
    }
    let p = match SPEC.parse(&args[1..]) {
        Ok(p) => p,
        Err(e) => {
            let st = common::option_error(ctx, &SPEC, argv, &e).await;
            // GNU env's own errors are 125.
            return if st == 1 { 125 } else { st };
        }
    };
    let mut env = SpawnEnv {
        clear: dash_i,
        ..SpawnEnv::default()
    };
    let mut nul = false;
    let mut chdir: Option<Vec<u8>> = None;
    for (key, value) in p.iter(&SPEC) {
        match key.as_str() {
            "chdir" => chdir = value.map(<[u8]>::to_vec),
            "ignore-environment" => env.clear = true,
            "unset" => env
                .unset
                .push(String::from_utf8_lossy(value.unwrap_or_default()).into_owned()),
            "null" => nul = true,
            _ => {}
        }
    }
    let mut rest = p.operands.as_slice();
    while let Some(a) = rest.first() {
        let Some(eq) = a.iter().position(|&b| b == b'=') else {
            break;
        };
        if eq == 0 {
            break;
        }
        env.set.push((
            String::from_utf8_lossy(&a[..eq]).into_owned(),
            a[eq + 1..].to_vec(),
        ));
        rest = &rest[1..];
    }
    if let Some(dir) = chdir {
        if rest.is_empty() {
            ctx.error("must specify command with --chdir (-C)").await;
            ctx.error_raw(b"Try 'env --help' for more information.\n")
                .await;
            return 125;
        }
        let target = ctx.path(&dir);
        let checked = match ctx.stat(&target, true).await {
            Ok(m) if m.kind == FileKind::Dir => ctx.canonicalize(&target).await,
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "Not a directory",
            )),
            Err(e) => Err(e),
        };
        match checked {
            Ok(abs) => env.cwd = Some(abs),
            Err(e) => {
                let d = String::from_utf8_lossy(&dir);
                ctx.error(&format!(
                    "cannot change directory to '{d}': {}",
                    error_text(&e)
                ))
                .await;
                return 125;
            }
        }
    }
    if rest.is_empty() {
        // Print the resulting environment.
        let mut vars: Vec<(Vec<u8>, Vec<u8>)> = if env.clear {
            Vec::new()
        } else {
            ctx.env()
                .iter()
                .filter(|(k, _)| !env.unset.iter().any(|u| u.as_bytes() == k.as_slice()))
                .cloned()
                .collect()
        };
        for (k, v) in &env.set {
            vars.retain(|(n, _)| n != k.as_bytes());
            vars.push((k.as_bytes().to_vec(), v.clone()));
        }
        let mut out = Vec::new();
        for (k, v) in vars {
            out.extend_from_slice(&k);
            out.push(b'=');
            out.extend_from_slice(&v);
            out.push(if nul { 0 } else { b'\n' });
        }
        if let Err(e) = ctx.write(&out).await {
            return common::write_failed(ctx, &e).await;
        }
        return 0;
    }
    let mut child = ctx.spawn(rest.to_vec(), env).await;
    child.wait().await
}

/// Replaces each `-S STRING` before the first operand with the words it splits into,
/// which may be options again (`env -S'-i A=1 cmd'`).
fn expand_split(ctx: &UtilCtx<'_>, argv: &[Vec<u8>]) -> Result<Vec<Vec<u8>>, String> {
    let mut out = vec![argv[0].clone()];
    let mut i = 1;
    while i < argv.len() {
        let a = &argv[i];
        i += 1;
        let value = if a == b"-S" || a == b"--split-string" {
            match argv.get(i) {
                Some(v) => {
                    i += 1;
                    v.clone()
                }
                // The parser reports the missing value.
                None => {
                    out.push(a.clone());
                    continue;
                }
            }
        } else if let Some(v) = a.strip_prefix(b"--split-string=") {
            v.to_vec()
        } else if a.starts_with(b"-S") {
            a[2..].to_vec()
        } else {
            out.push(a.clone());
            if a == b"--" || a.first() != Some(&b'-') || a == b"-" {
                out.extend_from_slice(&argv[i..]);
                break;
            }
            // An option that takes the next word as its value.
            if matches!(a.as_slice(), b"-u" | b"-C" | b"--unset" | b"--chdir")
                && let Some(v) = argv.get(i)
            {
                out.push(v.clone());
                i += 1;
            }
            continue;
        };
        out.extend(split_string(&value, |name| {
            ctx.var(name).map(<[u8]>::to_vec)
        })?);
    }
    Ok(out)
}

/// GNU `env -S`'s splitting: whitespace separates words; `'...'` is literal but for
/// `\\` and `\'`; `"..."` and bare text take the escapes `\c` (ends the string) `\f
/// \n \r \t \v \# \$ \_` (a space; outside quotes a separator-free one) `\" \'
/// \\`; `${NAME}` expands outside single quotes; `#` starting a word comments out
/// the rest.
pub fn split_string(
    s: &[u8],
    var: impl Fn(&[u8]) -> Option<Vec<u8>>,
) -> Result<Vec<Vec<u8>>, String> {
    let mut words = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    // A word is open (it may be empty: `''`).
    let mut open = false;
    let mut i = 0;
    #[derive(PartialEq)]
    enum Q {
        None,
        Single,
        Double,
    }
    let mut q = Q::None;
    while i < s.len() {
        let c = s[i];
        i += 1;
        match q {
            Q::Single => match c {
                b'\\' if matches!(s.get(i), Some(b'\\' | b'\'')) => {
                    cur.push(s[i]);
                    i += 1;
                }
                b'\'' => q = Q::None,
                _ => cur.push(c),
            },
            _ => match c {
                b' ' | b'\t' | b'\n' | b'\r' | b'\x0b' | b'\x0c' if q == Q::None => {
                    if open {
                        words.push(std::mem::take(&mut cur));
                        open = false;
                    }
                }
                b'#' if q == Q::None && !open => break,
                b'\'' if q == Q::None => {
                    q = Q::Single;
                    open = true;
                }
                b'"' => {
                    q = if q == Q::Double { Q::None } else { Q::Double };
                    open = true;
                }
                b'\\' => {
                    let Some(&e) = s.get(i) else {
                        return Err("invalid backslash at end of string in -S".into());
                    };
                    i += 1;
                    let r = match e {
                        b'c' => {
                            if q == Q::Double {
                                return Err(
                                    "'\\c' must not appear in double-quoted -S string".into()
                                );
                            }
                            break;
                        }
                        b'f' => 0x0c,
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'v' => 0x0b,
                        b'_' => b' ',
                        b'#' | b'$' | b'"' | b'\'' | b'\\' => e,
                        _ => {
                            return Err(format!("invalid sequence '\\{}' in -S", e as char));
                        }
                    };
                    cur.push(r);
                    open = true;
                }
                b'$' => {
                    let bad = || {
                        format!(
                            "only ${{VARNAME}} expansion is supported, error at: {}",
                            String::from_utf8_lossy(&s[i - 1..])
                        )
                    };
                    if s.get(i) != Some(&b'{') {
                        return Err(bad());
                    }
                    let Some(end) = s[i..].iter().position(|&b| b == b'}') else {
                        return Err(bad());
                    };
                    let name = &s[i + 1..i + end];
                    if let Some(v) = var(name) {
                        cur.extend_from_slice(&v);
                    }
                    i += end + 1;
                    open = true;
                }
                _ => {
                    cur.push(c);
                    open = true;
                }
            },
        }
    }
    if q != Q::None {
        return Err("no terminating quote in -S string".into());
    }
    if open {
        words.push(cur);
    }
    Ok(words)
}

/// `printenv`.
pub async fn run_printenv(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &PRINTENV_SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return if st == 1 { 2 } else { st },
    };
    let end = if p.has(&PRINTENV_SPEC, "null") {
        0
    } else {
        b'\n'
    };
    let mut out = Vec::new();
    let mut status = 0;
    if p.operands.is_empty() {
        for (k, v) in ctx.env() {
            out.extend_from_slice(k);
            out.push(b'=');
            out.extend_from_slice(v);
            out.push(end);
        }
    } else {
        for name in &p.operands {
            match ctx.env().iter().find(|(k, _)| k == name) {
                Some((_, v)) if !name.contains(&b'=') => {
                    out.extend_from_slice(v);
                    out.push(end);
                }
                _ => status = 1,
            }
        }
    }
    if let Err(e) = ctx.write(&out).await {
        return common::write_failed(ctx, &e).await;
    }
    status
}

#[cfg(test)]
mod tests {
    use super::split_string;

    fn split(s: &str) -> Result<Vec<String>, String> {
        split_string(s.as_bytes(), |n| (n == b"FOO").then(|| b"bar".to_vec())).map(|w| {
            w.into_iter()
                .map(|x| String::from_utf8(x).unwrap())
                .collect()
        })
    }

    #[test]
    fn split_strings() {
        assert_eq!(split("echo a  b").unwrap(), ["echo", "a", "b"]);
        assert_eq!(
            split("echo 'x y' \"z\\tw\"").unwrap(),
            ["echo", "x y", "z\tw"]
        );
        assert_eq!(split("printf %s ${FOO}").unwrap(), ["printf", "%s", "bar"]);
        assert_eq!(split("echo a\\_b\\c ignored").unwrap(), ["echo", "a b"]);
        assert_eq!(split("echo a #comment").unwrap(), ["echo", "a"]);
        assert_eq!(split("echo a#b").unwrap(), ["echo", "a#b"]);
        assert_eq!(split("echo ''").unwrap(), ["echo", ""]);
        assert_eq!(split("'it\\'s'").unwrap(), ["it's"]);
        assert!(split("echo $FOO").is_err());
        assert!(split("echo 'open").is_err());
    }
}
