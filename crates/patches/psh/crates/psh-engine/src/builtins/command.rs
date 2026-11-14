//! Builtins about commands: `command`, `type`, `builtin`, `exec`, `enable`, `help`,
//! `times`.

use std::rc::Rc;

use crate::exec::{self, ExtOpts};
use crate::host::{Access, FileKind};
use crate::state::{Func, R, Unwind, Vp};

use super::{BASH_BUILTINS, SPECIAL, hash, out, s};

/// Bash's reserved words (`type` says "shell keyword").
const KEYWORDS: &[&[u8]] = &[
    b"!",
    b"[[",
    b"]]",
    b"case",
    b"coproc",
    b"do",
    b"done",
    b"elif",
    b"else",
    b"esac",
    b"fi",
    b"for",
    b"function",
    b"if",
    b"in",
    b"select",
    b"then",
    b"time",
    b"until",
    b"while",
    b"{",
    b"}",
];

/// What a command name is, in the order the shell looks.
enum Kind {
    /// An alias (only while aliases expand), with its text.
    Alias(Vec<u8>),
    Keyword,
    Function(Rc<Func>),
    Builtin,
    /// An in-process utility (D1080).
    Utility,
    /// A program: its path, and whether the hash table had it.
    File(Vec<u8>, bool),
}

#[derive(Clone, Copy, Default)]
struct Lookup {
    /// Every match, not only the first (`type -a`).
    all: bool,
    /// Skip functions (`type -f`).
    no_funcs: bool,
    /// Only programs, searched in `PATH` even when hashed (`type -P`).
    path_only: bool,
}

async fn lookup(vp: &Vp, name: &[u8], o: Lookup) -> Vec<Kind> {
    let mut found = Vec::new();
    if !o.path_only {
        if vp.expand_aliases
            && let Some(a) = vp.aliases.get(name)
        {
            found.push(Kind::Alias(a.to_vec()));
        }
        if KEYWORDS.contains(&name) {
            found.push(Kind::Keyword);
        }
        if !o.no_funcs
            && (o.all || !super::is_special(vp, name))
            && let Some(f) = std::str::from_utf8(name).ok().and_then(|n| vp.funcs.get(n))
        {
            found.push(Kind::Function(f.clone()));
        }
        if super::is_builtin(vp, name) {
            found.push(Kind::Builtin);
        }
        if crate::util::lookup(vp, name).await.is_some() {
            match hash::search(vp, name, None).await {
                Some(p) if !o.all => found.push(Kind::File(p, false)),
                _ => found.push(Kind::Utility),
            }
        }
        if !found.is_empty() && !o.all {
            return found;
        }
    }
    if name.contains(&b'/') {
        let p = vp.abs_path(name);
        if is_executable(vp, &p).await {
            found.push(Kind::File(name.to_vec(), false));
        }
        return found;
    }
    if !o.all {
        if let Some(p) = hash::hashed(vp, name) {
            found.push(Kind::File(p, true));
        } else if let Some(p) = hash::search(vp, name, None).await {
            found.push(Kind::File(p, false));
        }
        return found;
    }
    // Every directory of PATH, one at a time.
    let path = vp
        .var("PATH")
        .map(<[u8]>::to_vec)
        .unwrap_or_else(|| exec::DEFAULT_PATH.to_vec());
    if o.path_only
        && let Some(p) = hash::hashed(vp, name)
    {
        found.push(Kind::File(p, false));
    }
    for dir in vp.host().path_entries(&path) {
        let dir = &dir[..];
        if let Some(p) = hash::search(vp, name, Some(dir)).await
            && (is_executable(vp, &vp.abs_path(&p)).await
                || vp.host().stat(&vp.abs_path(&p), true).await.is_err())
            && !found
                .iter()
                .any(|k| matches!(k, Kind::File(q, _) if *q == p))
        {
            found.push(Kind::File(p, false));
        }
    }
    found
}

async fn is_executable(vp: &Vp, p: &[u8]) -> bool {
    matches!(vp.host().stat(p, true).await, Ok(m) if m.kind != FileKind::Dir)
        && matches!(vp.host().access(p, Access::Execute).await, Ok(true))
}

/// A function as `type` prints it: the definition in bash's layout.
fn function_text(f: &Func) -> String {
    let ast = &f.code.ast;
    let def = ast.cmd_ids().find(
        |&id| matches!(&ast.cmd(id).kind, psh_syntax::CommandKind::Function(d) if d.body == f.body),
    );
    psh_syntax::print_function_bash(ast, def.unwrap_or(f.body))
}

/// `command [-pVv] command [arg ...]`
pub(super) async fn command(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let (mut default_path, mut short, mut verbose) = (false, false, false);
    let mut i = 1;
    while let Some(a) = args.get(i) {
        if a == b"--" {
            i += 1;
            break;
        }
        if a.len() < 2 || a[0] != b'-' {
            break;
        }
        for &c in &a[1..] {
            match c {
                b'p' => default_path = true,
                b'v' => (short, verbose) = (true, false),
                b'V' => (short, verbose) = (false, true),
                _ => {
                    vp.error(&format!("command: -{}: invalid option", c as char))
                        .await;
                    vp.error_raw(b"command: usage: command [-pVv] command [arg ...]\n")
                        .await;
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    let rest = &args[i..];
    if rest.is_empty() {
        return Ok(0);
    }
    if short || verbose {
        let mut any = false;
        let mut buf = Vec::new();
        for name in rest {
            let mut kinds = lookup(vp, name, Lookup::default()).await;
            // `-p` runs the program the standard path has, not psh's utility (below), so
            // that is what it reports too; a hashed program stays (as in bash).
            if default_path
                && !name.contains(&b'/')
                && kinds
                    .first()
                    .is_none_or(|k| matches!(k, Kind::File(_, false) | Kind::Utility))
            {
                let path = vp.host().standard_path();
                kinds = hash::search(vp, name, Some(&path))
                    .await
                    .map(|p| Kind::File(p, false))
                    .into_iter()
                    .collect();
            }
            let Some(k) = kinds.into_iter().next() else {
                if verbose {
                    flush(vp, "command", &mut buf).await?;
                    vp.error(&format!("command: {}: not found", s(name))).await;
                }
                continue;
            };
            any = true;
            if verbose {
                describe(&mut buf, name, &k);
            } else {
                match k {
                    Kind::Alias(a) => {
                        buf.extend_from_slice(&super::alias::definition(name, &a));
                        continue;
                    }
                    Kind::File(p, _) => buf.extend_from_slice(&p),
                    _ => buf.extend_from_slice(name),
                }
                buf.push(b'\n');
            }
        }
        if !flush(vp, "command", &mut buf).await? {
            return Ok(1);
        }
        return Ok(i32::from(!any));
    }
    let name = &rest[0];
    if let Some(b) = super::get(vp, name) {
        return super::call(vp, b, rest).await;
    }
    // `command -p` asks for the standard program from the default PATH.
    if !default_path && let Some(e) = crate::util::lookup(vp, name).await {
        return crate::util::run(vp, e, rest.to_vec(), Vec::new()).await;
    }
    let opts = ExtOpts {
        path: default_path.then(|| vp.host().standard_path()),
        ..ExtOpts::default()
    };
    exec::run_external_with(vp, rest.to_vec(), Vec::new(), opts).await
}

/// Writes what `buf` holds to stdout; `false` after a write error (reported).
async fn flush(vp: &Vp, name: &str, buf: &mut Vec<u8>) -> Result<bool, Unwind> {
    if buf.is_empty() {
        return Ok(true);
    }
    super::write_fd(vp, 1, name, &std::mem::take(buf)).await
}

/// `NAME is …` lines of `type` and `command -V`.
fn describe(buf: &mut Vec<u8>, name: &[u8], k: &Kind) {
    buf.extend_from_slice(name);
    match k {
        Kind::Alias(a) => {
            buf.extend_from_slice(b" is aliased to `");
            buf.extend_from_slice(a);
            buf.extend_from_slice(b"'\n");
        }
        Kind::Keyword => buf.extend_from_slice(b" is a shell keyword\n"),
        Kind::Builtin => buf.extend_from_slice(b" is a shell builtin\n"),
        Kind::Utility => buf.extend_from_slice(b" is a psh utility\n"),
        Kind::Function(f) => {
            buf.extend_from_slice(b" is a function\n");
            buf.extend_from_slice(function_text(f).as_bytes());
            buf.push(b'\n');
        }
        Kind::File(p, true) => {
            buf.extend_from_slice(b" is hashed (");
            buf.extend_from_slice(p);
            buf.extend_from_slice(b")\n");
        }
        Kind::File(p, false) => {
            buf.extend_from_slice(b" is ");
            buf.extend_from_slice(p);
            buf.push(b'\n');
        }
    }
}

/// `type [-afptP] name [name ...]`
pub(super) async fn type_(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let mut o = Lookup::default();
    let (mut kind_only, mut path, mut force_path) = (false, false, false);
    let mut i = 1;
    while let Some(a) = args.get(i) {
        if a == b"--" {
            i += 1;
            break;
        }
        if a.len() < 2 || a[0] != b'-' {
            break;
        }
        for &c in &a[1..] {
            match c {
                b'a' => o.all = true,
                b'f' => o.no_funcs = true,
                b't' => kind_only = true,
                b'p' => path = true,
                b'P' => force_path = true,
                _ => {
                    vp.error(&format!("type: -{}: invalid option", c as char))
                        .await;
                    vp.error_raw(b"type: usage: type [-afptP] name [name ...]\n")
                        .await;
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    o.path_only = force_path;
    let mut status = 0;
    let mut buf = Vec::new();
    for name in &args[i..] {
        let mut kinds = lookup(vp, name, o).await;
        if !o.all {
            kinds.truncate(1);
        }
        if kinds.is_empty() {
            status = 1;
            if !kind_only && !path && !force_path {
                flush(vp, "type", &mut buf).await?;
                vp.error(&format!("type: {}: not found", s(name))).await;
            }
            continue;
        }
        for k in &kinds {
            if kind_only {
                buf.extend_from_slice(match k {
                    Kind::Alias(_) => b"alias\n".as_slice(),
                    Kind::Keyword => b"keyword\n",
                    Kind::Function(_) => b"function\n",
                    Kind::Builtin | Kind::Utility => b"builtin\n",
                    Kind::File(..) => b"file\n",
                });
            } else if path || force_path {
                if let Kind::File(p, _) = k {
                    buf.extend_from_slice(p);
                    buf.push(b'\n');
                }
            } else {
                describe(&mut buf, name, k);
            }
        }
    }
    if !flush(vp, "type", &mut buf).await? {
        return Ok(1);
    }
    Ok(status)
}

/// `builtin shell-builtin [arg ...]`
pub(super) async fn builtin(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let rest = match args.get(1) {
        Some(a) if a == b"--" => &args[2..],
        Some(a) if a.len() > 1 && a[0] == b'-' => {
            vp.error(&format!("builtin: {}: invalid option", s(&a[..2])))
                .await;
            vp.error_raw(b"builtin: usage: builtin [shell-builtin [arg ...]]\n")
                .await;
            return Ok(2);
        }
        _ => &args[1..],
    };
    let Some(name) = rest.first() else {
        return Ok(0);
    };
    match super::get(vp, name) {
        Some(b) => super::call(vp, b, rest).await,
        None => {
            vp.error(&format!("builtin: {}: not a shell builtin", s(name)))
                .await;
            Ok(1)
        }
    }
}

/// `exec [-cl] [-a name] [command [argument ...]]`: runs the command and ends the
/// (sub)shell with its status. Without a command, its redirections stay (the caller
/// applies them for good).
pub(super) async fn exec_(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let mut opts = ExtOpts {
        exec: true,
        ..ExtOpts::default()
    };
    let mut login = false;
    let mut i = 1;
    'opts: while let Some(a) = args.get(i) {
        if a == b"--" {
            i += 1;
            break;
        }
        if a.len() < 2 || a[0] != b'-' {
            break;
        }
        for (j, &c) in a.iter().enumerate().skip(1) {
            match c {
                b'c' => opts.clear_env = true,
                b'l' => login = true,
                b'a' => {
                    let value = if j + 1 < a.len() {
                        a[j + 1..].to_vec()
                    } else {
                        i += 1;
                        match args.get(i) {
                            Some(v) => v.clone(),
                            None => {
                                vp.error("exec: -a: option requires an argument").await;
                                vp.error_raw(EXEC_USAGE).await;
                                return Ok(2);
                            }
                        }
                    };
                    opts.arg0 = Some(value);
                    i += 1;
                    continue 'opts;
                }
                _ => {
                    vp.error(&format!("exec: -{}: invalid option", c as char))
                        .await;
                    vp.error_raw(EXEC_USAGE).await;
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    let rest = &args[i..];
    let Some(name) = rest.first() else {
        return Ok(0);
    };
    if login {
        let mut a0 = b"-".to_vec();
        a0.extend_from_slice(opts.arg0.as_deref().unwrap_or(name));
        opts.arg0 = Some(a0);
    }
    // A utility runs in place of the shell: then the shell ends with its status.
    if let Some(e) = crate::util::lookup(vp, name).await {
        let mut argv = rest.to_vec();
        // A shell psh runs (D1410) has `-a`'s name, and `-l`'s, as its `$0`.
        if crate::util::is_nested_shell(e)
            && let Some(a0) = opts.arg0
        {
            argv[0] = a0;
        }
        let st = crate::util::run_exec(vp, e, argv).await?;
        return Err(Unwind::Exit(st));
    }
    if !name.contains(&b'/') && hash::find(vp, name, None, false).await.is_none() {
        vp.error(&format!("exec: {}: not found", s(name))).await;
        return Err(Unwind::Exit(127));
    }
    let st = exec::run_external_with(vp, rest.to_vec(), Vec::new(), opts).await?;
    Err(Unwind::Exit(st))
}

const EXEC_USAGE: &[u8] =
    b"exec: usage: exec [-cl] [-a name] [command [argument ...]] [redirection ...]\n";

/// `enable [-a] [-nps] [name ...]`: lists builtins, turns them off (`-n`) and on;
/// loading builtins from files (`-f`, `-d`) is not supported.
pub(super) async fn enable(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let (mut all, mut off, mut special) = (false, false, false);
    let mut i = 1;
    while let Some(a) = args.get(i) {
        if a == b"--" {
            i += 1;
            break;
        }
        if a.len() < 2 || a[0] != b'-' {
            break;
        }
        for &c in &a[1..] {
            match c {
                b'a' => all = true,
                b'n' => off = true,
                b'p' => {}
                b's' => special = true,
                b'f' | b'd' => {
                    return crate::expand::unsupported(vp, &format!("enable -{}", c as char)).await;
                }
                _ => {
                    vp.error(&format!("enable: -{}: invalid option", c as char))
                        .await;
                    vp.error_raw(b"enable: usage: enable [-a] [-dnps] [-f filename] [name ...]\n")
                        .await;
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    let names = &args[i..];
    if !names.is_empty() {
        let mut status = 0;
        for n in names {
            if super::find(n).is_none() && !BASH_BUILTINS.contains(&n.as_slice()) {
                vp.error(&format!("enable: {}: not a shell builtin", s(n)))
                    .await;
                status = 1;
                continue;
            }
            let d = Rc::make_mut(&mut vp.disabled);
            d.retain(|x| x != n);
            if off {
                d.push(n.clone());
            }
        }
        return Ok(status);
    }
    let mut buf = Vec::new();
    for name in BASH_BUILTINS {
        let disabled = vp.disabled.iter().any(|d| d == name);
        if special && !SPECIAL.contains(name) {
            continue;
        }
        if !all && disabled != off {
            continue;
        }
        buf.extend_from_slice(if disabled { b"enable -n " } else { b"enable " });
        buf.extend_from_slice(name);
        buf.push(b'\n');
    }
    out(vp, "enable", &buf).await
}

/// `times`: the shell's and its children's user and system times, zero on a host
/// without that clock.
pub(super) async fn times(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    if let Some(a) = args.get(1)
        && a.len() > 1
        && a[0] == b'-'
        && a != b"--"
    {
        vp.error(&format!("times: {}: invalid option", s(&a[..2])))
            .await;
        vp.error_raw(b"times: usage: times\n").await;
        return Ok(2);
    }
    let cpu = vp.core.host.cpu_times();
    let mut text = Vec::new();
    for (time, end) in [
        (cpu.user, b' '),
        (cpu.system, b'\n'),
        (cpu.children_user, b' '),
        (cpu.children_system, b'\n'),
    ] {
        crate::timing::clock(&mut text, 3, true, time);
        text.push(end);
    }
    out(vp, "times", &text).await
}

/// `caller [expr]`: where the current function (or frame `expr`) was called from.
pub(super) async fn caller(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let rest = match args.get(1) {
        Some(a) if a == b"--" => &args[2..],
        _ => &args[1..],
    };
    // Innermost first, as bash's FUNCNAME/BASH_SOURCE/BASH_LINENO arrays.
    let calls: Vec<_> = vp.calls.iter().rev().cloned().collect();
    let line = match rest.first() {
        None => {
            let Some(c) = calls.first() else {
                return Ok(1);
            };
            let src = calls.get(1).map_or("NULL", |c| c.source.as_str());
            format!("{} {src}\n", c.line)
        }
        Some(a) => {
            let n = std::str::from_utf8(a)
                .ok()
                .and_then(|t| t.trim().parse::<usize>().ok());
            let Some(n) = n else {
                vp.error(&format!("caller: {}: invalid number", s(a))).await;
                vp.error_raw(b"caller: usage: caller [expr]\n").await;
                return Ok(2);
            };
            match (calls.get(n), calls.get(n + 1)) {
                (Some(c), Some(up)) => format!("{} {} {}\n", c.line, up.func, up.source),
                _ => return Ok(1),
            }
        }
    };
    out(vp, "caller", line.as_bytes()).await
}
