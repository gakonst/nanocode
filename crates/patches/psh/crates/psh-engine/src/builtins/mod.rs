//! Shell builtins (Tier 0, ARCH.md): part of the engine, run on the vproc that calls
//! them.

pub(crate) mod alias;
mod command;
#[cfg(test)]
mod core_tests;
pub(crate) mod declare;
mod dirs;
mod eval;
pub(crate) mod hash;
mod help;
mod jobs;
#[cfg(test)]
mod jobs_tests;
mod mapfile;
mod printf;
pub(crate) mod read;
pub(crate) mod set;
mod shopt;
pub(crate) mod strftime;
pub(crate) mod test;
pub(crate) mod trap;
mod ulimit;
pub(crate) mod umask;

pub(crate) use set::apply_invocation_options;

use crate::escape::{Dialect, decode};
use crate::host::{FileKind, error_text};
use crate::sched::LocalBoxFuture;
use crate::state::{R, Unwind, Vp, is_name};

pub(crate) type BuiltinFn = for<'a> fn(&'a mut Vp, &'a [Vec<u8>]) -> LocalBoxFuture<'a, R>;

#[derive(Clone, Copy)]
pub(crate) struct Builtin {
    pub run: BuiltinFn,
    /// A POSIX special builtin. Outside POSIX mode bash treats them like the others;
    /// `set -o posix` will (assignments in front of them stay, errors end the shell).
    #[allow(dead_code)]
    pub special: bool,
}

macro_rules! builtin {
    ($f:path) => {{
        fn wrap<'a>(vp: &'a mut Vp, args: &'a [Vec<u8>]) -> LocalBoxFuture<'a, R> {
            Box::pin($f(vp, args))
        }
        wrap as BuiltinFn
    }};
}

pub(crate) fn find(name: &[u8]) -> Option<Builtin> {
    let (run, special) = match name {
        b":" => (builtin!(colon), true),
        b"true" => (builtin!(colon), false),
        b"false" => (builtin!(false_), false),
        b"echo" => (builtin!(echo), false),
        b"printf" => (builtin!(printf::printf), false),
        b"cd" => (builtin!(cd), false),
        b"pwd" => (builtin!(pwd), false),
        b"export" => (builtin!(declare_words), true),
        b"readonly" => (builtin!(declare_words), true),
        b"declare" | b"typeset" => (builtin!(declare_words), false),
        b"unset" => (builtin!(unset), true),
        b"alias" => (builtin!(alias::alias), false),
        b"unalias" => (builtin!(alias::unalias), false),
        b"exit" => (builtin!(exit), true),
        b"set" => (builtin!(set::set), true),
        b"shopt" => (builtin!(shopt::shopt), false),
        b"trap" => (builtin!(trap::trap), true),
        b"shift" => (builtin!(shift), true),
        b"return" => (builtin!(return_), true),
        b"break" => (builtin!(break_), true),
        b"continue" => (builtin!(continue_), true),
        b"local" => (builtin!(declare_words), false),
        b"let" => (builtin!(let_), false),
        b"getopts" => (builtin!(getopts), false),
        b"mapfile" | b"readarray" => (builtin!(mapfile::mapfile), false),
        b"test" => (builtin!(test::test), false),
        b"[" => (builtin!(test::bracket), false),
        b"read" => (builtin!(read::read), false),
        b"eval" => (builtin!(eval::eval), true),
        b"source" | b"." => (builtin!(eval::source), true),
        b"command" => (builtin!(command::command), false),
        b"type" => (builtin!(command::type_), false),
        b"builtin" => (builtin!(command::builtin), false),
        b"exec" => (builtin!(command::exec_), true),
        b"enable" => (builtin!(command::enable), false),
        b"times" => (builtin!(command::times), true),
        b"hash" => (builtin!(hash::hash), false),
        b"wait" => (builtin!(jobs::wait), false),
        b"kill" => (builtin!(jobs::kill), false),
        b"jobs" => (builtin!(jobs::jobs), false),
        b"disown" => (builtin!(jobs::disown), false),
        b"caller" => (builtin!(command::caller), false),
        b"help" => (builtin!(help::help), false),
        b"umask" => (builtin!(umask::umask), false),
        b"dirs" => (builtin!(dirs::dirs), false),
        b"ulimit" => (builtin!(ulimit::ulimit), false),
        b"pushd" => (builtin!(dirs::pushd), false),
        b"popd" => (builtin!(dirs::popd), false),
        b"logout" => (builtin!(help::logout), false),
        b"history" => (builtin!(help::history), false),
        // Every other bash builtin: an error, never a program of the same name.
        _ if BASH_BUILTINS.contains(&name) => (builtin!(unsupported), SPECIAL.contains(&name)),
        _ => return None,
    };
    Some(Builtin { run, special })
}

/// The builtin `name` runs, unless `enable -n` turned it off.
pub(crate) fn get(vp: &Vp, name: &[u8]) -> Option<Builtin> {
    if vp.disabled.iter().any(|d| d == name) {
        return None;
    }
    find(name)
}

/// Whether `name` is an enabled builtin (`type`, `command -v`).
pub(crate) fn is_builtin(vp: &Vp, name: &[u8]) -> bool {
    (find(name).is_some() || BASH_BUILTINS.contains(&name))
        && !vp.disabled.iter().any(|d| d == name)
}

/// Bash 5.3's builtins (`enable -a`), in its order.
pub(crate) const BASH_BUILTINS: &[&[u8]] = &[
    b".",
    b":",
    b"[",
    b"alias",
    b"bg",
    b"bind",
    b"break",
    b"builtin",
    b"caller",
    b"cd",
    b"command",
    b"compgen",
    b"complete",
    b"compopt",
    b"continue",
    b"declare",
    b"dirs",
    b"disown",
    b"echo",
    b"enable",
    b"eval",
    b"exec",
    b"exit",
    b"export",
    b"false",
    b"fc",
    b"fg",
    b"getopts",
    b"hash",
    b"help",
    b"history",
    b"jobs",
    b"kill",
    b"let",
    b"local",
    b"logout",
    b"mapfile",
    b"popd",
    b"printf",
    b"pushd",
    b"pwd",
    b"read",
    b"readarray",
    b"readonly",
    b"return",
    b"set",
    b"shift",
    b"shopt",
    b"source",
    b"suspend",
    b"test",
    b"times",
    b"trap",
    b"true",
    b"type",
    b"typeset",
    b"ulimit",
    b"umask",
    b"unalias",
    b"unset",
    b"wait",
];

/// The POSIX special builtins (`enable -s`).
pub(crate) const SPECIAL: &[&[u8]] = &[
    b".",
    b":",
    b"break",
    b"continue",
    b"eval",
    b"exec",
    b"exit",
    b"export",
    b"readonly",
    b"return",
    b"set",
    b"shift",
    b"source",
    b"times",
    b"trap",
    b"unset",
];

/// A bash builtin psh does not have yet: fails instead of running a program of the
/// same name (macOS has `/usr/bin/read`, `/usr/bin/type`, ... as sh wrappers).
async fn unsupported(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    crate::expand::unsupported(vp, &s(&args[0])).await
}

/// Writes to a descriptor of the vproc. A closed pipe ends the vproc as SIGPIPE would
/// (status 141); other errors print `NAME: write error: …` and return false.
pub(crate) async fn write_fd(vp: &Vp, fd: u32, name: &str, data: &[u8]) -> Result<bool, Unwind> {
    let Some(f) = vp.fd(fd) else {
        vp.error(&format!("{name}: write error: Bad file descriptor"))
            .await;
        return Ok(false);
    };
    match f.write_all(data).await {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
            Err(Unwind::Exit(128 + crate::host::SIGPIPE))
        }
        Err(e) => {
            vp.error(&format!("{name}: write error: {}", error_text(&e)))
                .await;
            Ok(false)
        }
    }
}

pub(crate) async fn out(vp: &Vp, name: &str, data: &[u8]) -> R {
    Ok(if write_fd(vp, 1, name, data).await? {
        0
    } else {
        1
    })
}

fn s(b: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(b)
}

/// A declaration builtin called with plain words (the interpreter passes expanded
/// assignments to `declare::run` directly).
async fn declare_words(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let which = declare::Which::from_name(&args[0]).unwrap_or(declare::Which::Declare);
    let args = args[1..]
        .iter()
        .cloned()
        .map(declare::DeclArg::from_word)
        .collect();
    declare::run(vp, which, args).await
}

async fn colon(_vp: &mut Vp, _args: &[Vec<u8>]) -> R {
    Ok(0)
}

async fn false_(_vp: &mut Vp, _args: &[Vec<u8>]) -> R {
    Ok(1)
}

async fn echo(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let mut newline = true;
    // `shopt -s xpg_echo`: escapes by default (`-E` still turns them off).
    let mut escapes = vp.opts.shopt(crate::options::Shopt::XpgEcho);
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a.len() < 2 || a[0] != b'-' || !a[1..].iter().all(|c| b"neE".contains(c)) {
            break;
        }
        for c in &a[1..] {
            match c {
                b'n' => newline = false,
                b'e' => escapes = true,
                _ => escapes = false,
            }
        }
        i += 1;
    }
    let mut buf = Vec::new();
    for (k, a) in args[i..].iter().enumerate() {
        if k > 0 {
            buf.push(b' ');
        }
        if escapes {
            let d = decode(a, Dialect::Echo);
            buf.extend_from_slice(&d.bytes);
            if d.stop {
                return out(vp, "echo", &buf).await;
            }
        } else {
            buf.extend_from_slice(a);
        }
    }
    if newline {
        buf.push(b'\n');
    }
    out(vp, "echo", &buf).await
}

/// Normalizes `.` and `..` in an absolute path, lexically (`cd -L`).
pub(crate) fn normalize(path: &[u8]) -> Vec<u8> {
    // `C:/x` and `C:\x` (Windows) keep the drive in front, with `/` separators; `..`
    // stops at the drive (D1323).
    if path.len() >= 2 && path[0].is_ascii_alphabetic() && path[1] == b':' {
        let rest: Vec<u8> = path[2..]
            .iter()
            .map(|&b| if b == b'\\' { b'/' } else { b })
            .collect();
        let rest = normalize(&rest);
        let mut out = path[..2].to_vec();
        out.extend_from_slice(&rest);
        return out;
    }
    let mut parts: Vec<&[u8]> = Vec::new();
    for c in path.split(|&b| b == b'/') {
        match c {
            b"" | b"." => {}
            b".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    let mut out = Vec::new();
    for p in parts {
        out.push(b'/');
        out.extend_from_slice(p);
    }
    if out.is_empty() {
        out.push(b'/');
    }
    out
}

async fn cd(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let mut physical = false;
    let mut i = 1;
    while i < args.len() && args[i].len() > 1 && args[i][0] == b'-' {
        match &args[i][..] {
            b"--" => {
                i += 1;
                break;
            }
            b"-L" => physical = false,
            b"-P" => physical = true,
            b"-e" | b"-@" => {}
            _ => break,
        }
        i += 1;
    }
    let rest = &args[i..];
    if rest.len() > 1 {
        vp.error("cd: too many arguments").await;
        return Ok(1);
    }
    let mut print = false;
    let dir = match rest.first() {
        None => match vp.var("HOME") {
            Some(h) => h.to_vec(),
            None => {
                vp.error("cd: HOME not set").await;
                return Ok(1);
            }
        },
        Some(d) if d == b"-" => match vp.var("OLDPWD") {
            Some(o) => {
                print = true;
                o.to_vec()
            }
            None => {
                vp.error("cd: OLDPWD not set").await;
                return Ok(1);
            }
        },
        Some(d) => d.clone(),
    };
    if dir.is_empty() {
        return Ok(0);
    }
    let logical = vp.abs_path(&dir);
    // `cd missing/..` fails: each `..` leaves a directory that must be one. The working
    // directory and the ones above it are not asked: `cd ..` out of a directory that was
    // removed works, as it does in bash through the directory it still holds.
    let mut through = 0;
    while let Some(at) = logical[through..]
        .windows(3)
        .position(|w| w == b"/..")
        .map(|at| through + at)
    {
        through = at + 3;
        if logical.get(through).is_some_and(|&b| b != b'/') {
            continue;
        }
        let left = normalize(&logical[..at]);
        if vp
            .cwd
            .strip_prefix(&left[..])
            .is_some_and(|rest| rest.is_empty() || rest[0] == b'/' || left.ends_with(b"/"))
        {
            continue;
        }
        match vp.host().stat(&left, true).await {
            Ok(m) if m.kind == FileKind::Dir => {}
            Ok(_) => {
                vp.error(&format!("cd: {}: Not a directory", s(&dir))).await;
                return Ok(1);
            }
            Err(e) => {
                vp.error(&format!("cd: {}: {}", s(&dir), error_text(&e)))
                    .await;
                return Ok(1);
            }
        }
    }
    let mut target = normalize(&logical);
    match vp.host().stat(&target, true).await {
        Ok(m) if m.kind == FileKind::Dir => {}
        Ok(_) => {
            vp.error(&format!("cd: {}: Not a directory", s(&dir))).await;
            return Ok(1);
        }
        Err(e) => {
            vp.error(&format!("cd: {}: {}", s(&dir), error_text(&e)))
                .await;
            return Ok(1);
        }
    }
    if physical && let Ok(p) = vp.host().canonicalize(&target).await {
        target = p;
    }
    let old = vp.cwd.clone();
    vp.set_var("OLDPWD", old);
    vp.set_var("PWD", target.clone());
    vp.cwd = target;
    if print {
        let mut line = vp.cwd.clone();
        line.push(b'\n');
        return out(vp, "cd", &line).await;
    }
    Ok(0)
}

async fn pwd(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let physical = args[1..].iter().any(|a| a == b"-P");
    let mut dir = vp.cwd.clone();
    if physical && let Ok(p) = vp.host().canonicalize(&dir).await {
        dir = p;
    }
    dir.push(b'\n');
    out(vp, "pwd", &dir).await
}

async fn unset(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let mut i = 1;
    let (mut funcs, mut vars_only, mut nameref) = (false, false, false);
    while i < args.len() && args[i].first() == Some(&b'-') && args[i].len() > 1 {
        if args[i] == b"--" {
            i += 1;
            break;
        }
        if !args[i][1..].iter().all(|c| b"fvn".contains(c)) {
            vp.error(&format!("unset: {}: invalid option", s(&args[i][..2])))
                .await;
            return Ok(2);
        }
        funcs |= args[i].contains(&b'f');
        vars_only |= args[i].contains(&b'v');
        nameref |= args[i].contains(&b'n');
        i += 1;
    }
    let mut status = 0;
    for a in &args[i..] {
        let n = std::str::from_utf8(a).unwrap_or_default();
        // `-f`, or no option and no such variable: the function.
        if (funcs && !vars_only)
            || (!vars_only && vp.vars.get(n).is_none() && vp.funcs.contains_key(n))
        {
            std::rc::Rc::make_mut(&mut vp.funcs).remove(n);
            continue;
        }
        // `unset a[i]`
        if !funcs
            && let Some(open) = a.iter().position(|&b| b == b'[')
            && a.ends_with(b"]")
            && is_name(&a[..open])
        {
            let name = String::from_utf8_lossy(&a[..open]).into_owned();
            let sub = crate::expand::expand_sub_text(vp, &a[open + 1..a.len() - 1]).await?;
            if let Err(msg) = vp.unset_elem(&name, &sub) {
                vp.error(&format!("unset: {msg}")).await;
                status = 1;
            }
            continue;
        }
        if !is_name(a) {
            vp.error(&format!("unset: `{}': not a valid identifier", s(a)))
                .await;
            status = 1;
            continue;
        }
        // A nameref unsets its target, unless `-n` (which unsets only namerefs).
        let n = if nameref {
            if !vp.vars.get(n).is_some_and(|v| v.nameref) {
                continue;
            }
            n.to_string()
        } else {
            match vp.target(n) {
                Ok((t, Some(sub))) => {
                    let sub = crate::expand::expand_sub_text(vp, &sub).await?;
                    if let Err(msg) = vp.unset_elem(&t, &sub) {
                        vp.error(&format!("unset: {msg}")).await;
                        status = 1;
                    }
                    continue;
                }
                Ok((t, None)) => t,
                Err(_) => n.to_string(),
            }
        };
        if vp.vars.get(&n).is_some_and(|v| v.readonly) {
            vp.error(&format!("unset: {n}: cannot unset: readonly variable"))
                .await;
            status = 1;
            continue;
        }
        // The call stack but FUNCNAME stays (bash).
        if vp.vars.get(&n).is_some_and(|v| {
            use crate::vars::Dynamic::*;
            matches!(
                v.dynamic,
                Some(BashSource | BashLineno | BashArgv | BashArgc)
            )
        }) {
            vp.error(&format!("unset: {n}: cannot unset")).await;
            status = 1;
            continue;
        }
        // Unsetting an assignment in front of the running function brings back the
        // value under it; that variable is then the one later assignments change (bash).
        if let Some(frame) = vp
            .frames
            .iter_mut()
            .rev()
            .find(|f| f.saved.iter().any(|(s, _)| *s == n))
            && let Some(t) = frame.temps.iter().position(|t| *t == n)
        {
            frame.temps.remove(t);
            let i = frame.saved.iter().position(|(s, _)| *s == n).unwrap_or(0);
            let (_, old) = frame.saved.remove(i);
            match old {
                Some(v) => vp.vars.insert(&n, v),
                None => {
                    vp.vars.remove(&n);
                }
            }
            continue;
        }
        vp.vars.remove(&n);
    }
    Ok(status)
}

/// Parses a decimal status argument the way `exit`, `return` and `shift` do.
fn parse_int(a: &[u8]) -> Option<i64> {
    let t = std::str::from_utf8(a).ok()?.trim();
    t.parse::<i64>().ok()
}

async fn exit(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let mut rest = &args[1..];
    if rest.first().is_some_and(|a| a == b"--") {
        rest = &rest[1..];
    }
    match rest {
        // In a trap, a bare `exit` keeps the status the trap was entered with.
        [] => Err(Unwind::Exit(vp.in_trap.unwrap_or(vp.status))),
        [a] => match parse_int(a) {
            Some(n) => Err(Unwind::Exit((n & 0xff) as i32)),
            None => {
                vp.error(&format!("exit: {}: numeric argument required", s(a)))
                    .await;
                Err(Unwind::Exit(2))
            }
        },
        _ => {
            // The rest of the line is abandoned.
            vp.error("exit: too many arguments").await;
            Err(Unwind::Abort(2))
        }
    }
}

async fn return_(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    if vp.frames.is_empty() && !vp.calls.iter().any(|c| c.sourced) {
        vp.error("return: can only `return' from a function or sourced script")
            .await;
        return Ok(2);
    }
    let mut rest = &args[1..];
    if rest.first().is_some_and(|a| a == b"--") {
        rest = &rest[1..];
    }
    if rest.len() > 1 {
        vp.error("return: too many arguments").await;
        return Err(Unwind::Abort(1));
    }
    match rest.first() {
        None => Err(Unwind::Return(vp.status)),
        Some(a) => match parse_int(a) {
            Some(n) => Err(Unwind::Return((n & 0xff) as i32)),
            None => {
                vp.error(&format!("return: {}: numeric argument required", s(a)))
                    .await;
                Err(Unwind::Return(2))
            }
        },
    }
}

async fn break_(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    loop_control(vp, args, "break").await
}

async fn continue_(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    loop_control(vp, args, "continue").await
}

async fn loop_control(vp: &mut Vp, args: &[Vec<u8>], name: &str) -> R {
    let args = match args.get(1) {
        Some(a) if a == b"--" => &args[1..],
        _ => args,
    };
    if args.len() > 2 && vp.loops > 0 {
        vp.error(&format!("{name}: too many arguments")).await;
        return Err(Unwind::Abort(1));
    }
    let n = match args.get(1) {
        None => 1,
        Some(a) => match parse_int(a) {
            Some(n) if n >= 1 => n,
            Some(_) => {
                // bash leaves every enclosing loop, with status 1.
                vp.error(&format!("{name}: {}: loop count out of range", s(a)))
                    .await;
                if vp.loops == 0 {
                    return Ok(1);
                }
                vp.status = 1;
                return Err(Unwind::Break(vp.loops));
            }
            None => {
                // A usage error of a special builtin ends the shell.
                vp.error(&format!("{name}: {}: numeric argument required", s(a)))
                    .await;
                return Err(Unwind::Exit(2));
            }
        },
    };
    if vp.loops == 0 {
        vp.error(&format!(
            "{name}: only meaningful in a `for', `while', or `until' loop"
        ))
        .await;
        return Ok(0);
    }
    let n = n.min(i64::from(vp.loops)) as u32;
    Err(if name == "break" {
        Unwind::Break(n)
    } else {
        Unwind::Continue(n)
    })
}

async fn let_(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    // A leading `--` ends the options (there are none); `-x` is an expression.
    let args: &[Vec<u8>] = if args.get(1).is_some_and(|a| a == b"--") {
        &args[1..]
    } else {
        args
    };
    if args.len() < 2 {
        vp.error("let: expression expected").await;
        return Ok(1);
    }
    let mut last = 0;
    for a in &args[1..] {
        match crate::arith::eval(vp, a) {
            Ok(v) => last = v,
            Err(msg) => {
                vp.error(&format!("let: {msg}")).await;
                return Ok(1);
            }
        }
    }
    Ok(i32::from(last == 0))
}

async fn getopts(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let args = match args.get(1) {
        Some(a) if a == b"--" => {
            let mut v = args.to_vec();
            v.remove(1);
            v
        }
        Some(a) if a.len() > 1 && a[0] == b'-' => {
            vp.error(&format!("getopts: {}: invalid option", s(&a[..2])))
                .await;
            vp.error_raw(b"getopts: usage: getopts optstring name [arg ...]\n")
                .await;
            return Ok(2);
        }
        _ => args.to_vec(),
    };
    if args.len() < 3 {
        vp.error("getopts: usage: getopts optstring name [arg ...]")
            .await;
        return Ok(2);
    }
    let spec = args[1].clone();
    let Some(name) = std::str::from_utf8(&args[2])
        .ok()
        .filter(|n| is_name(n.as_bytes()))
        .map(str::to_string)
    else {
        vp.error(&format!(
            "getopts: `{}': not a valid identifier",
            s(&args[2])
        ))
        .await;
        return Ok(1);
    };
    let (silent, spec) = match spec.strip_prefix(b":") {
        Some(rest) => (true, rest.to_vec()),
        None => (false, spec),
    };
    let params: Vec<Vec<u8>> = if args.len() > 3 {
        args[3..].to_vec()
    } else {
        vp.args.clone()
    };
    let mut optind = vp.var("OPTIND").and_then(parse_int).unwrap_or(1).max(1);
    let mut pos = if vp.getopts.0 == optind {
        vp.getopts.1
    } else {
        0
    };
    let end = |vp: &mut Vp, optind: i64| {
        vp.set_var(&name, b"?".to_vec());
        vp.vars.remove("OPTARG");
        vp.set_var("OPTIND", optind.to_string().into_bytes());
        vp.getopts = (optind, 0);
        Ok(1)
    };
    let Some(arg) = params.get(optind as usize - 1).cloned() else {
        return end(vp, optind);
    };
    if pos >= arg.len() {
        pos = 0;
    }
    if pos == 0 {
        if arg == b"--" {
            return end(vp, optind + 1);
        }
        if arg.len() < 2 || arg[0] != b'-' {
            return end(vp, optind);
        }
        pos = 1;
    }
    let c = arg[pos];
    pos += 1;
    let at_end = pos >= arg.len();
    let found = spec.iter().position(|&x| x == c && x != b':');
    let mut value = vec![c];
    let mut optarg: Option<Vec<u8>> = None;
    let next = |optind: &mut i64, pos: &mut usize| {
        if *pos >= arg.len() {
            *optind += 1;
            *pos = 0;
        }
    };
    match found {
        Some(i) if spec.get(i + 1) == Some(&b':') => {
            if !at_end {
                optarg = Some(arg[pos..].to_vec());
                optind += 1;
                pos = 0;
            } else if let Some(v) = params.get(optind as usize) {
                optarg = Some(v.clone());
                optind += 2;
                pos = 0;
            } else {
                optind += 1;
                pos = 0;
                if silent {
                    value = b":".to_vec();
                    optarg = Some(vec![c]);
                } else {
                    let msg = format!(
                        "{}: option requires an argument -- {}\n",
                        String::from_utf8_lossy(&vp.arg0),
                        c as char
                    );
                    vp.error_raw(msg.as_bytes()).await;
                    value = b"?".to_vec();
                }
            }
        }
        Some(_) => next(&mut optind, &mut pos),
        None => {
            next(&mut optind, &mut pos);
            if silent {
                optarg = Some(vec![c]);
            } else {
                let msg = format!(
                    "{}: illegal option -- {}\n",
                    String::from_utf8_lossy(&vp.arg0),
                    c as char
                );
                vp.error_raw(msg.as_bytes()).await;
            }
            value = b"?".to_vec();
        }
    }
    vp.set_var(&name, value);
    match optarg {
        Some(v) => vp.set_var("OPTARG", v),
        None => {
            vp.vars.remove("OPTARG");
        }
    }
    vp.set_var("OPTIND", optind.to_string().into_bytes());
    vp.getopts = (optind, pos);
    Ok(0)
}

async fn shift(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let args = match args.get(1) {
        Some(a) if a == b"--" => &args[1..],
        _ => args,
    };
    if args.len() > 2 {
        vp.error("shift: too many arguments").await;
        return Err(Unwind::Abort(1));
    }
    let n = match args.get(1) {
        None => 1,
        Some(a) => match parse_int(a) {
            Some(n) if n >= 0 => n as usize,
            Some(_) => {
                vp.error(&format!("shift: {}: shift count out of range", s(a)))
                    .await;
                return Ok(1);
            }
            None => {
                vp.error(&format!("shift: {}: numeric argument required", s(a)))
                    .await;
                return Ok(2);
            }
        },
    };
    if n > vp.args.len() {
        return Ok(1);
    }
    vp.args.drain(..n);
    Ok(0)
}

/// Quoting for `set` output and `printf %q`: backslashes, or `$'…'` for control chars,
/// `''` for the empty string.
pub(crate) fn shell_quote(v: &[u8], utf8: bool) -> Vec<u8> {
    if v.is_empty() {
        return b"''".to_vec();
    }
    if set::needs_ansi(v, utf8) {
        return set::ansi_c_quote(v, utf8);
    }
    let mut out = Vec::with_capacity(v.len());
    for (i, &b) in v.iter().enumerate() {
        let special = matches!(
            b,
            b' ' | b'\''
                | b'"'
                | b'\\'
                | b'$'
                | b'`'
                | b'!'
                | b'&'
                | b';'
                | b'|'
                | b'<'
                | b'>'
                | b'('
                | b')'
                | b'*'
                | b'?'
                | b'['
                | b']'
                | b'{'
                | b'}'
                | b'^'
                | b','
        ) || (b == b'#' && i == 0)
            // A tilde that would expand: first, or after `:` or `=`.
            || (b == b'~' && (i == 0 || matches!(v[i - 1], b':' | b'=')))
            || (b == b'=' && i == 0);
        if special {
            out.push(b'\\');
        }
        out.push(b);
    }
    out
}
