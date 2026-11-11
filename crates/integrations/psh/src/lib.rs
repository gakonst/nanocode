use psh_classify::CommandInfo;
use psh_syntax::{CommandKind, Options};
use std::fmt;

#[cfg(unix)]
pub const ARGUMENT: &str = "psh-shell";

#[cfg(unix)]
#[must_use]
pub fn serve_if_requested(
    psh: impl FnOnce(Vec<std::ffi::OsString>) -> std::process::ExitCode,
) -> Option<std::process::ExitCode> {
    let mut arguments = std::env::args_os().skip(1);
    (arguments.next()? == ARGUMENT).then(|| psh(arguments.collect()))
}

#[cfg(unix)]
pub fn shell() -> std::io::Result<nanocode_home::shell::Shell> {
    Ok(nanocode_home::shell::Shell::psh(
        std::env::current_exe()?,
        vec![ARGUMENT.to_owned()],
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    Psh,
    System(Reason),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    Syntax(String),
    Command(&'static str),
    Builtin(String),
    Signal(String),
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(message) => write!(f, "psh does not parse it: {message}"),
            Self::Command(name) => write!(f, "psh has no `{name}`"),
            Self::Builtin(name) => write!(f, "psh has no `{name}` builtin"),
            Self::Signal(number) => write!(
                f,
                "signal {number} is another signal in psh, which numbers them as Linux does"
            ),
        }
    }
}

const INTERACTIVE: [&str; 8] = [
    "bg", "bind", "compgen", "complete", "compopt", "fc", "fg", "suspend",
];

const SHARED_SIGNALS: [&str; 13] = [
    "0", "1", "2", "3", "4", "5", "6", "8", "9", "11", "13", "14", "15",
];

#[must_use]
pub fn plan(script: &str) -> Plan {
    unsupported(script).map_or(Plan::Psh, Plan::System)
}

fn unsupported(script: &str) -> Option<Reason> {
    let ast = match psh_syntax::parse(script) {
        Ok(ast) => ast,
        Err(error) => match psh_syntax::parse_with(script, Options { extglob: true }) {
            Ok(ast) => ast,
            Err(_) => return Some(Reason::Syntax(error.message)),
        },
    };
    let lacking = ast.cmd_ids().find_map(|id| match ast.cmd(id).kind {
        CommandKind::Coproc(_) => Some(Reason::Command("coproc")),
        _ => None,
    });
    if lacking.is_some() {
        return lacking;
    }
    let classified = psh_classify::classify_ast(&ast);
    classified.commands.iter().find_map(command).or_else(|| {
        classified
            .sources
            .iter()
            .find_map(|source| match unsupported(&source.text) {
                Some(Reason::Syntax(_)) | None => None,
                reason => reason,
            })
    })
}

fn command(command: &CommandInfo) -> Option<Reason> {
    let name = command.name.as_deref()?;
    let mut operands = command
        .words
        .iter()
        .skip(1)
        .filter_map(|word| word.value.as_deref());
    if INTERACTIVE.contains(&name) {
        return Some(Reason::Builtin(name.to_owned()));
    }
    match name {
        "enable" => operands
            .take_while(|operand| operand.starts_with('-'))
            .any(|flags| flags.contains(['f', 'd']))
            .then_some(Reason::Command("enable -f")),
        "set" => {
            let mut posix = false;
            operands.find_map(|operand| {
                let named = std::mem::replace(&mut posix, operand == "-o");
                if named && operand == "posix" {
                    Some(Reason::Command("set -o posix"))
                } else if operand.starts_with('-') && !operand.starts_with("--") {
                    operand
                        .contains('r')
                        .then_some(Reason::Command("set -r"))
                } else {
                    None
                }
            })
        }
        "kill" | "trap" if cfg!(not(any(target_os = "linux", target_os = "android"))) => operands
            .map(|operand| operand.strip_prefix('-').unwrap_or(operand))
            .find(|operand| {
                !operand.is_empty()
                    && operand.bytes().all(|byte| byte.is_ascii_digit())
                    && !SHARED_SIGNALS.contains(operand)
                    && (name == "trap" || operand.len() <= 2)
            })
            .map(|number| Reason::Signal(number.to_owned())),
        _ => None,
    }
}
