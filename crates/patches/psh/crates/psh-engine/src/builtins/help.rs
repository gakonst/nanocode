//! `help` (synopses and one-line descriptions, in psh's own words), and the
//! interactive-only builtins a script can still call: `logout`, `history`.

use crate::pattern::Pattern;
use crate::state::{R, Vp};

use super::{out, s};

/// Name, synopsis (bash's usage line), what it does.
const TOPICS: &[(&str, &str, &str)] = &[
    (
        ".",
        ". [-p path] filename [arguments]",
        "Run the commands of a file in this shell.",
    ),
    (":", ":", "Do nothing, successfully."),
    (
        "[",
        "[ arg... ]",
        "Evaluate a conditional expression; the last argument must be `]'.",
    ),
    (
        "alias",
        "alias [-p] [name[=value] ... ]",
        "Define or list aliases.",
    ),
    ("bg", "bg [job_spec ...]", "Resume jobs in the background."),
    (
        "bind",
        "bind [-lpsvPSVX] [-m keymap] [-f filename] [-q name] [-u name] [-r keyseq] [-x keyseq:shell-command] [keyseq:readline-function or readline-command]",
        "Set line editing key bindings (interactive shells only).",
    ),
    (
        "break",
        "break [n]",
        "Leave the innermost (or the n-th enclosing) loop.",
    ),
    (
        "builtin",
        "builtin [shell-builtin [arg ...]]",
        "Run a builtin, bypassing functions of the same name.",
    ),
    (
        "caller",
        "caller [expr]",
        "Print where the current function call came from.",
    ),
    (
        "cd",
        "cd [-L|[-P [-e]]] [-@] [dir]",
        "Change the working directory.",
    ),
    (
        "command",
        "command [-pVv] command [arg ...]",
        "Run a command bypassing functions, or describe commands.",
    ),
    (
        "compgen",
        "compgen [-V varname] [-abcdefgjksuv] [-o option] [-A action] [-G globpat] [-W wordlist] [-F function] [-C command] [-X filterpat] [-P prefix] [-S suffix] [word]",
        "Generate completions (interactive shells only).",
    ),
    (
        "complete",
        "complete [-abcdefgjksuv] [-pr] [-DEI] [-o option] [-A action] [-G globpat] [-W wordlist] [-F function] [-C command] [-X filterpat] [-P prefix] [-S suffix] [name ...]",
        "Define completions (interactive shells only).",
    ),
    (
        "compopt",
        "compopt [-o|+o option] [-DEI] [name ...]",
        "Change completion options (interactive shells only).",
    ),
    (
        "continue",
        "continue [n]",
        "Start the next iteration of the innermost (or the n-th enclosing) loop.",
    ),
    (
        "declare",
        "declare [-aAfFgiIlnrtux] [name[=value] ...] or declare -p [-aAfFilnrtux] [name ...]",
        "Declare variables and their attributes.",
    ),
    (
        "dirs",
        "dirs [-clpv] [+N] [-N]",
        "List the directory stack.",
    ),
    (
        "disown",
        "disown [-h] [-ar] [jobspec ... | pid ...]",
        "Remove jobs from the job table.",
    ),
    (
        "echo",
        "echo [-neE] [arg ...]",
        "Write the arguments, separated by spaces, and a newline.",
    ),
    (
        "enable",
        "enable [-a] [-dnps] [-f filename] [name ...]",
        "List builtins, or turn them off and on.",
    ),
    (
        "eval",
        "eval [arg ...]",
        "Run the arguments, joined by spaces, as shell code.",
    ),
    (
        "exec",
        "exec [-cl] [-a name] [command [argument ...]] [redirection ...]",
        "Replace the shell with a command, or keep redirections for the shell.",
    ),
    (
        "exit",
        "exit [n]",
        "Leave the shell with status n (default: the last status).",
    ),
    (
        "export",
        "export [-fn] [name[=value] ...] or export -p [-f]",
        "Mark variables for the environment of commands.",
    ),
    ("false", "false", "Fail: return status 1."),
    (
        "fc",
        "fc [-e ename] [-lnr] [first] [last] or fc -s [pat=rep] [command]",
        "Edit or rerun history entries (interactive shells only).",
    ),
    ("fg", "fg [job_spec]", "Bring a job to the foreground."),
    (
        "getopts",
        "getopts optstring name [arg ...]",
        "Parse options from the positional parameters or arguments.",
    ),
    (
        "hash",
        "hash [-lr] [-p pathname] [-dt] [name ...]",
        "Remember or show where commands were found.",
    ),
    (
        "help",
        "help [-dms] [pattern ...]",
        "Show what builtins do.",
    ),
    (
        "history",
        "history [-c] [-d offset] [n] or history -anrw [filename] or history -ps arg [arg...]",
        "Show or change the command history (empty outside interactive shells).",
    ),
    (
        "jobs",
        "jobs [-lnprs] [jobspec ...] or jobs -x command [args]",
        "List background jobs.",
    ),
    (
        "kill",
        "kill [-s sigspec | -n signum | -sigspec] pid | jobspec ... or kill -l [sigspec]",
        "Send a signal to processes or jobs, or list signals.",
    ),
    (
        "let",
        "let arg [arg ...]",
        "Evaluate arithmetic expressions.",
    ),
    (
        "local",
        "local [option] name[=value] ...",
        "Create variables local to a function.",
    ),
    ("logout", "logout [n]", "Leave a login shell."),
    (
        "mapfile",
        "mapfile [-d delim] [-n count] [-O origin] [-s count] [-t] [-u fd] [-C callback] [-c quantum] [array]",
        "Read lines into an indexed array.",
    ),
    (
        "popd",
        "popd [-n] [+N | -N]",
        "Remove a directory from the directory stack.",
    ),
    (
        "printf",
        "printf [-v var] format [arguments]",
        "Format and write the arguments.",
    ),
    (
        "pushd",
        "pushd [-n] [+N | -N | dir]",
        "Add a directory to the directory stack.",
    ),
    ("pwd", "pwd [-LP]", "Print the working directory."),
    (
        "read",
        "read [-Eers] [-a array] [-d delim] [-i text] [-n nchars] [-N nchars] [-p prompt] [-t timeout] [-u fd] [name ...]",
        "Read a line and split it into variables.",
    ),
    (
        "readarray",
        "readarray [-d delim] [-n count] [-O origin] [-s count] [-t] [-u fd] [-C callback] [-c quantum] [array]",
        "Read lines into an indexed array.",
    ),
    (
        "readonly",
        "readonly [-aAf] [name[=value] ...] or readonly -p",
        "Make variables unchangeable.",
    ),
    (
        "return",
        "return [n]",
        "Return from a function or sourced file.",
    ),
    (
        "set",
        "set [-abefhkmnptuvxBCEHPT] [-o option-name] [--] [-] [arg ...]",
        "Set shell options and positional parameters.",
    ),
    (
        "shift",
        "shift [n]",
        "Drop the first n positional parameters.",
    ),
    (
        "shopt",
        "shopt [-pqsu] [-o] [optname ...]",
        "Set and show shell options.",
    ),
    (
        "source",
        "source [-p path] filename [arguments]",
        "Run the commands of a file in this shell.",
    ),
    (
        "suspend",
        "suspend [-f]",
        "Stop the shell (interactive shells only).",
    ),
    ("test", "test [expr]", "Evaluate a conditional expression."),
    (
        "times",
        "times",
        "Print the user and system times of the shell and its children.",
    ),
    (
        "trap",
        "trap [-Plp] [[action] signal_spec ...]",
        "Run commands when the shell receives signals or exits.",
    ),
    ("true", "true", "Succeed: return status 0."),
    (
        "type",
        "type [-afptP] name [name ...]",
        "Describe how each name would be run.",
    ),
    (
        "typeset",
        "typeset [-aAfFgiIlnrtux] name[=value] ... or typeset -p [-aAfFilnrtux] [name ...]",
        "Declare variables and their attributes (as declare).",
    ),
    (
        "ulimit",
        "ulimit [-SHabcdefiklmnpqrstuvxPRT] [limit]",
        "Show or set resource limits.",
    ),
    (
        "umask",
        "umask [-p] [-S] [mode]",
        "Show or set the file creation mask.",
    ),
    ("unalias", "unalias [-a] name [name ...]", "Remove aliases."),
    (
        "unset",
        "unset [-f] [-v] [-n] [name ...]",
        "Remove variables or functions.",
    ),
    (
        "wait",
        "wait [-fn] [-p var] [id ...]",
        "Wait for background jobs and return their status.",
    ),
];

/// `help [-dms] [pattern ...]`
pub(super) fn topic_text(name: &[u8]) -> Option<String> {
    TOPICS
        .iter()
        .find(|(n, _, _)| n.as_bytes() == name)
        .map(|(n, syn, what)| format!("{n}: {syn}\n    {what}\n"))
}

pub(super) async fn help(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let mut mode = b'm';
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
                b'd' | b'm' | b's' => mode = c,
                _ => {
                    vp.error(&format!("help: -{}: invalid option", c as char))
                        .await;
                    vp.error_raw(b"help: usage: help [-dms] [pattern ...]\n")
                        .await;
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    let patterns = &args[i..];
    let mut buf = Vec::new();
    if patterns.is_empty() {
        buf.extend_from_slice(b"psh: shell builtins. Type `help name' for more about `name'.\n\n");
        for (_, syn, _) in TOPICS {
            buf.extend_from_slice(syn.as_bytes());
            buf.push(b'\n');
        }
        return out(vp, "help", &buf).await;
    }
    let mut status = 0;
    for p in patterns {
        let pat = Pattern::new(p, false);
        let mut found = false;
        for (name, syn, what) in TOPICS {
            if !(pat.matches(name.as_bytes(), false) || name.as_bytes().starts_with(p)) {
                continue;
            }
            found = true;
            let text = match mode {
                b's' => format!("{name}: {syn}\n"),
                b'd' => format!("{name} - {what}\n"),
                _ => format!("{name}: {syn}\n    {what}\n"),
            };
            buf.extend_from_slice(text.as_bytes());
        }
        if !found {
            if !buf.is_empty() {
                super::write_fd(vp, 1, "help", &std::mem::take(&mut buf)).await?;
            }
            vp.error(&format!(
                "help: no help topics match `{}'.  Try `help help' or `man -k {}' or `info {}'.",
                s(p),
                s(p),
                s(p)
            ))
            .await;
            status = 1;
        }
    }
    if !buf.is_empty() {
        out(vp, "help", &buf).await?;
    }
    Ok(status)
}

/// `logout`: only a login shell may; psh never is one.
pub(super) async fn logout(vp: &mut Vp, _args: &[Vec<u8>]) -> R {
    vp.error("logout: not login shell: use `exit'").await;
    Ok(1)
}

/// `history`: a script's shell keeps no history, so there is nothing to show or
/// change; the arguments are still checked.
pub(super) async fn history(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    const USAGE: &[u8] = b"history: usage: history [-c] [-d offset] [n] or history -anrw [filename] or history -ps arg [arg...]\n";
    let mut rest = &args[1..];
    while let Some(a) = rest.first() {
        if a == b"--" {
            rest = &rest[1..];
            break;
        }
        if a.len() < 2 || a[0] != b'-' {
            break;
        }
        for &c in &a[1..] {
            match c {
                b'c' | b'a' | b'n' | b'r' | b'w' | b'p' | b's' => {}
                b'd' => return Ok(0),
                _ => {
                    vp.error(&format!("history: -{}: invalid option", c as char))
                        .await;
                    vp.error_raw(USAGE).await;
                    return Ok(2);
                }
            }
        }
        rest = &rest[1..];
    }
    match rest {
        [] => Ok(0),
        [n] if std::str::from_utf8(n)
            .ok()
            .and_then(|t| t.trim().parse::<i64>().ok())
            .is_some() =>
        {
            Ok(0)
        }
        [n] => {
            vp.error(&format!("history: {}: numeric argument required", s(n)))
                .await;
            Ok(1)
        }
        _ => {
            vp.error("history: too many arguments").await;
            Ok(1)
        }
    }
}
