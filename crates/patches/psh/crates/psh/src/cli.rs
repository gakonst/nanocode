//! psh's own command line (D035, D1340). `argument` parses psh's modes (`--parse`,
//! `--classify`, `--utils`), `--help` and `--version`, and renders the help pages, usage lines and
//! errors. Bash's invocation (`-c`, `-euc`, `+o NAME`, `-O NAME`, `--`, the first operand
//! ending the options, bash's long options) is not expressible in `argument` (`+`
//! options, `-o` taking the next argument even inside `-eo`, `-c` as a flag whose command
//! is the first operand) and stays with `psh_engine::invocation`, which the conformance
//! runner shares: here `argument` reads only the leading `--name` arguments that are not
//! bash's and hands everything else over untouched.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use argument::{Cli, Error, Opt, Parser, Pos, ValueHint};
use psh_engine::invocation::LONG_OPTIONS;

/// What the command line asks for.
pub enum Mode {
    /// Run the shell: the arguments for `invocation::parse_args`, as bytes.
    Shell(Vec<Vec<u8>>),
    /// `--parse`
    Parse(Diag),
    /// `--classify`
    Classify(Diag),
    /// `--utils [--json]`
    Utils { json: bool },
}

/// The arguments of `--parse` and `--classify`.
pub struct Diag {
    pub source: Source,
    pub compact: bool,
    /// `--parse --tree`: the outline as text, not the JSON.
    pub tree: bool,
    pub options: psh_syntax::Options,
}

/// Where the script of `--parse` and `--classify` comes from.
pub enum Source {
    /// `-c COMMAND`
    Command(Vec<u8>),
    /// `FILE`
    File(PathBuf),
    /// Neither: stdin.
    Stdin,
}

#[derive(Copy, Clone)]
enum Top {
    Parse,
    Classify,
    Utils,
    /// The shell's options: listed in the help, never parsed here.
    Shell,
}

const USAGE: &str = "[OPTIONS] [FILE [ARGS]...]
       psh [OPTIONS] -c COMMAND [NAME [ARGS]...]
       psh --parse [--compact | --tree] [-O extglob] [-c COMMAND | FILE]
       psh --classify [--compact] [-O extglob] [-c COMMAND | FILE]
       psh --utils [--json]";

static PSH: Cli<Top> = Cli::new("psh")
    .version(env!("CARGO_PKG_VERSION"))
    .about(
        "psh is a portable, bash-compatible shell for the commands coding agents write. \
         It runs a script from FILE, from -c COMMAND or from stdin as bash does, and it \
         shows how it parses and classifies a script.",
    )
    .usage(USAGE)
    .opts(&[
        Opt::new(Top::Parse)
            .long("parse")
            .help("Print the syntax tree of a script as JSON (see 'psh --parse --help')"),
        Opt::new(Top::Classify)
            .long("classify")
            .help("Print what a script does as JSON (see 'psh --classify --help')"),
        Opt::new(Top::Utils)
            .long("utils")
            .help("List psh's own utilities and which of them run here (see 'psh --utils --help')"),
        Opt::section("Shell options (as bash)"),
        Opt::new(Top::Shell)
            .short('c')
            .help("Run COMMAND, the first argument after the options; NAME becomes $0, ARGS $1..."),
        Opt::new(Top::Shell)
            .short('s')
            .help("Read the script from stdin even with arguments; they become $1..."),
        Opt::new(Top::Shell)
            .short('e')
            .help("Exit when a command fails (set -e); +e turns it off, as + does for all"),
        Opt::new(Top::Shell)
            .short('o')
            .value("NAME")
            .help("Turn on the option NAME as 'set -o NAME' does (pipefail, nounset, ...)"),
        Opt::new(Top::Shell)
            .short('O')
            .value("NAME")
            .help("Turn on the shopt option NAME (extglob, nullglob, ...)"),
        // bash's `-h` is `set -h` (hashall) and `-V` is no option: neither may print the
        // help or the version, as `argument` would without these.
        Opt::new(Top::Shell).short('h').hidden(),
        Opt::new(Top::Shell).short('V').hidden(),
    ])
    .after_help(
        "The other letters of 'set' work as well (-abefhkmnptuvxBCEHPT), and options \
         combine as with bash: -euc, -eo pipefail, +O extglob. The first argument that is \
         not an option ends the options, as do -- and -. Without -c or FILE the script is \
         read from stdin. bash's long options --norc, --noprofile and --noediting are \
         accepted before all others and change nothing.",
    );

#[derive(Copy, Clone)]
enum D {
    Command,
    Compact,
    Tree,
    Shopt,
    File,
}

const COMMAND: Opt<D> = Opt::new(D::Command)
    .short('c')
    .value("COMMAND")
    .help("Read the script from COMMAND");
const COMPACT: Opt<D> = Opt::new(D::Compact)
    .long("compact")
    .help("Print the JSON on one line instead of indented");
const SHOPT: Opt<D> = Opt::new(D::Shopt)
    .short('O')
    .value("NAME")
    .possible_values(&["extglob"])
    .help("Parse with the shopt option NAME on");

/// The options of `--classify`; `--parse` has `--tree` too.
const DIAG_OPTS: [Opt<D>; 3] = [COMMAND, COMPACT, SHOPT];
const PARSE_OPTS: [Opt<D>; 4] = [
    COMMAND,
    COMPACT,
    Opt::new(D::Tree).long("tree").help(
        "Print an outline for people instead: one line per node, with its kind and \
         source text, wrappers of one child left out",
    ),
    SHOPT,
];

const DIAG_FILE: [Pos<D>; 1] = [Pos::new(D::File, "FILE")
    .optional()
    .value_hint(ValueHint::FilePath)
    .help("The script; stdin without FILE or -c")];

static PARSE: Cli<D> = Cli::new("--parse")
    .about(
        "Prints the syntax tree of a script as JSON: every node with its type, its byte \
         span in the input and its parts. Bytes that are not UTF-8 keep their offsets.",
    )
    .opts(&PARSE_OPTS)
    .args(&DIAG_FILE)
    .after_help(
        "Exits with 0 when the script parses and with 2 when it does not; the JSON then \
         holds the error with its span, line and column (with --tree, stderr has it).",
    );

static CLASSIFY: Cli<D> = Cli::new("--classify")
    .about(
        "Prints what a script does as JSON: every command it would run (also those in \
         $(...), bash -c '...' or behind timeout), the files each reads and writes, its \
         categories (read, mutate, build, network, ...) and whether the whole script only \
         reads. Whatever cannot be seen through is \"unknown\", never read-only.",
    )
    .opts(&DIAG_OPTS)
    .args(&DIAG_FILE)
    .after_help(
        "Exits with 0 when the script parses and with 2 when it does not (it is then \
         \"unknown\").",
    );

#[derive(Copy, Clone)]
enum U {
    Json,
}

static UTILS: Cli<U> = Cli::new("--utils")
    .about(
        "Lists the utilities psh runs in-process instead of a program (cat, head, rm, \
         timeout, ...): their names, their mode, whether they are destructive, and what a \
         command of that name runs here, with this PATH and PSH_UTILS_FORCE.",
    )
    .opts(&[Opt::new(U::Json)
        .long("json")
        .help("Print the list as JSON")])
    .after_help(
        "Modes: S runs in-process always; F only when PATH has no such program; W always \
         on Windows (where the system's programs of these names are others) and as F \
         elsewhere. PSH_UTILS_FORCE=NAME,... (or all) runs F and W utilities in-process \
         even when PATH has the program. RUNS is what a command of that name runs: psh \
         (the utility), program (the one PATH finds, under PATH) or builtin.",
    );

/// Parses the command line after the program name. Help, version and errors are
/// printed by `argument`'s `Error::exit` (status 0 for help and version, 2 for errors).
pub fn parse(args: Vec<OsString>) -> Result<Mode, Error> {
    let mut p = PSH.parser_from_args(args);
    let mut shell = Vec::new();
    loop {
        // Only `--name` arguments that are not bash's go to `argument`; the first other
        // argument (an option, `--`, an operand) starts bash's part.
        match p.peek_raw_arg() {
            Some(a) if is_long(a) => {
                let b = crate::os_bytes(a.to_owned());
                if LONG_OPTIONS.iter().any(|o| o.as_bytes() == b) {
                    p.raw_arg();
                    shell.push(b);
                    continue;
                }
            }
            _ => {
                while let Some(a) = p.raw_arg() {
                    shell.push(crate::os_bytes(a));
                }
                return Ok(Mode::Shell(shell));
            }
        }
        match p.param()? {
            Some(Top::Parse) => return diag(p.subcommand(&PARSE)).map(Mode::Parse),
            Some(Top::Classify) => return diag(p.subcommand(&CLASSIFY)).map(Mode::Classify),
            Some(Top::Utils) => return utils(p.subcommand(&UTILS)),
            // `-c`, `-o` and the rest never reach `param`: an argument not starting with
            // `--` ended the loop above.
            Some(Top::Shell) | None => unreachable!("bash's options are parsed by invocation"),
        }
    }
}

fn is_long(a: &OsStr) -> bool {
    let b = a.as_encoded_bytes();
    b.len() > 2 && b.starts_with(b"--")
}

fn utils(mut p: Parser<U>) -> Result<Mode, Error> {
    let mut json = false;
    while let Some(arg) = p.param()? {
        match arg {
            U::Json => json = true,
        }
    }
    Ok(Mode::Utils { json })
}

fn diag(mut p: Parser<D>) -> Result<Diag, Error> {
    let mut d = Diag {
        source: Source::Stdin,
        compact: false,
        tree: false,
        options: psh_syntax::Options::default(),
    };
    while let Some(arg) = p.param()? {
        let source = match arg {
            D::Compact => {
                d.compact = true;
                continue;
            }
            D::Tree => {
                d.tree = true;
                continue;
            }
            D::Shopt => {
                // `possible_values` admits only extglob.
                let _: String = p.value()?;
                d.options.extglob = true;
                continue;
            }
            D::Command => Source::Command(crate::os_bytes(p.raw_value()?)),
            D::File => Source::File(p.raw_value()?.into()),
        };
        if !matches!(d.source, Source::Stdin) {
            return Err(p.error("give the script once: -c COMMAND or FILE"));
        }
        d.source = source;
    }
    if d.compact && d.tree {
        return Err(p.error("--compact and --tree exclude each other"));
    }
    Ok(d)
}

/// The usage line and the hint at `--help` for the shell's own invocation errors.
pub fn usage_hint() -> String {
    let p = PSH.parser_from_args(Vec::<OsString>::new());
    format!(
        "Usage: {}\n\nFor more information, try 'psh --help'.",
        p.usage()
    )
}

/// The help page, for `--help` among bash's options (`psh -e --help`).
pub fn help_text() -> String {
    PSH.parser_from_args(Vec::<OsString>::new()).help_text()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Mode, Error> {
        parse(args.iter().map(OsString::from).collect())
    }

    fn shell(args: &[&str]) -> Vec<Vec<u8>> {
        match run(args) {
            Ok(Mode::Shell(a)) => a,
            _ => panic!("{args:?} is not a shell invocation"),
        }
    }

    #[test]
    fn bash_invocations_pass_through() {
        for args in [
            &["-c", "echo", "--parse"][..],
            &["-euc", "x"],
            &["+o", "errexit", "-O", "extglob", "f.sh", "--help"],
            &["--", "--parse"],
            &["-", "x"],
            &["--norc", "--noprofile", "-c", "x"],
            &["--posix"],
            &["-h", "-c", "x"],
            &[],
        ] {
            assert_eq!(
                shell(args),
                args.iter()
                    .map(|a| a.as_bytes().to_vec())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn modes() {
        let Ok(Mode::Parse(d)) = run(&["--norc", "--parse", "--compact", "-O", "extglob"]) else {
            panic!()
        };
        assert!(d.compact && d.options.extglob && matches!(d.source, Source::Stdin));
        let Ok(Mode::Classify(d)) = run(&["--classify", "-c", "ls"]) else {
            panic!()
        };
        assert!(!d.compact && matches!(d.source, Source::Command(c) if c == b"ls"));
        let Ok(Mode::Parse(d)) = run(&["--parse", "f.sh"]) else {
            panic!()
        };
        assert!(matches!(d.source, Source::File(f) if f == std::path::Path::new("f.sh")));
        assert!(matches!(run(&["--utils"]), Ok(Mode::Utils { json: false })));
        assert!(matches!(
            run(&["--utils", "--json"]),
            Ok(Mode::Utils { json: true })
        ));
    }

    #[test]
    fn errors() {
        use argument::ErrorKind;
        let kind = |args: &[&str]| run(args).err().map(|e| e.kind());
        assert_eq!(kind(&["--help"]), Some(ErrorKind::Help));
        assert_eq!(kind(&["--version"]), Some(ErrorKind::Version));
        assert_eq!(kind(&["--parse", "-h"]), Some(ErrorKind::Help));
        assert_eq!(kind(&["--pars"]), Some(ErrorKind::UnknownOption));
        assert_eq!(
            kind(&["--parse", "-O", "nullglob"]),
            Some(ErrorKind::InvalidValue)
        );
        assert_eq!(kind(&["--parse", "-c"]), Some(ErrorKind::MissingValue));
        assert!(kind(&["--parse", "-c", "x", "f.sh"]).is_some());
        assert!(kind(&["--classify", "a", "b"]).is_some());
        assert_eq!(kind(&["--utils", "x"]), Some(ErrorKind::UnexpectedArgument));
    }

    #[test]
    fn help_page_names_no_letter_for_help() {
        let help = help_text();
        assert!(help.contains("    --help"), "{help}");
        assert!(!help.contains("-h, --help"), "{help}");
        assert!(!help.contains("-V, --version"), "{help}");
    }
}
