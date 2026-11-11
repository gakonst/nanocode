//! The rules for one simple command: from its name and argument values to categories,
//! paths read and written, and the commands it runs (wrapped commands, `bash -c`
//! strings, `find -exec`).

use alloc::string::String;
use alloc::vec::Vec;

use crate::utils::{Effect, Operands, Utility, utility};
use crate::{Arg, Categories, Category, Via};

/// A path a command reads or writes: the argument it is in, and its literal value when
/// known (for `--output=FILE`, the part after `=`).
#[derive(Clone, Debug)]
pub(crate) struct PathRef {
    pub arg: usize,
    pub value: Option<String>,
}

/// A command run by this one.
#[derive(Clone, Debug)]
pub(crate) enum Nested {
    /// `args[start..end]` are a command (`timeout 5 cmd ...`, `find -exec cmd ... ;`).
    Words { start: usize, end: usize, via: Via },
    /// Shell code in `args[first..=last]`, already joined (`bash -c '...'`, `eval`).
    Source {
        first: usize,
        last: usize,
        text: String,
        via: Via,
    },
    /// A shell that reads its script from stdin; the walker looks at the redirections.
    Stdin,
}

#[derive(Debug, Default)]
pub(crate) struct Outcome {
    pub cats: Categories,
    pub reads: Vec<PathRef>,
    pub writes: Vec<PathRef>,
    pub nested: Vec<Nested>,
    pub reason: Option<&'static str>,
}

impl Outcome {
    fn of(c: Category) -> Outcome {
        Outcome {
            cats: c.into(),
            ..Outcome::default()
        }
    }

    fn unknown(reason: &'static str) -> Outcome {
        Outcome {
            reason: Some(reason),
            ..Outcome::of(Category::Unknown)
        }
    }

    fn add(mut self, c: Category) -> Outcome {
        self.cats.insert(c);
        self
    }

    fn set_unknown(&mut self, reason: &'static str) {
        self.cats.insert(Category::Unknown);
        self.reason.get_or_insert(reason);
    }
}

/// Paths that are not files: writing to them changes nothing.
pub(crate) fn harmless_path(p: &str) -> bool {
    matches!(
        p,
        "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/stdin" | "/dev/tty" | "-"
    ) || p
        .strip_prefix("/dev/fd/")
        .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
}

fn val(args: &[Arg], i: usize) -> Option<&str> {
    args.get(i).and_then(|a| a.value.as_deref())
}

/// Directories whose programs are taken by their base name (`/usr/bin/grep` is `grep`).
const SYSTEM_DIRS: &[&str] = &[
    "/bin/",
    "/usr/bin/",
    "/usr/local/bin/",
    "/opt/homebrew/bin/",
    "/sbin/",
    "/usr/sbin/",
];

/// The name a command is looked up by, or `None` for a program given by a path that is
/// not a system directory (`./x`, `target/debug/app`).
pub(crate) fn command_name(word: &str) -> Option<&str> {
    if !word.contains('/') {
        return Some(word);
    }
    for d in SYSTEM_DIRS {
        if let Some(base) = word.strip_prefix(d)
            && !base.is_empty()
            && !base.contains('/')
        {
            return Some(base);
        }
    }
    None
}

pub(crate) const SHELLS: &[&str] = &["bash", "sh", "zsh", "dash", "ksh", "psh"];
const PYTHONS: &[&str] = &[
    "python",
    "python3",
    "python3.10",
    "python3.11",
    "python3.12",
    "python3.13",
    "python3.14",
];
const INTERPRETERS: &[&str] = &[
    "node",
    "deno",
    "bun",
    "perl",
    "ruby",
    "php",
    "lua",
    "Rscript",
    "julia",
    "tsx",
    "ts-node",
    "osascript",
    "swift",
    "irb",
    "pwsh",
    "powershell",
    "cmd",
    "cmd.exe",
];

/// Classifies a simple command whose name (`args[0]`) is literal.
pub(crate) fn classify(name: &str, args: &[Arg]) -> Outcome {
    match name {
        "export" | "declare" | "typeset" | "local" | "readonly" if sets_dangerous(args) => {
            Outcome::unknown("sets a variable that changes what runs")
        }
        // The shell's own builtins.
        "cd" | "pushd" | "popd" | "dirs" | ":" | "true" | "false" | "echo" | "printf" | "test"
        | "[" | "[[" | "exit" | "return" | "break" | "continue" | "shift" | "wait" | "jobs"
        | "times" | "getopts" | "let" | "read" | "mapfile" | "readarray" | "local" | "declare"
        | "typeset" | "export" | "readonly" | "unset" | "set" | "shopt" | "umask" | "ulimit"
        | "caller" | "compgen" | "complete" | "compopt" | "help" | "type" | "hash" | "disown"
        | "bg" | "fg" | "unalias" | "history" | "logout" | "suspend" => Outcome::of(Category::Read),
        "alias" => alias(args),
        "source" | "." => Outcome::unknown("runs a script file"),
        "enable" => Outcome::unknown("enable can load builtins"),
        "fc" => Outcome::unknown("fc runs commands from history"),
        "eval" => eval(args),
        "trap" => trap(args),
        "exec" => {
            wrapper(args, 1, "cl", "a", Via::Wrapper).unwrap_or_else(|| Outcome::of(Category::Read))
        }
        "command" => command(args),
        "builtin" => wrap_at(args, 1, Via::Wrapper),
        "env" => env(args),
        "timeout" | "gtimeout" => timeout(args),
        "nice" => nice(args),
        "nohup" => wrap_at(args, 1, Via::Wrapper).add(Category::LongRunning),
        "stdbuf" => wrapper(args, 1, "", "ioe", Via::Wrapper)
            .unwrap_or_else(|| Outcome::unknown("stdbuf without a command")),
        "caffeinate" => wrapper(args, 1, "dimsu", "tw", Via::Wrapper)
            .unwrap_or_else(|| Outcome::of(Category::LongRunning)),
        "time" => wrapper(args, 1, "pvl", "fo", Via::Wrapper)
            .unwrap_or_else(|| Outcome::of(Category::Read)),
        "sudo" | "doas" => {
            let mut o = wrapper(args, 1, "AbEHiKklnPSsv", "CDghpRrTUu", Via::Wrapper)
                .unwrap_or_else(|| Outcome::of(Category::Read));
            o.set_unknown("runs as another user");
            o
        }
        "xargs" => xargs(args),
        "find" => find(args),
        "git" => git(args),
        "gh" => gh(args),
        "jj" => jj(args),
        "npm" | "pnpm" | "yarn" | "bun" if name != "bun" || !bun_runs_file(args) => {
            js_package_manager(name, args)
        }
        "npx" | "pnpx" | "bunx" | "uvx" => Outcome::of(Category::Build),
        "cargo" => cargo(args),
        "go" => go(args),
        "uv" => uv(args),
        "pip" | "pip3" => pip(&args[1..]),
        "brew" | "apt" | "apt-get" | "port" | "gem" | "rustup" | "pipx" | "nvm" | "pyenv"
        | "asdf" | "mise" | "conda" | "dnf" | "yum" | "pacman" | "apk" | "winget" | "choco"
        | "scoop" => system_package_manager(name, args),
        "docker" | "podman" => docker(args),
        "tmux" => tmux(args),
        "launchctl" | "systemctl" | "service" => service(args),
        "ssh" | "mosh" => {
            Outcome::unknown("runs commands on another machine").add(Category::Network)
        }
        "rsync" => rsync(args),
        "tar" | "bsdtar" | "gtar" => tar(args),
        "unzip" => unzip(args),
        "gzip" | "gunzip" | "bzip2" | "bunzip2" | "xz" | "unxz" | "zstd" | "brotli"
            if compressor_reads(args) =>
        {
            let mut o = Outcome::of(Category::Read);
            let files =
                (1..args.len()).filter(|&i| val(args, i).is_some_and(|v| !v.starts_with('-')));
            o.reads.extend(files.map(|i| path(args, i)));
            o
        }
        "sqlite3" => {
            let vals: Vec<Option<&str>> = args[1..].iter().map(|a| a.value.as_deref()).collect();
            match crate::sqlite::classify(&vals) {
                Some((c, file)) => {
                    let mut o = Outcome::of(c);
                    if let Some(f) = file {
                        let p = path(args, f + 1);
                        if c == Category::Mutate {
                            o.writes.push(p);
                        } else {
                            o.reads.push(p);
                        }
                    }
                    o
                }
                None => Outcome::unknown(
                    "sqlite3 runs SQL that is not literal, or that can run programs or write other files",
                ),
            }
        }
        "sips" => {
            let gets = |v: &str| {
                matches!(
                    v,
                    "-g" | "--getProperty" | "-1" | "--oneLine" | "-h" | "--help"
                )
            };
            let flags = args[1..]
                .iter()
                .filter_map(|a| a.value.as_deref())
                .filter(|v| v.starts_with('-'));
            if args[1..].iter().all(|a| a.value.is_some()) && flags.clone().all(gets) {
                Outcome::of(Category::Read)
            } else {
                Outcome::of(Category::Mutate)
            }
        }
        "plutil" => {
            let reads =
                |v: &str| matches!(v, "-p" | "-lint" | "-help" | "-type" | "-extract" | "-s");
            let first = args.get(1).and_then(|a| a.value.as_deref());
            match first {
                Some(f)
                    if reads(f) && !args.iter().any(|a| a.value.as_deref() == Some("-o"))
                        || (f == "-extract"
                            && args.windows(2).any(|w| {
                                w[0].value.as_deref() == Some("-o")
                                    && w[1].value.as_deref() == Some("-")
                            })) =>
                {
                    Outcome::of(Category::Read)
                }
                _ => Outcome::of(Category::Mutate),
            }
        }
        "open" | "xdg-open" => Outcome::unknown("opens a file with another program"),
        "vim" | "vi" | "nvim" | "nano" | "emacs" | "code" | "subl" => {
            Outcome::unknown("starts an editor")
        }
        _ if SHELLS.contains(&name) => shell(args),
        _ if PYTHONS.contains(&name) => python(args),
        _ if INTERPRETERS.contains(&name) || name == "bun" => interpreter(name, args),
        _ => match utility(name) {
            Some(u) => table(name, u, args, 1),
            None => Outcome::unknown("unknown program"),
        },
    }
}

/// The result of reading options against a [`Utility`].
#[derive(Default)]
struct Scan {
    operands: Vec<usize>,
    effects: Vec<(Effect, Option<PathRef>)>,
    /// A non-literal word where an option could be: the effect is not knowable.
    dynamic: bool,
    /// A short or long option gave the script or pattern (`grep -e`, `sed -e`).
    script_given: bool,
}

fn scan(u: &Utility, args: &[Arg], start: usize) -> Scan {
    let mut s = Scan::default();
    let script_opts = match u.operands {
        Operands::Script(o) => o,
        _ => "",
    };
    let mut ended = false;
    let mut i = start;
    while i < args.len() {
        let Some(a) = args[i].value.as_deref() else {
            if !ended && args[i].may_be_option() {
                s.dynamic = true;
            }
            s.operands.push(i);
            i += 1;
            continue;
        };
        if ended || a == "-" || !a.starts_with('-') || a.len() == 1 {
            s.operands.push(i);
        } else if a == "--" {
            ended = true;
        } else if let Some(long) = a.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (long, None),
            };
            let takes = u.long_values.iter().any(|l| &l[2..] == name);
            let value = match inline {
                Some(v) => Some(PathRef {
                    arg: i,
                    value: Some(String::from(v)),
                }),
                None if takes => {
                    i += 1;
                    (i < args.len()).then(|| PathRef {
                        arg: i,
                        value: args[i].value.clone(),
                    })
                }
                None => None,
            };
            if matches!(name, "regexp" | "expression" | "file") && !script_opts.is_empty() {
                s.script_given = true;
            }
            for (opt, e) in u.effects {
                if opt.strip_prefix("--") == Some(name) {
                    s.effects.push((*e, value.clone()));
                }
            }
        } else {
            let cluster = &a[1..];
            for (k, c) in cluster.char_indices() {
                let takes = u.short_values.contains(c);
                let value = if takes {
                    let rest = &cluster[k + c.len_utf8()..];
                    if rest.is_empty() {
                        i += 1;
                        (i < args.len()).then(|| PathRef {
                            arg: i,
                            value: args[i].value.clone(),
                        })
                    } else {
                        Some(PathRef {
                            arg: i,
                            value: Some(String::from(rest)),
                        })
                    }
                } else {
                    None
                };
                if script_opts.contains(c) {
                    s.script_given = true;
                }
                for (opt, e) in u.effects {
                    if !opt.starts_with("--") && opt[1..].starts_with(c) {
                        s.effects.push((*e, value.clone()));
                    }
                }
                if takes {
                    break;
                }
            }
        }
        i += 1;
    }
    s
}

fn path(args: &[Arg], i: usize) -> PathRef {
    PathRef {
        arg: i,
        value: args[i].value.clone(),
    }
}

/// A utility described by the table.
fn table(name: &str, u: &Utility, args: &[Arg], start: usize) -> Outcome {
    let mut s = scan(u, args, start);
    let mut o = Outcome::of(u.category);
    // A dynamic word can turn into an option (`rg "$x"` with x=`--pre=sh`): it matters
    // only where some option writes, mutates or runs code.
    let risky = u.effects.iter().any(|(_, e)| {
        matches!(
            e,
            Effect::WritesOperands | Effect::WritesValue | Effect::Mutates | Effect::Unknown
        )
    });
    if s.dynamic && risky {
        o.set_unknown("a dynamic word could be an option that changes the effect");
    }
    let mut writes_operands = false;
    for (e, v) in &s.effects {
        match e {
            Effect::WritesOperands => writes_operands = true,
            Effect::WritesValue => {
                let value = v.as_ref().and_then(|v| v.value.as_deref());
                if !value.is_some_and(harmless_path) {
                    o.cats.insert(Category::Mutate);
                }
                if let Some(v) = v {
                    o.writes.push(v.clone());
                }
            }
            Effect::ReadsValue => o.reads.extend(v.clone()),
            Effect::Mutates => o.cats.insert(Category::Mutate),
            Effect::LongRunning => o.cats.insert(Category::LongRunning),
            Effect::Unknown => o.set_unknown("an option runs or loads code"),
        }
    }
    // The script or pattern operand of grep, sed, awk.
    if let Operands::Script(_) = u.operands
        && !s.script_given
        && !s.operands.is_empty()
    {
        let script = s.operands.remove(0);
        // BSD `sed -i '' script file`: the empty word is the backup suffix.
        if writes_operands && val(args, script) == Some("") && !s.operands.is_empty() {
            let script = s.operands.remove(0);
            script_check(name, args, script, &mut o);
        } else {
            script_check(name, args, script, &mut o);
        }
    } else if s.script_given && matches!(name, "sed") {
        // -e scripts: check each one.
        for (i, a) in args.iter().enumerate().skip(start) {
            if matches!(a.value.as_deref(), Some("-e" | "--expression")) && i + 1 < args.len() {
                script_check(name, args, i + 1, &mut o);
            }
        }
    }
    if name == "tee"
        && s.operands
            .iter()
            .all(|&i| val(args, i).is_some_and(harmless_path))
    {
        o.cats = Category::Read.into();
    }
    if writes_operands {
        o.cats.insert(Category::Mutate);
        o.writes.extend(s.operands.iter().map(|&i| path(args, i)));
        return o;
    }
    match u.operands {
        Operands::None => {}
        Operands::Read | Operands::Script(_) => {
            o.reads.extend(s.operands.iter().map(|&i| path(args, i)))
        }
        Operands::Write | Operands::AllWrite => {
            o.writes.extend(s.operands.iter().map(|&i| path(args, i)))
        }
        Operands::LastWrite => {
            if let Some((&last, rest)) = s.operands.split_last() {
                o.reads.extend(rest.iter().map(|&i| path(args, i)));
                o.writes.push(path(args, last));
            }
        }
        Operands::ReadThenWrite => {
            for (k, &i) in s.operands.iter().enumerate() {
                if k == 0 {
                    o.reads.push(path(args, i));
                } else {
                    if !val(args, i).is_some_and(harmless_path) {
                        o.cats.insert(Category::Mutate);
                    }
                    o.writes.push(path(args, i));
                }
            }
        }
    }
    o
}

/// Scripts that can write or run commands: sed's `w` and `e`, awk's `system` and
/// output redirections.
fn script_check(name: &str, args: &[Arg], i: usize, o: &mut Outcome) {
    let Some(script) = val(args, i) else {
        if matches!(name, "sed" | "awk" | "gawk" | "mawk" | "nawk") {
            o.set_unknown("the script is not literal");
        }
        return;
    };
    match name {
        "sed" => match crate::sed::effects(script) {
            Some(e) => {
                if e.writes {
                    o.cats.insert(Category::Mutate);
                }
                if e.runs {
                    o.set_unknown("the sed script runs commands");
                }
            }
            None => o.set_unknown("the sed script did not parse"),
        },
        "awk" | "gawk" | "mawk" | "nawk" if awk_has_effects(script) => {
            o.set_unknown("the awk program runs commands or writes files");
        }
        _ => {}
    }
}

/// An awk program that may run commands or write files: `system(...)`, `cmd | getline`,
/// `print … | "cmd"`, or `print … > "file"`. String and regex literals are skipped, and a
/// `>` inside parentheses (`print ($1 > 5)`) or outside a print statement is a
/// comparison.
fn awk_has_effects(p: &str) -> bool {
    let code = awk_code(p);
    if code.contains("system") || code.contains("fflush") {
        return true;
    }
    let b = code.as_bytes();
    for (k, &c) in b.iter().enumerate() {
        if c == b'|' && b.get(k + 1) != Some(&b'|') && (k == 0 || b[k - 1] != b'|') {
            return true;
        }
    }
    code.split([';', '{', '}', '\n']).any(|stmt| {
        let Some(at) = stmt.find("print") else {
            return false;
        };
        let mut depth = 0i32;
        stmt[at..].bytes().any(|c| {
            match c {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            c == b'>' && depth <= 0
        })
    })
}

/// The program without its string and regex literals (each replaced by a space).
fn awk_code(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    let mut chars = p.chars().peekable();
    // A `/` starts a regex where an operand is expected.
    let mut operand_expected = true;
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                while let Some(d) = chars.next() {
                    match d {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => {}
                    }
                }
                out.push(' ');
                operand_expected = false;
            }
            '/' if operand_expected => {
                let mut in_bracket = false;
                while let Some(d) = chars.next() {
                    match d {
                        '\\' => {
                            chars.next();
                        }
                        '[' => in_bracket = true,
                        ']' => in_bracket = false,
                        '/' if !in_bracket => break,
                        _ => {}
                    }
                }
                out.push(' ');
                operand_expected = false;
            }
            _ => {
                out.push(c);
                if !c.is_whitespace() {
                    operand_expected = "(,~!&|{};=<>?:+-*%^".contains(c) || c == '\n';
                }
            }
        }
    }
    out
}

fn alias(args: &[Arg]) -> Outcome {
    let mut o = Outcome::of(Category::Read);
    for (i, a) in args.iter().enumerate().skip(1) {
        match a.value.as_deref() {
            Some(v) => {
                if let Some((_, body)) = v.split_once('=') {
                    o.nested.push(Nested::Source {
                        first: i,
                        last: i,
                        text: String::from(body),
                        via: Via::Alias,
                    });
                }
            }
            None => o.set_unknown("an alias defined from a dynamic word"),
        }
    }
    o
}

fn eval(args: &[Arg]) -> Outcome {
    if args.len() == 1 {
        return Outcome::of(Category::Read);
    }
    let mut text = String::new();
    for (i, a) in args.iter().enumerate().skip(1) {
        let Some(v) = a.value.as_deref() else {
            return Outcome::unknown("eval of a dynamic string");
        };
        if i > 1 {
            text.push(' ');
        }
        text.push_str(v);
    }
    let mut o = Outcome::of(Category::Read);
    o.nested.push(Nested::Source {
        first: 1,
        last: args.len() - 1,
        text,
        via: Via::Eval,
    });
    o
}

fn trap(args: &[Arg]) -> Outcome {
    let mut o = Outcome::of(Category::Read);
    let Some(first) = args.get(1) else { return o };
    match first.value.as_deref() {
        Some("-p" | "-l" | "-" | "--") | Some("") => o,
        // `trap 2` style signal-only reset, or `trap -- 'x' EXIT` handled loosely.
        Some(code) if args.len() > 2 => {
            o.nested.push(Nested::Source {
                first: 1,
                last: 1,
                text: String::from(code),
                via: Via::Trap,
            });
            o
        }
        Some(_) => o,
        None => {
            o.set_unknown("a trap handler from a dynamic word");
            o
        }
    }
}

/// A wrapper: skips its options (`flags` take no value, `valued` take one) and returns
/// the wrapped command starting at the first operand, or `None` if there is none.
fn wrapper(args: &[Arg], start: usize, flags: &str, valued: &str, via: Via) -> Option<Outcome> {
    let mut i = start;
    while i < args.len() {
        let Some(a) = args[i].value.as_deref() else {
            return Some(Outcome::unknown("a dynamic word in a wrapper's options"));
        };
        if a == "--" {
            i += 1;
            break;
        }
        if !a.starts_with('-') || a.len() == 1 {
            break;
        }
        let cluster = &a[1..];
        if cluster.starts_with('-') {
            // A long option: `--opt=value` or a flag.
            i += 1;
            continue;
        }
        for (k, c) in cluster.char_indices() {
            if valued.contains(c) {
                if k + 1 == cluster.len() {
                    i += 1;
                }
                break;
            }
            if !flags.contains(c) && !flags.is_empty() {
                // An option this table does not know: it may take a value.
                return Some(Outcome::unknown("an unknown option of a wrapper"));
            }
        }
        i += 1;
    }
    (i < args.len()).then(|| wrap_at(args, i, via))
}

/// The command at `args[start..]` runs as a nested command.
fn wrap_at(args: &[Arg], start: usize, via: Via) -> Outcome {
    let mut o = Outcome::of(Category::Read);
    if start < args.len() {
        o.nested.push(Nested::Words {
            start,
            end: args.len(),
            via,
        });
    }
    o
}

fn command(args: &[Arg]) -> Outcome {
    match val(args, 1) {
        Some("-v" | "-V") => Outcome::of(Category::Read),
        Some("-p") => wrap_at(args, 2, Via::Wrapper),
        Some(_) => wrap_at(args, 1, Via::Wrapper),
        None if args.len() == 1 => Outcome::of(Category::Read),
        None => Outcome::unknown("a dynamic command name"),
    }
}

fn env(args: &[Arg]) -> Outcome {
    let mut i = 1;
    while i < args.len() {
        let Some(a) = args[i].value.as_deref() else {
            return Outcome::unknown("a dynamic word in env's arguments");
        };
        match a {
            "-u" | "--unset" | "-C" | "--chdir" => i += 2,
            "-S" | "--split-string" => {
                return Outcome::unknown("env -S splits a string into a command");
            }
            "--" => {
                i += 1;
                break;
            }
            _ if a.starts_with('-') && a.len() > 1 => i += 1,
            _ if a.contains('=') => {
                let name = &a[..a.find('=').unwrap_or(0)];
                if crate::dangerous_var(name) {
                    return Outcome::unknown("sets a variable that changes what runs");
                }
                i += 1;
            }
            _ => break,
        }
    }
    if i >= args.len() {
        return Outcome::of(Category::Read);
    }
    wrap_at(args, i, Via::Wrapper)
}

fn timeout(args: &[Arg]) -> Outcome {
    let mut i = 1;
    while i < args.len() {
        let Some(a) = args[i].value.as_deref() else {
            return Outcome::unknown("a dynamic word in timeout's arguments");
        };
        match a {
            "-s" | "--signal" | "-k" | "--kill-after" => i += 2,
            _ if a.starts_with('-') && a.len() > 1 => i += 1,
            _ => break,
        }
    }
    // The duration, then the command.
    wrap_at(args, i + 1, Via::Wrapper)
}

fn nice(args: &[Arg]) -> Outcome {
    let mut i = 1;
    while let Some(a) = val(args, i) {
        match a {
            "-n" | "--adjustment" => i += 2,
            _ if a.starts_with('-') && a.len() > 1 => i += 1,
            _ => break,
        }
    }
    if i < args.len() && args[i].value.is_none() {
        return Outcome::unknown("a dynamic command name");
    }
    wrap_at(args, i, Via::Wrapper)
}

fn xargs(args: &[Arg]) -> Outcome {
    let mut i = 1;
    while i < args.len() {
        let Some(a) = args[i].value.as_deref() else {
            return Outcome::unknown("a dynamic word in xargs's options");
        };
        if !a.starts_with('-') || a == "-" {
            break;
        }
        if a == "--" {
            i += 1;
            break;
        }
        // Options with a separate value; `-I{}`, `-n1` are attached.
        if matches!(
            a,
            "-I" | "-n" | "-P" | "-L" | "-d" | "-E" | "-s" | "-a" | "-R" | "-S"
        ) {
            i += 1;
        }
        i += 1;
    }
    if i >= args.len() {
        // The default command is echo.
        return Outcome::of(Category::Read);
    }
    wrap_at(args, i, Via::Xargs)
}

fn find(args: &[Arg]) -> Outcome {
    let mut o = Outcome::of(Category::Read);
    let mut i = 1;
    let mut in_expr = false;
    while i < args.len() {
        let Some(a) = args[i].value.as_deref() else {
            // Even a path from a variable can start with `-` and be read as an action.
            if args[i].may_be_option() || in_expr {
                o.set_unknown("a dynamic word in find's arguments");
            }
            i += 1;
            continue;
        };
        if a.starts_with('-') || a == "(" || a == "!" || a == ")" {
            in_expr = true;
        }
        match a {
            "-delete" => o.cats.insert(Category::Mutate),
            "-exec" | "-execdir" | "-ok" | "-okdir" => {
                let start = i + 1;
                let mut end = start;
                while end < args.len() && !matches!(val(args, end), Some(";" | "+")) {
                    end += 1;
                }
                if end > start {
                    o.nested.push(Nested::Words {
                        start,
                        end,
                        via: Via::FindExec,
                    });
                }
                i = end;
            }
            "-fprint" | "-fprint0" | "-fls" | "-fprintf" => {
                o.cats.insert(Category::Mutate);
                if i + 1 < args.len() {
                    o.writes.push(path(args, i + 1));
                }
                i += if a == "-fprintf" { 2 } else { 1 };
            }
            _ if !in_expr => o.reads.push(path(args, i)),
            _ => {}
        }
        i += 1;
    }
    o
}

/// The first operand after the global options, skipping `skip` options with a value.
fn subcommand(args: &[Arg], valued: &[&str]) -> (Option<usize>, bool) {
    let mut i = 1;
    while i < args.len() {
        let Some(a) = args[i].value.as_deref() else {
            return (Some(i), true);
        };
        if valued.contains(&a) {
            i += 2;
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        return (Some(i), false);
    }
    (None, false)
}

const GIT_READ: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "rev-parse",
    "blame",
    "grep",
    "ls-files",
    "ls-tree",
    "describe",
    "shortlog",
    "reflog",
    "cat-file",
    "rev-list",
    "merge-base",
    "name-rev",
    "whatchanged",
    "for-each-ref",
    "show-ref",
    "help",
    "version",
    "check-ignore",
    "count-objects",
    "fsck",
    "verify-commit",
    "range-diff",
    "diff-tree",
    "diff-index",
    "diff-files",
    "show-branch",
    "var",
    "check-attr",
    "hash-object",
    "annotate",
    "cherry",
    "grep",
    "archive",
    "bugreport",
    "check-ref-format",
    "ls-remote",
];
const GIT_MUTATE: &[&str] = &[
    "add",
    "commit",
    "push",
    "checkout",
    "switch",
    "reset",
    "rebase",
    "merge",
    "cherry-pick",
    "restore",
    "rm",
    "mv",
    "init",
    "clone",
    "fetch",
    "pull",
    "am",
    "apply",
    "revert",
    "clean",
    "gc",
    "update-index",
    "update-ref",
    "commit-tree",
    "read-tree",
    "write-tree",
    "format-patch",
    "filter-branch",
    "filter-repo",
    "lfs",
    "prune",
    "repack",
    "sparse-checkout",
    "maintenance",
    "mktree",
    "mktag",
    "replace",
    "rerere",
    "symbolic-ref",
];
const GIT_NETWORK: &[&str] = &[
    "clone",
    "fetch",
    "pull",
    "push",
    "ls-remote",
    "submodule",
    "lfs",
];

fn git(args: &[Arg]) -> Outcome {
    // Global options: `-c` sets config, which can name commands to run.
    for a in &args[1..] {
        match a.value.as_deref() {
            Some("-c" | "--config-env" | "--exec-path") => {
                return Outcome::unknown("git -c can configure commands to run");
            }
            Some(v) if v.starts_with("--config-env=") || v.starts_with("--exec-path=") => {
                return Outcome::unknown("git -c can configure commands to run");
            }
            Some(v) if !v.starts_with('-') => break,
            _ => {}
        }
    }
    let (sub, dynamic) = subcommand(args, &["-C", "--git-dir", "--work-tree", "--namespace"]);
    let Some(si) = sub else {
        return Outcome::of(Category::Read);
    };
    if dynamic {
        return Outcome::unknown("a dynamic git subcommand");
    }
    let sub = val(args, si).unwrap_or("");
    let rest: Vec<Option<&str>> = args[si + 1..].iter().map(|a| a.value.as_deref()).collect();
    let has = |opts: &[&str]| rest.iter().any(|r| r.is_some_and(|r| opts.contains(&r)));
    let operands = rest
        .iter()
        .filter(|r| r.is_none_or(|r| !r.starts_with('-')))
        .count();
    let mut o = if GIT_MUTATE.contains(&sub) {
        Outcome::of(Category::Vcs)
    } else if GIT_READ.contains(&sub) {
        let mut o = Outcome::of(Category::Read);
        if sub == "diff"
            && rest
                .iter()
                .any(|r| r.is_some_and(|r| r.starts_with("--output")))
        {
            o.cats.insert(Category::Mutate);
        }
        if sub == "archive" && has(&["-o", "--output"]) {
            o.cats.insert(Category::Mutate);
        }
        o
    } else {
        match sub {
            "stash" => match rest.first() {
                Some(Some("list" | "show")) => Outcome::of(Category::Read),
                _ => Outcome::of(Category::Vcs),
            },
            "branch" => {
                if has(&[
                    "-d",
                    "-D",
                    "-m",
                    "-M",
                    "-c",
                    "-C",
                    "--delete",
                    "--move",
                    "--copy",
                    "-f",
                    "--force",
                    "-u",
                    "--set-upstream-to",
                    "--unset-upstream",
                    "--edit-description",
                ]) || (operands > 0
                    && !has(&[
                        "-l",
                        "--list",
                        "--contains",
                        "--merged",
                        "--no-merged",
                        "--points-at",
                        "--show-current",
                        "-a",
                        "-r",
                        "-v",
                        "-vv",
                    ]))
                {
                    Outcome::of(Category::Vcs)
                } else {
                    Outcome::of(Category::Read)
                }
            }
            "tag" => {
                if has(&[
                    "-l",
                    "--list",
                    "--contains",
                    "--points-at",
                    "--merged",
                    "--no-merged",
                    "-n",
                ]) || operands == 0
                {
                    Outcome::of(Category::Read)
                } else {
                    Outcome::of(Category::Vcs)
                }
            }
            "remote" => match rest.iter().find(|r| r.is_none_or(|r| !r.starts_with('-'))) {
                None | Some(Some("show" | "get-url")) => Outcome::of(Category::Read),
                Some(Some("update")) => Outcome::of(Category::Vcs).add(Category::Network),
                _ => Outcome::of(Category::Vcs),
            },
            "config" => {
                let reading = has(&[
                    "--get",
                    "--get-all",
                    "--get-regexp",
                    "-l",
                    "--list",
                    "--show-origin",
                    "--show-scope",
                ]) || rest
                    .first()
                    .is_some_and(|r| matches!(r, Some("get" | "list")));
                if reading
                    || (operands <= 1
                        && !has(&[
                            "--unset",
                            "--unset-all",
                            "--add",
                            "--replace-all",
                            "--remove-section",
                            "--rename-section",
                            "-e",
                            "--edit",
                        ]))
                {
                    Outcome::of(Category::Read)
                } else {
                    Outcome::of(Category::Vcs)
                }
            }
            "worktree" => match rest.first() {
                Some(Some("list")) => Outcome::of(Category::Read),
                _ => Outcome::of(Category::Vcs),
            },
            "submodule" => match rest.first() {
                Some(Some("foreach")) => Outcome::unknown("git submodule foreach runs commands"),
                None | Some(Some("status" | "summary")) => Outcome::of(Category::Read),
                _ => Outcome::of(Category::Vcs),
            },
            "notes" => match rest.first() {
                None | Some(Some("list" | "show")) => Outcome::of(Category::Read),
                _ => Outcome::of(Category::Vcs),
            },
            "bisect" => match rest.first() {
                Some(Some("log" | "visualize" | "view")) => Outcome::of(Category::Read),
                Some(Some("run")) => Outcome::unknown("git bisect run runs commands"),
                _ => Outcome::of(Category::Vcs),
            },
            "difftool" | "mergetool" | "instaweb" | "send-email" | "daemon" | "credential"
            | "filter-process" => Outcome::unknown("runs other programs"),
            // Aliases and plugins (`git foo` runs `git-foo`) can run anything.
            _ => Outcome::unknown("an unknown git subcommand (an alias or plugin)"),
        }
    };
    if GIT_NETWORK.contains(&sub) && !o.cats.contains(Category::Read) {
        o.cats.insert(Category::Network);
    }
    if sub == "ls-remote" {
        o.cats.insert(Category::Network);
    }
    o
}

fn gh(args: &[Arg]) -> Outcome {
    let mut o = Outcome::of(Category::Network);
    let (sub, dynamic) = subcommand(args, &["-R", "--repo"]);
    if dynamic {
        o.set_unknown("a dynamic gh subcommand");
        return o;
    }
    let Some(si) = sub else { return o };
    let sub = val(args, si).unwrap_or("");
    let action = args[si + 1..]
        .iter()
        .find(|a| a.value.as_deref().is_none_or(|v| !v.starts_with('-')))
        .map(|a| a.value.as_deref());
    let rest: Vec<&str> = args[si + 1..]
        .iter()
        .filter_map(|a| a.value.as_deref())
        .collect();
    match sub {
        "api" => {
            let method = rest
                .windows(2)
                .find(|w| matches!(w[0], "-X" | "--method"))
                .map(|w| w[1])
                .or_else(|| {
                    rest.iter()
                        .find_map(|r| r.strip_prefix("-X").filter(|m| !m.is_empty()))
                });
            let fields = rest.iter().any(|r| {
                matches!(*r, "-f" | "-F" | "--field" | "--raw-field" | "--input")
                    || r.starts_with("--field=")
                    || r.starts_with("--raw-field=")
            });
            if method.is_some_and(|m| !m.eq_ignore_ascii_case("GET")) || fields {
                o.cats.insert(Category::Vcs);
            }
        }
        "pr" | "issue" | "release" | "repo" | "gist" | "label" | "secret" | "variable"
        | "workflow" | "run" | "project" | "ruleset" | "cache" => {
            let reading = matches!(
                action,
                Some(Some(
                    "view"
                        | "list"
                        | "status"
                        | "diff"
                        | "checks"
                        | "ls"
                        | "download"
                        | "watch"
                        | "get"
                ))
            ) || action.is_none();
            if !reading {
                o.cats.insert(Category::Vcs);
            }
            if matches!(action, Some(Some("checkout" | "clone" | "download"))) {
                o.cats.insert(Category::Mutate);
            }
            if matches!(action, Some(Some("watch"))) {
                o.cats.insert(Category::LongRunning);
            }
        }
        "auth" | "config" | "extension" | "alias" => {
            if !matches!(action, Some(Some("status" | "list" | "get" | "token"))) {
                o.set_unknown("changes gh's setup");
            }
        }
        "search" | "browse" | "status" | "help" | "version" | "completion" => {}
        _ => o.set_unknown("an unknown gh subcommand (an alias or extension)"),
    }
    o
}

fn jj(args: &[Arg]) -> Outcome {
    let (sub, dynamic) = subcommand(args, &["-R", "--repository", "--at-op", "--config"]);
    if dynamic {
        return Outcome::unknown("a dynamic jj subcommand");
    }
    match sub.and_then(|i| val(args, i)) {
        None
        | Some(
            "log" | "status" | "st" | "diff" | "show" | "op" | "help" | "version" | "root" | "file"
            | "evolog" | "interdiff",
        ) => Outcome::of(Category::Read),
        Some("git") => Outcome::of(Category::Vcs).add(Category::Network),
        Some("util" | "fix" | "run") => Outcome::unknown("runs other programs"),
        Some(_) => Outcome::of(Category::Vcs),
    }
}

fn bun_runs_file(args: &[Arg]) -> bool {
    val(args, 1).is_some_and(|a| {
        a.ends_with(".js")
            || a.ends_with(".ts")
            || a.ends_with(".mjs")
            || a == "-e"
            || a == "--eval"
    })
}

const JS_INSTALL: &[&str] = &[
    "install",
    "i",
    "ci",
    "add",
    "remove",
    "uninstall",
    "rm",
    "un",
    "update",
    "up",
    "upgrade",
    "link",
    "unlink",
    "dedupe",
    "prune",
    "rebuild",
    "import",
];
const JS_SERVERS: &[&str] = &["dev", "start", "serve", "preview", "watch"];

fn js_package_manager(name: &str, args: &[Arg]) -> Outcome {
    let (sub, dynamic) = subcommand(
        args,
        &[
            "-C",
            "--prefix",
            "--filter",
            "-F",
            "--dir",
            "--cwd",
            "-w",
            "--workspace",
        ],
    );
    if dynamic {
        return Outcome::unknown("a dynamic package-manager subcommand");
    }
    let Some(si) = sub else {
        // `npm --version`; bare `pnpm` / `yarn` installs.
        return if args[1..].iter().any(|a| {
            matches!(
                a.value.as_deref(),
                Some("--version" | "-v" | "--help" | "-h")
            )
        }) {
            Outcome::of(Category::Read)
        } else if name == "yarn" || name == "bun" {
            Outcome::of(Category::Install).add(Category::Network)
        } else {
            Outcome::of(Category::Read)
        };
    };
    let sub = val(args, si).unwrap_or("");
    let script = val(args, si + 1);
    match sub {
        _ if JS_INSTALL.contains(&sub) => Outcome::of(Category::Install).add(Category::Network),
        "view" | "info" | "search" | "outdated" | "audit" | "show" | "dist-tag" | "ping"
        | "whoami" => {
            if sub == "audit" && script == Some("fix") {
                Outcome::of(Category::Install).add(Category::Network)
            } else {
                Outcome::of(Category::Network)
            }
        }
        "ls" | "list" | "why" | "explain" | "root" | "bin" | "prefix" | "help" | "config"
        | "get" | "query" | "pack" | "version"
            if !(sub == "version" && script.is_some_and(|s| !s.starts_with('-'))) =>
        {
            Outcome::of(Category::Read)
        }
        "publish" | "unpublish" | "deprecate" | "login" | "logout" | "adduser" | "owner"
        | "access" | "token" | "version" => Outcome::of(Category::Network).add(Category::Mutate),
        "exec" | "x" | "dlx" | "create" | "init" => Outcome::of(Category::Build),
        "run" | "run-script" => {
            let mut o = Outcome::of(Category::Build);
            if script.is_some_and(|s| JS_SERVERS.contains(&s)) {
                o.cats.insert(Category::LongRunning);
            }
            o
        }
        _ => {
            // `npm test`, `pnpm build`, `yarn dev`: project scripts.
            let mut o = Outcome::of(Category::Build);
            if JS_SERVERS.contains(&sub) {
                o.cats.insert(Category::LongRunning);
            }
            o
        }
    }
}

fn cargo(args: &[Arg]) -> Outcome {
    let (sub, dynamic) = subcommand(args, &["-C", "--manifest-path", "--config", "-Z"]);
    if dynamic {
        return Outcome::unknown("a dynamic cargo subcommand");
    }
    match sub.and_then(|i| val(args, i)) {
        None => Outcome::of(Category::Read),
        Some(
            "install" | "add" | "remove" | "rm" | "update" | "fetch" | "uninstall" | "vendor"
            | "generate-lockfile",
        ) => Outcome::of(Category::Install).add(Category::Network),
        Some(
            "tree" | "metadata" | "pkgid" | "locate-project" | "read-manifest" | "help" | "version"
            | "verify-project",
        ) => Outcome::of(Category::Read),
        Some("search" | "info" | "owner") => Outcome::of(Category::Network),
        Some("publish" | "yank" | "login" | "logout") => {
            Outcome::of(Category::Network).add(Category::Mutate)
        }
        Some("watch") => Outcome::of(Category::Build).add(Category::LongRunning),
        Some(_) => Outcome::of(Category::Build),
    }
}

fn go(args: &[Arg]) -> Outcome {
    match val(args, 1) {
        Some("get" | "install" | "mod") => Outcome::of(Category::Install).add(Category::Network),
        Some("version" | "env" | "list" | "doc" | "help") | None => Outcome::of(Category::Read),
        Some(_) if args[1].value.is_none() => Outcome::unknown("a dynamic go subcommand"),
        _ => Outcome::of(Category::Build),
    }
}

fn uv(args: &[Arg]) -> Outcome {
    let (sub, dynamic) = subcommand(args, &["--directory", "--project", "-p", "--python"]);
    if dynamic {
        return Outcome::unknown("a dynamic uv subcommand");
    }
    let Some(si) = sub else {
        return Outcome::of(Category::Read);
    };
    match val(args, si).unwrap_or("") {
        "add" | "sync" | "remove" | "lock" | "venv" => {
            Outcome::of(Category::Install).add(Category::Network)
        }
        "pip" => pip(&args[si + 1..]),
        "tool" => match val(args, si + 1) {
            Some("install" | "uninstall" | "upgrade") => {
                Outcome::of(Category::Install).add(Category::Network)
            }
            Some("run") => Outcome::of(Category::Build),
            _ => Outcome::of(Category::Read),
        },
        "run" => {
            // `uv run [opts] cmd ...`: the command decides.
            let mut i = si + 1;
            while let Some(a) = val(args, i) {
                if !a.starts_with('-') || a == "-" {
                    break;
                }
                i += if matches!(
                    a,
                    "--with"
                        | "--python"
                        | "-p"
                        | "--package"
                        | "--env-file"
                        | "--extra"
                        | "--group"
                ) {
                    2
                } else {
                    1
                };
            }
            if i >= args.len() || val(args, i) == Some("-") {
                return Outcome::unknown("uv run without a command runs a script");
            }
            wrap_at(args, i, Via::Wrapper)
        }
        "tree" | "version" | "help" | "python" => Outcome::of(Category::Read),
        _ => Outcome::of(Category::Build),
    }
}

/// `pip ...` with `args` starting at the subcommand.
fn pip(args: &[Arg]) -> Outcome {
    let sub = args
        .iter()
        .find_map(|a| a.value.as_deref().filter(|v| !v.starts_with('-')));
    match sub {
        Some("install" | "uninstall" | "download" | "wheel") => {
            Outcome::of(Category::Install).add(Category::Network)
        }
        Some("list" | "show" | "freeze" | "check" | "help" | "config" | "debug" | "inspect")
        | None => Outcome::of(Category::Read),
        Some("search" | "index") => Outcome::of(Category::Network),
        Some(_) => Outcome::unknown("an unknown pip subcommand"),
    }
}

fn system_package_manager(name: &str, args: &[Arg]) -> Outcome {
    let (sub, dynamic) = subcommand(args, &[]);
    if dynamic {
        return Outcome::unknown("a dynamic subcommand");
    }
    let sub = sub.and_then(|i| val(args, i));
    match sub {
        Some(
            "install" | "add" | "upgrade" | "update" | "reinstall" | "uninstall" | "remove"
            | "target" | "toolchain" | "component" | "use" | "default" | "link" | "unlink" | "tap"
            | "untap" | "cask" | "sync" | "autoremove" | "cleanup" | "set" | "override" | "self",
        ) => Outcome::of(Category::Install).add(Category::Network),
        Some("services") => Outcome::of(Category::Mutate),
        Some("run" | "exec") => Outcome::unknown("runs a program"),
        Some("search" | "outdated" | "fetch") => Outcome::of(Category::Network),
        None if name == "rustup" || name == "brew" => Outcome::of(Category::Read),
        _ if args[1..]
            .iter()
            .any(|a| matches!(a.value.as_deref(), Some("-S" | "-Syu" | "-U" | "-R" | "-i"))) =>
        {
            Outcome::of(Category::Install).add(Category::Network)
        }
        _ => Outcome::of(Category::Read),
    }
}

fn docker(args: &[Arg]) -> Outcome {
    let (sub, dynamic) = subcommand(
        args,
        &[
            "-H",
            "--host",
            "--context",
            "-c",
            "--config",
            "-l",
            "--log-level",
        ],
    );
    if dynamic {
        return Outcome::unknown("a dynamic docker subcommand");
    }
    match sub.and_then(|i| val(args, i)) {
        None
        | Some(
            "ps" | "images" | "logs" | "inspect" | "version" | "info" | "stats" | "top" | "port"
            | "diff" | "history" | "events" | "search",
        ) => Outcome::of(Category::Read),
        Some("pull" | "push" | "login" | "logout") => {
            Outcome::of(Category::Network).add(Category::Mutate)
        }
        Some("build" | "buildx") => Outcome::of(Category::Build),
        Some("run" | "exec") => Outcome::unknown("runs a command in a container"),
        Some("compose" | "start") => Outcome::of(Category::Build).add(Category::LongRunning),
        Some(_) => Outcome::of(Category::Mutate),
    }
}

fn tmux(args: &[Arg]) -> Outcome {
    match val(args, 1) {
        Some(
            "ls" | "list-sessions" | "list-windows" | "list-panes" | "list-clients"
            | "capture-pane" | "has-session" | "has" | "display-message" | "display"
            | "show-options" | "show" | "-V",
        ) => Outcome::of(Category::Read),
        Some("kill-session" | "kill-server" | "kill-window" | "kill-pane") => {
            Outcome::of(Category::Mutate)
        }
        _ => Outcome::unknown("tmux can run commands in its panes"),
    }
}

fn service(args: &[Arg]) -> Outcome {
    match val(args, 1) {
        Some(
            "list" | "print" | "status" | "show" | "is-active" | "is-enabled" | "list-units"
            | "blame" | "print-disabled",
        ) => Outcome::of(Category::Read),
        _ => Outcome::of(Category::Mutate).add(Category::LongRunning),
    }
}

fn rsync(args: &[Arg]) -> Outcome {
    let mut o = Outcome::of(Category::Mutate);
    let operands: Vec<usize> = (1..args.len())
        .filter(|&i| args[i].value.as_deref().is_none_or(|v| !v.starts_with('-')))
        .collect();
    if args[1..].iter().any(|a| {
        matches!(a.value.as_deref(), Some("-e" | "--rsh"))
            || a.value.as_deref().is_some_and(|v| v.starts_with("--rsh="))
    }) {
        o.set_unknown("rsync -e runs a remote shell command");
    }
    if operands
        .iter()
        .any(|&i| val(args, i).is_none_or(|v| v.contains(':')))
    {
        o.cats.insert(Category::Network);
    }
    if let Some((&last, rest)) = operands.split_last() {
        o.reads.extend(rest.iter().map(|&i| path(args, i)));
        o.writes.push(path(args, last));
    }
    o
}

fn tar(args: &[Arg]) -> Outcome {
    let Some(mode) = val(args, 1) else {
        return Outcome::unknown("a dynamic tar mode");
    };
    let all: Vec<&str> = args[1..]
        .iter()
        .filter_map(|a| a.value.as_deref())
        .collect();
    if all.iter().any(|a| {
        a.starts_with("--to-command")
            || a.starts_with("--use-compress-program")
            || a.starts_with("--checkpoint-action")
            || a.starts_with("--info-script")
            || a.starts_with("--new-volume-script")
            || *a == "-I"
    }) {
        return Outcome::unknown("tar runs a program");
    }
    // The mode letters: bundled (`tar xzf`) or as options.
    let letters: String = if mode.starts_with('-') {
        all.iter()
            .filter(|a| a.starts_with('-') && !a.starts_with("--"))
            .flat_map(|a| a[1..].chars())
            .collect()
    } else {
        String::from(mode)
    };
    let long = |n: &str| all.iter().any(|a| a.starts_with(n));
    if letters.contains('t') || long("--list") {
        Outcome::of(Category::Read)
    } else if letters.contains('x') || long("--extract") || long("--get") {
        if letters.contains('O') || long("--to-stdout") {
            Outcome::of(Category::Read)
        } else {
            Outcome::of(Category::Mutate)
        }
    } else {
        Outcome::of(Category::Mutate)
    }
}

/// `gzip -c`, `-t`, `-l`: to stdout, test or list; the files stay.
fn compressor_reads(args: &[Arg]) -> bool {
    args[1..].iter().all(|a| a.value.is_some())
        && args[1..]
            .iter()
            .filter_map(|a| a.value.as_deref())
            .any(|v| {
                matches!(v, "--stdout" | "--to-stdout" | "--test" | "--list")
                    || (v.starts_with('-')
                        && !v.starts_with("--")
                        && v[1..].contains(['c', 't', 'l']))
            })
}

/// `export PATH=…` and the other declaration builtins.
fn sets_dangerous(args: &[Arg]) -> bool {
    args[1..].iter().any(|a| {
        let name = a.text.split('=').next().unwrap_or("");
        a.text.contains('=') && crate::dangerous_var(name.trim_end_matches('+'))
    })
}

fn unzip(args: &[Arg]) -> Outcome {
    if args[1..].iter().any(|a| {
        matches!(
            a.value.as_deref(),
            Some("-l" | "-p" | "-Z" | "-Z1" | "-t" | "-v" | "-lv" | "-qq" | "-c")
        )
    }) {
        Outcome::of(Category::Read)
    } else {
        Outcome::of(Category::Mutate)
    }
}

fn shell(args: &[Arg]) -> Outcome {
    let mut i = 1;
    let mut command_string = false;
    let mut check_only = false;
    while i < args.len() {
        let Some(a) = args[i].value.as_deref() else {
            return Outcome::unknown("a dynamic word in a shell's arguments");
        };
        if a == "--" || a == "-" {
            i += 1;
            break;
        }
        if let Some(long) = a.strip_prefix("--") {
            if matches!(long, "version" | "help") {
                return Outcome::of(Category::Read);
            }
            if matches!(long, "rcfile" | "init-file" | "login") {
                return Outcome::unknown("the shell loads startup files");
            }
            i += 1;
            continue;
        }
        if !(a.starts_with('-') || a.starts_with('+')) || a.len() == 1 {
            break;
        }
        for c in a[1..].chars() {
            match c {
                'i' | 'l' if a.starts_with('-') => {
                    return Outcome::unknown("the shell loads startup files");
                }
                'c' => command_string = true,
                'n' => check_only = true,
                'o' | 'O' => i += 1,
                _ => {}
            }
        }
        i += 1;
    }
    if command_string {
        return match args.get(i) {
            Some(a) => match a.value.as_deref() {
                Some(code) => {
                    let mut o = Outcome::of(Category::Read);
                    o.nested.push(Nested::Source {
                        first: i,
                        last: i,
                        text: String::from(code),
                        via: Via::ShellString,
                    });
                    o
                }
                None => Outcome::unknown("a shell runs a dynamic string"),
            },
            None => Outcome::unknown("-c without a command"),
        };
    }
    if i < args.len() {
        if check_only {
            let mut o = Outcome::of(Category::Read);
            o.reads.push(path(args, i));
            return o;
        }
        return Outcome::unknown("runs a script file");
    }
    let mut o = Outcome::of(Category::Read);
    o.nested.push(Nested::Stdin);
    o
}

fn version_only(args: &[Arg]) -> bool {
    args.len() == 2
        && matches!(
            val(args, 1),
            Some("--version" | "-V" | "-v" | "--help" | "-h")
        )
}

fn python(args: &[Arg]) -> Outcome {
    if version_only(args) {
        return Outcome::of(Category::Read);
    }
    // `python -m module`
    let mut i = 1;
    while let Some(a) = val(args, i) {
        if a == "-m" {
            return match val(args, i + 1) {
                Some("pip") => pip(&args[i + 2..]),
                Some(
                    "pytest" | "unittest" | "mypy" | "ruff" | "black" | "compileall" | "py_compile"
                    | "build" | "doctest",
                ) => Outcome::of(Category::Build),
                Some("venv" | "ensurepip") => Outcome::of(Category::Install),
                Some("http.server") => Outcome::of(Category::LongRunning).add(Category::Network),
                Some("json.tool") => Outcome::of(Category::Read),
                _ => Outcome::unknown("runs a Python module"),
            };
        }
        if !a.starts_with('-') || a == "-" || a == "-c" {
            break;
        }
        i += 1;
    }
    Outcome::unknown("an interpreter runs code the classifier cannot see")
}

fn interpreter(name: &str, args: &[Arg]) -> Outcome {
    if version_only(args) {
        return Outcome::of(Category::Read);
    }
    if name == "node" {
        if args[1..]
            .iter()
            .any(|a| a.value.as_deref() == Some("--test"))
        {
            return Outcome::of(Category::Build);
        }
        if args.len() == 3 && matches!(val(args, 1), Some("--check" | "-c")) {
            let mut o = Outcome::of(Category::Read);
            o.reads.push(path(args, 2));
            return o;
        }
    }
    if name == "bun" && matches!(val(args, 1), Some("test" | "build")) {
        return Outcome::of(Category::Build);
    }
    if name == "perl"
        && args[1..].iter().any(|a| {
            a.value
                .as_deref()
                .is_some_and(|v| v.starts_with('-') && !v.starts_with("--") && v[1..].contains('i'))
        })
    {
        let mut o = Outcome::unknown("perl -i edits files with code the classifier cannot see");
        o.cats.insert(Category::Mutate);
        return o;
    }
    Outcome::unknown("an interpreter runs code the classifier cannot see")
}
