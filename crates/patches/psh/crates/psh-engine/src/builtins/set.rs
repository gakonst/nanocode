//! `set`: options and positional parameters (D1280).

use crate::options::{self, SET_OPTS, SetOpt};
use crate::state::{R, Vp};

use super::{out, s};

const USAGE: &str = "set: usage: set [-abefhkmnptuvxBCEHPT] [-o option-name] [--] [-] [arg ...]";

/// One step of `set`'s options, applied in order once all of them are known to be valid.
enum Step {
    Opt(SetOpt, bool),
    /// `-o`/`+o` without a name: the listing, as the options stand at that point.
    List(bool),
    /// `-`: turns off -x and -v.
    Dash,
}

pub(crate) async fn set(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    if args.len() == 1 {
        return list_vars(vp).await;
    }
    // First read every option word: an invalid one fails before any is applied.
    let mut steps = Vec::new();
    let mut i = 1;
    let mut positional = false;
    while i < args.len() {
        let a = &args[i];
        if a == b"--" {
            i += 1;
            positional = true;
            break;
        }
        if a == b"-" {
            steps.push(Step::Dash);
            i += 1;
            positional = i < args.len();
            break;
        }
        // A lone `+` is skipped.
        if a == b"+" {
            i += 1;
            continue;
        }
        let on = match a.first() {
            Some(b'-') => true,
            Some(b'+') => false,
            _ => break,
        };
        let mut ended = false;
        for &c in &a[1..] {
            if c == b'o' {
                // The name, unless the next word is empty, a lone `-` or `+`, or starts
                // with `+` or `-` (then the options are listed).
                match args.get(i + 1) {
                    Some(name)
                        if !name.is_empty()
                            && !name.starts_with(b"-")
                            && !name.starts_with(b"+") =>
                    {
                        i += 1;
                        match options::by_name(name) {
                            Some(opt) => steps.push(Step::Opt(opt, on)),
                            None => {
                                // An unknown name stops here; what came before applies.
                                apply(vp, steps).await?;
                                vp.error(&format!("set: {}: invalid option name", s(name)))
                                    .await;
                                return Ok(2);
                            }
                        }
                    }
                    Some(name) => {
                        steps.push(Step::List(on));
                        // An empty word is the first operand.
                        if name.is_empty() {
                            ended = true;
                        }
                    }
                    None => steps.push(Step::List(on)),
                }
                continue;
            }
            match options::by_letter(c) {
                Some(opt) => steps.push(Step::Opt(opt, on)),
                // `+r` (restricted mode off) is accepted; restricted mode itself is not
                // something psh has.
                None if c == b'r' && !on => {}
                None if c == b'r' => {
                    return crate::expand::unsupported(vp, "set -r (restricted mode)").await;
                }
                None => {
                    vp.error(&format!(
                        "set: {}{}: invalid option",
                        if on { '-' } else { '+' },
                        c as char
                    ))
                    .await;
                    vp.error_raw(format!("{USAGE}\n").as_bytes()).await;
                    return Ok(2);
                }
            }
        }
        i += 1;
        if ended {
            positional = true;
            break;
        }
    }
    let st = apply(vp, steps).await?;
    if st != 0 {
        return Ok(st);
    }
    if positional || i < args.len() {
        vp.args = args[i..].to_vec();
    }
    Ok(0)
}

async fn apply(vp: &mut Vp, steps: Vec<Step>) -> R {
    for step in steps {
        match step {
            Step::Opt(opt, on) => set_opt(vp, opt, on),
            Step::Dash => {
                set_opt(vp, SetOpt::Xtrace, false);
                set_opt(vp, SetOpt::Verbose, false);
            }
            Step::List(on) => {
                let st = list_options(vp, on).await?;
                if st != 0 {
                    return Ok(st);
                }
            }
        }
    }
    Ok(0)
}

/// Turns an option on or off and keeps `SHELLOPTS` in step.
pub(crate) fn set_opt(vp: &mut Vp, opt: SetOpt, on: bool) {
    vp.opts.set(opt, on);
    options::sync(vp);
}

/// `set -o` (`on`) or `set +o`.
pub(crate) async fn list_options(vp: &Vp, on: bool) -> R {
    let mut buf = Vec::new();
    for (opt, name, ..) in SET_OPTS {
        let state = vp.opts.get(*opt);
        if on {
            buf.extend_from_slice(
                format!("{name:<15}\t{}\n", if state { "on" } else { "off" }).as_bytes(),
            );
        } else {
            buf.extend_from_slice(
                format!("set {}o {name}\n", if state { '-' } else { '+' }).as_bytes(),
            );
        }
    }
    out(vp, "set", &buf).await
}

/// `set` without arguments: every variable with a value, sorted, quoted for re-input.
async fn list_vars(vp: &Vp) -> R {
    let mut vars: Vec<_> = vp.vars.iter().filter(|(_, v)| v.get().is_some()).collect();
    vars.sort_by(|a, b| a.0.cmp(b.0));
    let mut buf = Vec::new();
    for (n, v) in vars {
        buf.extend_from_slice(n.as_bytes());
        buf.push(b'=');
        // bash's `set` quoting ('…' or $'…'), shared with `declare` (0026).
        buf.extend_from_slice(&super::declare::set_quote(
            v.get().unwrap_or_default(),
            vp.utf8(),
        ));
        buf.push(b'\n');
    }
    out(vp, "set", &buf).await
}

/// Applies the options of the command line (`psh -e -o pipefail -O extglob -c …`),
/// already checked by `invocation::parse_args`.
pub(crate) fn apply_invocation_options(vp: &mut Vp, words: &[Vec<u8>]) {
    if words.is_empty() {
        return;
    }
    let mut i = 0;
    while i < words.len() {
        let w = &words[i];
        let on = w.first() == Some(&b'-');
        for &c in &w[1..] {
            match c {
                b'o' | b'O' => {
                    i += 1;
                    let Some(name) = words.get(i) else { continue };
                    if c == b'o' {
                        if let Some(opt) = options::by_name(name) {
                            vp.opts.set(opt, on);
                        }
                    } else if let Some(idx) = std::str::from_utf8(name)
                        .ok()
                        .and_then(options::shopt_index)
                    {
                        vp.opts.set_shopt_at(idx, on);
                    }
                }
                b's' => vp.opts.stdin = true,
                c => {
                    if let Some(opt) = options::by_letter(c) {
                        vp.opts.set(opt, on);
                    }
                }
            }
        }
        i += 1;
    }
    options::sync(vp);
}

/// `set -x` (D1285): `PS4` (expanded, its first char repeated per level of command
/// substitution and `eval`) and the line, on `BASH_XTRACEFD` or stderr.
pub(crate) fn xtrace<'a>(vp: &'a mut Vp, line: &'a [u8]) -> crate::sched::LocalBoxFuture<'a, ()> {
    // Boxed: callers sit on every command's path, and this future is large (D1059).
    Box::pin(xtrace_inner(vp, line))
}

async fn xtrace_inner(vp: &mut Vp, line: &[u8]) {
    let mut buf = ps4(vp).await;
    buf.extend_from_slice(line);
    buf.push(b'\n');
    let fd = vp
        .var("BASH_XTRACEFD")
        .and_then(|v| std::str::from_utf8(v).ok())
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|&n| vp.fd(n).is_some())
        .unwrap_or(2);
    if let Some(f) = vp.fd(fd) {
        let _ = f.write_all(&buf).await;
    }
}

/// The expanded `PS4` prefix; tracing is off while it expands.
async fn ps4(vp: &mut Vp) -> Vec<u8> {
    let raw = vp
        .var("PS4")
        .map(<[u8]>::to_vec)
        .unwrap_or_else(|| b"+ ".to_vec());
    let expanded = if raw.contains(&b'$') || raw.contains(&b'`') {
        // Expanded as the inside of double quotes.
        let mut src = b": \"".to_vec();
        for &b in &raw {
            if b == b'"' {
                src.push(b'\\');
            }
            src.push(b);
        }
        src.push(b'"');
        match psh_syntax::parse_bytes(&src, psh_syntax::Options::default()) {
            Ok(ast) => {
                let word = match &ast
                    .cmd(ast.list(ast.root()).items[0].first.stages[0].cmd)
                    .kind
                {
                    psh_syntax::CommandKind::Simple(sc) => sc.words.get(1).cloned(),
                    _ => None,
                };
                let code = std::rc::Rc::new(crate::state::Code {
                    lines: std::rc::Rc::new(psh_syntax::LineIndex::from_bytes(&src)),
                    ast: std::sync::Arc::new(ast),
                    line_base: 0,
                    arith: Default::default(),
                });
                let line = vp.line;
                vp.opts.set(SetOpt::Xtrace, false);
                let r = match word {
                    Some(w) => {
                        crate::expand::expand_single(vp, &code, &w, crate::expand::Ctx::Word)
                            .await
                            .map(|(v, _)| v)
                            .ok()
                    }
                    None => None,
                };
                vp.opts.set(SetOpt::Xtrace, true);
                vp.line = line;
                r.unwrap_or(raw)
            }
            Err(_) => raw,
        }
    } else {
        raw
    };
    let mut out = Vec::new();
    if let Some(&first) = expanded.first() {
        for _ in 1..vp.xtrace_level {
            out.push(first);
        }
    }
    out.extend_from_slice(&expanded);
    out
}

/// A word as `set -x` shows it: as is when it needs no quotes, else in single quotes,
/// or `$'…'` when it has control characters or bytes that are not UTF-8.
pub(crate) fn xtrace_word(w: &[u8]) -> Vec<u8> {
    if w.is_empty() {
        return b"''".to_vec();
    }
    let valid = std::str::from_utf8(w).is_ok();
    if !valid || w.iter().any(|&b| b < 0x20 || b == 0x7f) {
        return ansi_c_quote(w, true);
    }
    let needs = w.iter().enumerate().any(|(i, &b)| {
        b" \t\n'\"\\|&;()<>!{}*[]?^$`".contains(&b)
            || (b == b'#' && i == 0)
            || (b == b'~' && (i == 0 || matches!(w[i - 1], b'=' | b':')))
    });
    if !needs {
        return w.to_vec();
    }
    let mut out = vec![b'\''];
    for &b in w {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    out
}

pub(crate) fn needs_ansi(v: &[u8], utf8: bool) -> bool {
    v.iter().any(u8::is_ascii_control)
        || match utf8 {
            true => std::str::from_utf8(v).is_err(),
            false => v.iter().any(|&b| b >= 0x80),
        }
}

/// `$'…'` quoting: C escapes, octal for other control chars and invalid bytes.
pub(crate) fn ansi_c_quote(w: &[u8], utf8: bool) -> Vec<u8> {
    let mut out = b"$'".to_vec();
    let mut i = 0;
    while i < w.len() {
        let b = w[i];
        // A valid multibyte UTF-8 sequence stays as it is.
        if utf8 && b >= 0x80 {
            let len = match b {
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => 0,
            };
            if len > 0 && i + len <= w.len() && std::str::from_utf8(&w[i..i + len]).is_ok() {
                out.extend_from_slice(&w[i..i + len]);
                i += len;
                continue;
            }
        }
        match b {
            0x07 => out.extend_from_slice(b"\\a"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x1b => out.extend_from_slice(b"\\E"),
            0x0c => out.extend_from_slice(b"\\f"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0x0b => out.extend_from_slice(b"\\v"),
            b'\'' => out.extend_from_slice(b"\\'"),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b if !(0x20..0x7f).contains(&b) => {
                out.extend_from_slice(format!("\\{b:03o}").as_bytes())
            }
            b => out.push(b),
        }
        i += 1;
    }
    out.push(b'\'');
    out
}

/// An assignment as `set -x` shows it: `name=value`, an empty value without quotes.
pub(crate) fn xtrace_assign(name: &[u8], op: &[u8], value: &[u8]) -> Vec<u8> {
    let mut line = name.to_vec();
    line.extend_from_slice(op);
    if !value.is_empty() {
        line.extend_from_slice(&xtrace_word(value));
    }
    line
}

/// The line `set -x` shows for words: each one quoted as needed, joined by spaces.
pub(crate) fn xtrace_line(words: &[Vec<u8>]) -> Vec<u8> {
    let mut line = Vec::new();
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            line.push(b' ');
        }
        line.extend_from_slice(&xtrace_word(w));
    }
    line
}

#[cfg(test)]
mod tests {
    use crate::run_on_test_host as run;

    fn out(src: &str) -> (i32, String) {
        let (st, out, _) = run(src);
        (st, String::from_utf8_lossy(&out).into_owned())
    }

    #[test]
    fn errexit_exemptions() {
        let cases: &[(&str, i32, &str)] = &[
            ("set -e; f() { false && true; }; f; echo after", 1, ""),
            ("set -e; { false && true; }; echo after", 0, "after\n"),
            ("set -e; (false && true); echo after", 1, ""),
            ("set -e; x=$(false); echo after", 1, ""),
            (
                "set -e; f() { false; echo in; }; f || echo failed; echo after",
                0,
                "in\nafter\n",
            ),
            (
                "set -e; if false; then :; fi; while false; do :; done; echo ok",
                0,
                "ok\n",
            ),
            ("set -e; ! true; echo a; false || false; echo b", 1, "a\n"),
            ("set -e; true | { false; echo x; }; echo after", 1, ""),
            ("set -e; { false; echo x; } | cat; echo after", 0, "after\n"),
            ("set -e; false | true; echo after", 0, "after\n"),
            ("set -eo pipefail; false | true; echo after", 1, ""),
            ("set -e; x=$(false; echo sub); echo $x", 0, "sub\n"),
            (
                "shopt -s inherit_errexit; set -e; x=$(false; echo sub); echo $x",
                1,
                "",
            ),
            ("set -e; ((0)); echo after", 1, ""),
            ("set -e; f() { return 3; }; f; echo after", 3, ""),
        ];
        for (src, st, want) in cases {
            assert_eq!(out(src), (*st, want.to_string()), "{src}");
        }
    }

    #[test]
    fn xtrace_quoting_and_lines() {
        use super::xtrace_word as q;
        assert_eq!(q(b"a b"), b"'a b'");
        assert_eq!(q(b"it's"), b"'it'\\''s'");
        assert_eq!(q(b""), b"''");
        assert_eq!(q(b"x=1"), b"x=1");
        assert_eq!(q(b"a,b:c%d@e+f"), b"a,b:c%d@e+f");
        assert_eq!(q(b"#c"), b"'#c'");
        assert_eq!(q(b"a#b"), b"a#b");
        assert_eq!(q(b"~x"), b"'~x'");
        assert_eq!(q(b"x~"), b"x~");
        assert_eq!(q(b"a=~b"), b"'a=~b'");
        assert_eq!(q(b"a\nb"), b"$'a\\nb'");
        assert_eq!(q(b"\x1b"), b"$'\\E'");
        assert_eq!(q(b"\xff"), b"$'\\377'");
        assert_eq!(q("é".as_bytes()), "é".as_bytes());
        let (_, _, err) = run("set -x; x=; y='a b'; echo \"$y\" $(echo s); f() { :; }; f");
        assert_eq!(
            String::from_utf8_lossy(&err),
            "+ x=\n+ y='a b'\n++ echo s\n+ echo 'a b' s\n+ f\n+ :\n"
        );
        let (_, _, err) = run("PS4='[$LINENO] '; set -x; for i in 1 2; do :; done; ((1 + 1))");
        assert_eq!(
            String::from_utf8_lossy(&err),
            "[1] for i in 1 2\n[1] :\n[1] for i in 1 2\n[1] :\n[1] (( 1 + 1 ))\n"
        );
    }

    #[test]
    fn options_and_listing() {
        assert_eq!(out("set -euo pipefail; echo $-"), (0, "ehuBc\n".into()));
        assert_eq!(
            out("set -o errexit nounset; echo $1 $-"),
            (0, "nounset ehBc\n".into())
        );
        assert_eq!(out("set -x - a b; echo $# $-"), (0, "2 hBc\n".into()));
        assert_eq!(out("set -o bogus; echo $?"), (0, "2\n".into()));
        let (st, o) = out("set +o; shopt -p extglob; shopt -q extglob; echo $?");
        assert_eq!(st, 0);
        assert!(
            o.starts_with("set +o allexport\nset -o braceexpand\n"),
            "{o}"
        );
        assert!(o.ends_with("set +o xtrace\nshopt -u extglob\n1\n"), "{o}");
        assert_eq!(out("SHELLOPTS=x; echo no").0, 1);
        assert_eq!(
            out("set -u; echo ${x-d}; echo $x; echo no"),
            (127, "d\n".into())
        );
        assert_eq!(out("set -eu; echo $x"), (1, String::new()));
        // Only the shell itself ends with 127 under -c; a subshell with 1.
        assert_eq!(out("(set -u; echo $x); echo $?"), (0, "1\n".into()));
        assert!(out("set -a; x=1; env").1.lines().any(|l| l == "x=1"));
        assert!(!out("x=1; env").1.lines().any(|l| l == "x=1"));
    }
}
