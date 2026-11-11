//! Understanding commands: what a script does, from the same AST the engine runs.
//!
//! [`classify`] parses a script and reports every simple command it would run (including
//! those inside `$(…)`, `<(…)`, here-documents, literal `bash -c` and `eval` strings, and
//! commands wrapped by `env`, `timeout`, `xargs`, `find -exec` and the like), with its
//! words, redirections, categories and the paths it reads and writes.
//!
//! The classifier is conservative: anything it cannot know statically (a command name
//! from a variable, `eval` of a non-literal, `source`, functions and aliases defined in
//! the script, inline interpreters, local scripts, unknown programs) is
//! [`Category::Unknown`], never read-only.
//!
//! ```
//! use psh_classify::{Category, classify};
//! let c = classify("cd src && grep -rn TODO . | head -20");
//! assert!(c.read_only());
//! assert!(c.truncated);
//! let c = classify("git commit -m \"$(rm -rf /)\"");
//! assert_eq!(c.primary(), Category::Vcs);
//! assert!(c.categories.contains(Category::Mutate));
//! ```
#![no_std]

extern crate alloc;

mod category;
mod rules;
mod sed;
mod sqlite;
pub mod utils;
mod vars;
mod walk;

use alloc::string::String;
use alloc::vec::Vec;

pub use category::{Categories, Category};
pub use psh_syntax::{ParseError, RedirOp, Span};

/// The result of classifying a script.
#[derive(Clone, Debug, Default)]
pub struct Classification {
    /// Every simple command, in the order the walk meets them: a command before the
    /// commands nested in it.
    pub commands: Vec<CommandInfo>,
    /// Shell code found in strings (`bash -c '…'`, `eval`, `trap`, aliases, backquotes,
    /// literal here-documents fed to a shell). Spans of commands from such a string are
    /// relative to its `text`.
    pub sources: Vec<NestedSource>,
    /// The union of the commands' categories, plus [`Category::LongRunning`] for
    /// background jobs.
    pub categories: Categories,
    /// Paths read: input redirections and the file operands of known utilities. Best
    /// effort; reads do not make a command less safe.
    pub reads: Vec<PathEffect>,
    /// Paths written, created or removed: output redirections (except `/dev/null` and
    /// the standard streams) and the operands and options of known writers.
    pub writes: Vec<PathEffect>,
    /// Some pipeline cuts its output short (`| head`, `| tail`, `| sed -n …`).
    pub truncated: bool,
    /// The script did not parse; it is [`Category::Unknown`].
    pub error: Option<ParseError>,
}

impl Classification {
    /// The category for a one-word summary: the highest-priority one present.
    pub fn primary(&self) -> Category {
        self.categories.primary()
    }

    /// Only reads: no writes, no network, nothing unknown. Safe to auto-approve.
    pub fn read_only(&self) -> bool {
        self.error.is_none() && self.categories.read_only() && self.writes.is_empty()
    }
}

/// One simple command.
#[derive(Clone, Debug)]
pub struct CommandInfo {
    /// The whole command, with its redirections.
    pub span: Span,
    /// `None`: spans are in the classified script; `Some(i)`: in `sources[i].text`.
    pub source: Option<usize>,
    /// The command name after quote removal, if it is literal (the base name for
    /// programs in system directories: `/usr/bin/grep` is `grep`). A wrapped command
    /// (`cargo` in `timeout 5 cargo test`) is a command of its own, with the wrapper as
    /// its parent.
    pub name: Option<String>,
    /// The words, the name first.
    pub words: Vec<Arg>,
    /// Assignments before the name (`A=1 cmd`), as written.
    pub assigns: Vec<Arg>,
    pub redirects: Vec<RedirInfo>,
    /// The command this one runs inside of: the one whose word holds the `$(…)`, the
    /// wrapper, the `bash -c`.
    pub parent: Option<usize>,
    pub via: Via,
    pub categories: Categories,
    /// Why the command is [`Category::Unknown`], when it is.
    pub reason: Option<&'static str>,
}

impl CommandInfo {
    pub fn primary(&self) -> Category {
        self.categories.primary()
    }
}

/// A word: its span and text as written, and its value if it is literal.
#[derive(Clone, Debug)]
pub struct Arg {
    pub span: Span,
    pub text: String,
    /// The value after quote removal, or `None` if it has expansions. Unquoted glob
    /// characters stay in the value; see `glob`.
    pub value: Option<String>,
    /// Has unquoted glob characters, so it may expand to other words.
    pub glob: bool,
}

impl Arg {
    /// Whether the word could start with `-` once expanded, and so be taken as an
    /// option: it does literally, or it starts with an expansion (`$x`, `"$x"`,
    /// `` `…` ``, a brace). `src/$f` and `{a,b}.rs` cannot.
    pub fn may_be_option(&self) -> bool {
        if let Some(v) = &self.value {
            return v.starts_with('-');
        }
        // `$HOME` and `$PWD` are absolute paths: `"$HOME/x"` cannot be an option.
        let unquoted = self.text.trim_start_matches('"');
        for var in ["HOME", "PWD"] {
            for form in [alloc::format!("${var}"), alloc::format!("${{{var}}}")] {
                if let Some(rest) = unquoted.strip_prefix(form.as_str())
                    && !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
                    && !(form.len() == var.len() + 1 && rest.starts_with(['[', ':']))
                {
                    return false;
                }
            }
        }
        // Skip opening quotes and backslashes; a `$` among them starts an expansion.
        let t = self.text.trim_start_matches(['"', '\'', '\\', '$']);
        let lead = &self.text[..self.text.len() - t.len()];
        lead.contains('$') || matches!(t.chars().next(), Some('-' | '{' | '`') | None)
    }
}

#[derive(Clone, Debug)]
pub struct RedirInfo {
    pub span: Span,
    /// `2>`: the fd number written before the operator.
    pub fd: Option<u32>,
    pub op: RedirOp,
    /// The target word; `None` for a here-document.
    pub target: Option<Arg>,
}

/// How a command comes to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// Directly in the script or its compound commands.
    Script,
    /// `$(…)` or `${ …; }`.
    CommandSubst,
    /// `` `…` ``.
    Backtick,
    /// `<(…)` or `>(…)`.
    ProcessSubst,
    /// Wrapped by `env`, `timeout`, `nice`, `nohup`, `command`, `exec`, `sudo`, `uv run`, ...
    Wrapper,
    /// `xargs cmd`: run with more arguments read from stdin.
    Xargs,
    /// `find … -exec cmd … ;`.
    FindExec,
    /// The string of `bash -c` (or a literal script on a shell's stdin).
    ShellString,
    /// The string of `eval`.
    Eval,
    /// A `trap` handler.
    Trap,
    /// An alias body.
    Alias,
}

impl Via {
    pub fn name(self) -> &'static str {
        match self {
            Via::Script => "script",
            Via::CommandSubst => "command-substitution",
            Via::Backtick => "backtick",
            Via::ProcessSubst => "process-substitution",
            Via::Wrapper => "wrapper",
            Via::Xargs => "xargs",
            Via::FindExec => "find-exec",
            Via::ShellString => "shell-string",
            Via::Eval => "eval",
            Via::Trap => "trap",
            Via::Alias => "alias",
        }
    }
}

/// Shell code from a string, classified as a script of its own.
#[derive(Clone, Debug)]
pub struct NestedSource {
    /// Where the string is: in the classified script (`source: None`) or in another
    /// nested source.
    pub span: Span,
    pub source: Option<usize>,
    pub text: String,
}

/// A path read or written.
#[derive(Clone, Debug)]
pub struct PathEffect {
    /// The word (or redirection target) it comes from.
    pub span: Span,
    pub source: Option<usize>,
    /// The path, if literal.
    pub path: Option<String>,
    /// The index of the command in [`Classification::commands`]; `None` for a
    /// redirection of a compound command (`{ …; } > file`).
    pub command: Option<usize>,
}

/// Classifies a script. A script that does not parse is [`Category::Unknown`].
pub fn classify(source: &str) -> Classification {
    match psh_syntax::parse(source) {
        Ok(ast) => classify_ast(&ast),
        Err(e) => {
            let mut c = Classification::default();
            c.categories.insert(Category::Unknown);
            c.error = Some(e);
            c
        }
    }
}

/// Classifies a parsed script.
pub fn classify_ast(ast: &psh_syntax::Ast) -> Classification {
    walk::Walker::run(ast)
}

/// Variables whose value changes which program runs or what it loads: setting one makes
/// the command unknown.
pub(crate) fn dangerous_var(name: &str) -> bool {
    matches!(
        name,
        "PATH"
            | "BASH_ENV"
            | "ENV"
            | "SHELLOPTS"
            | "BASHOPTS"
            | "PROMPT_COMMAND"
            | "PS4"
            | "NODE_OPTIONS"
            | "PYTHONSTARTUP"
            | "PYTHONPATH"
            | "PERL5OPT"
            | "PERL5LIB"
            | "RUBYOPT"
            | "GIT_EXTERNAL_DIFF"
            | "GIT_SSH"
            | "GIT_SSH_COMMAND"
            | "GIT_PAGER"
            | "GIT_EDITOR"
            | "GIT_SEQUENCE_EDITOR"
            | "GIT_ASKPASS"
            | "GIT_EXEC_PATH"
            | "GIT_CONFIG_GLOBAL"
            | "GIT_CONFIG_PARAMETERS"
            | "GIT_CONFIG_COUNT"
            | "PAGER"
            | "EDITOR"
            | "VISUAL"
            | "RUSTC_WRAPPER"
            | "CARGO_BUILD_RUSTC_WRAPPER"
    ) || name.starts_with("LD_")
        || name.starts_with("DYLD_")
        || name.starts_with("BASH_FUNC_")
        || name.starts_with("GIT_CONFIG_KEY_")
        || name.starts_with("GIT_CONFIG_VALUE_")
}
