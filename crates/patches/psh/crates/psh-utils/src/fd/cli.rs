//! fd's command line (D1138) on psh's option parser: fd 10.5.1's flags, aliases,
//! overrides and conflicts as its clap definition (`src/cli.rs` at 193d0df) declares
//! them, with clap's wording for the errors agents meet. Written for psh after that
//! definition.
//!
//! `-x`/`-X` take every following word up to `;` as the command, which no option parser
//! of the GNU kind does: [`split_exec`] takes them out first.

use crate::opts::{Arg, Opt, Spec, both, flag, long};

const OPTS: &[Opt] = &[
    both('H', "hidden"),
    long("no-hidden"),
    both('I', "no-ignore"),
    long("ignore"),
    long("no-ignore-vcs"),
    long("ignore-vcs"),
    long("no-require-git"),
    long("require-git"),
    long("no-ignore-parent"),
    long("ignore-parent"),
    long("no-global-ignore-file"),
    both('u', "unrestricted"),
    both('s', "case-sensitive"),
    both('i', "ignore-case"),
    both('g', "glob"),
    long("regex"),
    both('F', "fixed-strings"),
    long("literal"),
    long("exact"),
    long("and").value(),
    both('a', "absolute-path"),
    long("relative-path"),
    both('l', "list-details"),
    both('L', "follow"),
    long("dereference"),
    long("no-follow"),
    both('p', "full-path"),
    both('0', "print0"),
    both('d', "max-depth").value(),
    long("maxdepth").value(),
    long("min-depth").value(),
    long("mindepth").value(),
    long("exact-depth").value(),
    both('E', "exclude").value(),
    long("prune"),
    both('t', "type").value(),
    both('e', "extension").value(),
    both('S', "size").value(),
    long("changed-within").value(),
    long("change-newer-than").value(),
    long("newer").value(),
    long("changed-after").value(),
    long("changed-before").value(),
    long("change-older-than").value(),
    long("older").value(),
    both('o', "owner").value(),
    long("format").value(),
    both('x', "exec").value(),
    both('X', "exec-batch").value(),
    long("batch-size").value(),
    long("ignore-file").value(),
    both('c', "color").value(),
    long("hyperlink").optional(),
    long("hyper").optional(),
    long("ignore-contain").value(),
    both('j', "threads").value(),
    long("max-buffer-time").value(),
    long("max-results").value(),
    flag('1'),
    both('q', "quiet"),
    long("has-results"),
    long("show-errors"),
    both('C', "base-directory").value(),
    long("path-separator").value(),
    long("search-path").value(),
    long("strip-cwd-prefix").optional(),
    long("one-file-system"),
    long("mount"),
    long("xdev"),
    both('h', "help"),
    both('V', "version"),
];

pub const SPEC: Spec = Spec {
    name: "fd",
    opts: OPTS,
    posix_order: false,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    File,
    Directory,
    Symlink,
    BlockDevice,
    CharDevice,
    Executable,
    Empty,
    Socket,
    Pipe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    Auto,
    Always,
    Never,
}

/// The parsed command line, as fd's `Opts` after clap.
#[derive(Debug, Default)]
pub struct Opts {
    pub hidden: bool,
    pub no_ignore: bool,
    pub no_ignore_vcs: bool,
    pub no_require_git: bool,
    pub no_ignore_parent: bool,
    pub no_global_ignore_file: bool,
    pub case_sensitive: bool,
    pub ignore_case: bool,
    pub glob: bool,
    pub fixed_strings: bool,
    pub exact: bool,
    pub exprs: Vec<Vec<u8>>,
    pub absolute_path: bool,
    pub list_details: bool,
    pub follow: bool,
    pub full_path: bool,
    pub null_separator: bool,
    pub max_depth: Option<usize>,
    pub min_depth: Option<usize>,
    pub exact_depth: Option<usize>,
    pub exclude: Vec<String>,
    pub prune: bool,
    pub filetype: Vec<FileType>,
    pub extensions: Vec<String>,
    pub size: Vec<super::filter::SizeFilter>,
    pub changed_within: Option<String>,
    pub changed_before: Option<String>,
    pub owner: Option<String>,
    pub format: Option<String>,
    /// The `-x` commands, or the `-X` ones.
    pub exec: Vec<Vec<Vec<u8>>>,
    pub exec_batch: bool,
    pub batch_size: usize,
    pub ignore_file: Vec<Vec<u8>>,
    pub color: Option<When>,
    pub hyperlink: Option<When>,
    pub ignore_contain: Vec<Vec<u8>>,
    pub threads: Option<usize>,
    pub max_results: Option<usize>,
    pub max_one_result: bool,
    pub quiet: bool,
    pub show_errors: bool,
    pub base_directory: Option<Vec<u8>>,
    pub pattern: Option<Vec<u8>>,
    pub path_separator: Option<String>,
    pub path: Vec<Vec<u8>>,
    pub search_path: Vec<Vec<u8>>,
    pub strip_cwd_prefix: Option<When>,
    pub one_file_system: bool,
}

impl Opts {
    pub fn no_search_paths(&self) -> bool {
        self.path.is_empty() && self.search_path.is_empty()
    }

    pub fn max_depth(&self) -> Option<usize> {
        self.max_depth.or(self.exact_depth)
    }

    pub fn min_depth(&self) -> Option<usize> {
        self.min_depth.or(self.exact_depth)
    }

    pub fn max_results(&self) -> Option<usize> {
        self.max_results
            .filter(|&m| m > 0)
            .or_else(|| self.max_one_result.then_some(1))
    }

    /// Whether `./` is stripped from results: only for the implicit search path, and
    /// by default only when no command runs and results are not NUL-separated.
    pub fn strip_cwd_prefix(&self, auto: bool) -> bool {
        self.no_search_paths()
            && match self.strip_cwd_prefix.unwrap_or(When::Auto) {
                When::Auto => auto,
                When::Always => true,
                When::Never => false,
            }
    }
}

/// What the command line asks for.
#[derive(Debug)]
pub enum Parsed {
    Run(Box<Opts>),
    Help { long: bool },
    Version,
}

/// A command-line error: clap's message (status 2), or an option psh does not know
/// (the real fd runs).
#[derive(Debug)]
pub enum CliError {
    Usage(String),
    Unknown(String),
}

/// One option as written, its canonical long name and its value.
struct Given {
    name: &'static str,
    value: Option<Vec<u8>>,
}

pub fn parse(args: &[Vec<u8>]) -> Result<Parsed, CliError> {
    let (rest, execs) = split_exec(args)?;
    let parsed = SPEC.parse(&rest).map_err(|e| {
        if e.is_unknown() {
            CliError::Unknown(e.option().to_string())
        } else {
            match e {
                crate::opts::OptError::MissingValue(o) => {
                    let name = canonical(o.trim_start_matches('-'));
                    CliError::Usage(missing_value(name))
                }
                crate::opts::OptError::UnexpectedValue(o) => CliError::Usage(format!(
                    "error: unexpected value for '{o}' found; no more were expected"
                )),
                other => CliError::Unknown(other.option().to_string()),
            }
        }
    })?;
    let given: Vec<Given> = parsed
        .opts
        .iter()
        .map(|f| Given {
            name: canonical(&SPEC.opts[f.index].key()),
            value: f.value.clone(),
        })
        .collect();
    if given.iter().any(|g| g.name == "help") {
        // `-h` short help, `--help` long help: the last one given wins, as in clap.
        let long = args
            .iter()
            .rev()
            .find(|a| a.as_slice() == b"-h" || a.starts_with(b"--h"))
            .is_some_and(|a| a.starts_with(b"--"));
        return Ok(Parsed::Help { long });
    }
    if given.iter().any(|g| g.name == "version") {
        return Ok(Parsed::Version);
    }
    let mut o = Opts::default();
    let (mut hidden, mut no_ignore, mut unrestricted) = (None, None, false);
    // The options as given, for the conflict checks: canonical name, in order.
    let mut seen: Vec<&'static str> = Vec::new();
    for g in &given {
        let v = || String::from_utf8_lossy(g.value.as_deref().unwrap_or_default()).into_owned();
        let num = |name: &'static str| -> Result<usize, CliError> {
            let s = v();
            s.parse::<usize>()
                .map_err(|e| invalid_value(&s, name, Some(&e.to_string())))
        };
        match g.name {
            "hidden" => hidden = Some(true),
            "no-hidden" => hidden = Some(false),
            "no-ignore" => no_ignore = Some(true),
            "ignore" => no_ignore = Some(false),
            "unrestricted" => unrestricted = true,
            "no-ignore-vcs" => o.no_ignore_vcs = true,
            "ignore-vcs" => o.no_ignore_vcs = false,
            "no-require-git" => o.no_require_git = true,
            "require-git" => o.no_require_git = false,
            "no-ignore-parent" => o.no_ignore_parent = true,
            "ignore-parent" => o.no_ignore_parent = false,
            "no-global-ignore-file" => o.no_global_ignore_file = true,
            "case-sensitive" => (o.case_sensitive, o.ignore_case) = (true, false),
            "ignore-case" => (o.case_sensitive, o.ignore_case) = (false, true),
            "glob" => o.glob = true,
            "regex" => o.glob = false,
            "fixed-strings" => o.fixed_strings = true,
            "exact" => o.exact = true,
            "and" => o.exprs.push(g.value.clone().unwrap_or_default()),
            "absolute-path" => o.absolute_path = true,
            "relative-path" => o.absolute_path = false,
            "list-details" => o.list_details = true,
            "follow" => o.follow = true,
            "no-follow" => o.follow = false,
            "full-path" => o.full_path = true,
            "print0" => o.null_separator = true,
            "max-depth" => o.max_depth = Some(num("max-depth")?),
            "min-depth" => o.min_depth = Some(num("min-depth")?),
            "exact-depth" => o.exact_depth = Some(num("exact-depth")?),
            "exclude" => o.exclude.push(v()),
            "prune" => o.prune = true,
            "type" => o.filetype.push(file_type(&v())?),
            "extension" => o.extensions.push(v()),
            "size" => {
                let s = v();
                match super::filter::SizeFilter::from_string(&s) {
                    Ok(f) => o.size.push(f),
                    Err(e) => return Err(invalid_value(&s, "size", Some(&e))),
                }
            }
            "changed-within" => o.changed_within = Some(v()),
            "changed-before" => o.changed_before = Some(v()),
            "owner" => {
                let s = v();
                if let Err(super::filter::OwnerError::Invalid(msg)) =
                    super::filter::OwnerFilter::from_string(&s)
                {
                    return Err(invalid_value(&s, "owner", Some(&msg)));
                }
                o.owner = Some(s);
            }
            "format" => o.format = Some(v()),
            "batch-size" => o.batch_size = num("batch-size")?,
            "ignore-file" => o.ignore_file.push(g.value.clone().unwrap_or_default()),
            "color" => o.color = Some(when(&v(), "color")?),
            "hyperlink" => {
                o.hyperlink = Some(match &g.value {
                    None => When::Auto,
                    Some(_) => when(&v(), "hyperlink")?,
                })
            }
            "ignore-contain" => o.ignore_contain.push(g.value.clone().unwrap_or_default()),
            "threads" => {
                let s = v();
                match s.parse::<usize>() {
                    Ok(0) => {
                        return Err(invalid_value(
                            &s,
                            "threads",
                            Some("number would be zero for non-zero type"),
                        ));
                    }
                    Ok(n) => o.threads = Some(n),
                    Err(e) => return Err(invalid_value(&s, "threads", Some(&e.to_string()))),
                }
            }
            "max-buffer-time" => {
                num("max-buffer-time")?;
            }
            "max-results" => {
                o.max_results = Some(num("max-results")?);
                o.max_one_result = false;
            }
            "1" => {
                o.max_one_result = true;
                o.max_results = None;
            }
            "quiet" => o.quiet = true,
            "show-errors" => o.show_errors = true,
            "base-directory" => o.base_directory = g.value.clone(),
            "path-separator" => o.path_separator = Some(v()),
            "search-path" => o.search_path.push(g.value.clone().unwrap_or_default()),
            "strip-cwd-prefix" => {
                o.strip_cwd_prefix = Some(match &g.value {
                    None => When::Always,
                    Some(_) => when(&v(), "strip-cwd-prefix")?,
                })
            }
            "one-file-system" => o.one_file_system = true,
            _ => {}
        }
        if !seen.contains(&g.name) {
            seen.push(g.name);
        }
    }
    o.hidden = hidden.unwrap_or(unrestricted);
    o.no_ignore = no_ignore.unwrap_or(unrestricted);
    let mut operands = parsed.operands.into_iter();
    o.pattern = operands.next();
    o.path = operands.collect();
    if let Some((batch, cmds)) = execs {
        o.exec_batch = batch;
        o.exec = cmds;
        seen.push(if batch { "exec-batch" } else { "exec" });
    }
    if o.pattern.is_some() {
        seen.push("pattern");
    }
    if !o.path.is_empty() {
        seen.push("path");
    }
    check_conflicts(&seen, &o)?;
    Ok(Parsed::Run(Box::new(o)))
}

/// The option an alias stands for (fd's names, as its messages use them).
fn canonical(name: &str) -> &'static str {
    let name = match name {
        "literal" => "fixed-strings",
        "dereference" => "follow",
        "maxdepth" => "max-depth",
        "mindepth" => "min-depth",
        "change-newer-than" | "newer" | "changed-after" => "changed-within",
        "change-older-than" | "older" => "changed-before",
        "hyper" => "hyperlink",
        "has-results" => "quiet",
        "mount" | "xdev" => "one-file-system",
        other => other,
    };
    // Every name is a key of OPTS (or one of the aliases above).
    OPTS.iter()
        .find_map(|o| {
            let key = match (o.long, o.short) {
                (Some(l), _) => l,
                (None, Some('1')) => "1",
                _ => return None,
            };
            (key == name).then_some(key)
        })
        .unwrap_or("")
}

/// The option as clap's messages show it: `--name <value>`, or `-1`.
fn display(name: &str) -> String {
    let value = match name {
        "and" | "pattern" => Some("pattern"),
        "max-depth" | "min-depth" | "exact-depth" => Some("depth"),
        "exclude" => Some("glob"),
        "type" => Some("filetype"),
        "extension" => Some("ext"),
        "size" | "batch-size" => Some("size"),
        "changed-within" | "changed-before" => Some("date|dur"),
        "owner" => Some("user:group"),
        "format" => Some("fmt"),
        "exec" | "exec-batch" => Some("cmd"),
        "ignore-file" | "base-directory" | "path" => Some("path"),
        "color" | "hyperlink" | "strip-cwd-prefix" => Some("when"),
        "ignore-contain" => Some("name"),
        "threads" => Some("num"),
        "max-results" => Some("count"),
        "path-separator" => Some("separator"),
        "search-path" => Some("search-path"),
        _ => None,
    };
    match (name, value) {
        ("1", _) => "-1".to_string(),
        ("pattern", _) => "[pattern]".to_string(),
        ("path", _) => "[path]...".to_string(),
        ("exec" | "exec-batch", Some(v)) => format!("--{name} <{v}>..."),
        ("hyperlink" | "strip-cwd-prefix", Some(v)) => format!("--{name}[=<{v}>]"),
        (_, Some(v)) => format!("--{name} <{v}>"),
        (_, None) => format!("--{name}"),
    }
}

const TRY_HELP: &str = "\nFor more information, try '--help'.";

fn missing_value(name: &str) -> String {
    format!(
        "error: a value is required for '{}' but none was supplied\n{TRY_HELP}",
        display(name)
    )
}

fn invalid_value(value: &str, name: &str, why: Option<&str>) -> CliError {
    let why = why.map(|w| format!(": {w}")).unwrap_or_default();
    CliError::Usage(format!(
        "error: invalid value '{value}' for '{}'{why}\n{TRY_HELP}",
        display(name)
    ))
}

fn when(v: &str, name: &str) -> Result<When, CliError> {
    match v {
        "auto" => Ok(When::Auto),
        "always" => Ok(When::Always),
        "never" => Ok(When::Never),
        _ => Err(CliError::Usage(format!(
            "error: invalid value '{v}' for '{}'\n  [possible values: auto, always, never]\n{TRY_HELP}",
            display(name)
        ))),
    }
}

fn file_type(v: &str) -> Result<FileType, CliError> {
    Ok(match v {
        "f" | "file" => FileType::File,
        "d" | "dir" | "directory" => FileType::Directory,
        "l" | "symlink" => FileType::Symlink,
        "b" | "block-device" => FileType::BlockDevice,
        "c" | "char-device" => FileType::CharDevice,
        "x" | "executable" => FileType::Executable,
        "e" | "empty" => FileType::Empty,
        "s" | "socket" => FileType::Socket,
        "p" | "pipe" => FileType::Pipe,
        _ => {
            return Err(CliError::Usage(format!(
                "error: invalid value '{v}' for '--type <filetype>'\n  [possible values: file, \
                 directory, symlink, block-device, char-device, executable, empty, socket, \
                 pipe]\n{TRY_HELP}"
            )));
        }
    })
}

/// clap's conflicts of fd's definition, reported for the first pair given.
fn check_conflicts(seen: &[&'static str], o: &Opts) -> Result<(), CliError> {
    const PAIRS: &[(&str, &str)] = &[
        ("glob", "fixed-strings"),
        ("exact", "glob"),
        ("exact", "fixed-strings"),
        ("list-details", "absolute-path"),
        ("print0", "list-details"),
        ("exact-depth", "max-depth"),
        ("exact-depth", "min-depth"),
        ("prune", "size"),
        ("prune", "exact-depth"),
        ("format", "list-details"),
        ("exec", "list-details"),
        ("exec-batch", "exec"),
        ("exec-batch", "list-details"),
        ("quiet", "max-results"),
        ("search-path", "path"),
        ("strip-cwd-prefix", "path"),
        ("strip-cwd-prefix", "search-path"),
        // The `execs` group (`-x`, `-X`, `-l`).
        ("exec", "max-results"),
        ("exec", "quiet"),
        ("exec", "1"),
        ("exec-batch", "max-results"),
        ("exec-batch", "quiet"),
        ("exec-batch", "1"),
        ("list-details", "max-results"),
        ("list-details", "quiet"),
        ("list-details", "1"),
    ];
    // Flags overridden later (`-g --regex`) no longer conflict.
    let active = |n: &str| match n {
        "glob" => o.glob,
        "absolute-path" => o.absolute_path,
        "max-results" => o.max_results.is_some(),
        "1" => o.max_one_result,
        _ => true,
    };
    let pos = |n: &str| seen.iter().position(|s| *s == n);
    let mut found: Option<(usize, &str, &str)> = None;
    for &(a, b) in PAIRS {
        if let (Some(i), Some(j)) = (pos(a), pos(b))
            && active(a)
            && active(b)
        {
            // clap names the one given first.
            let (first, second, at) = if i <= j { (a, b, j) } else { (b, a, i) };
            if found.is_none_or(|(k, _, _)| at < k) {
                found = Some((at, first, second));
            }
        }
    }
    if let Some((_, a, b)) = found {
        let used = |n: &str| seen.contains(&n);
        let pattern = if used("pattern") {
            "<pattern>"
        } else {
            "[pattern]"
        };
        let path = if used("path") {
            "<path>..."
        } else {
            "[path]..."
        };
        let first = display(a);
        return Err(CliError::Usage(format!(
            "error: the argument '{first}' cannot be used with '{}'\n\nUsage: fd {first} \
             {pattern} {path}\n{TRY_HELP}",
            display(b)
        )));
    }
    if o.batch_size != 0 && !o.exec_batch {
        let pattern = if seen.contains(&"pattern") {
            "<pattern>"
        } else {
            "[pattern]"
        };
        let path = if seen.contains(&"path") {
            "<path>..."
        } else {
            "[path]..."
        };
        return Err(CliError::Usage(format!(
            "error: the following required arguments were not provided:\n  --exec-batch \
             <cmd>...\n\nUsage: fd --exec-batch <cmd>... --batch-size <size> {pattern} \
             {path}\n{TRY_HELP}"
        )));
    }
    Ok(())
}

/// The `-x`/`-X` commands taken out of `args`: everything after the option up to `;`
/// (or the end). Returns the other words, and whether the commands are batches.
#[allow(clippy::type_complexity)]
fn split_exec(
    args: &[Vec<u8>],
) -> Result<(Vec<Vec<u8>>, Option<(bool, Vec<Vec<Vec<u8>>>)>), CliError> {
    let mut rest = Vec::new();
    let mut cmds: Vec<Vec<Vec<u8>>> = Vec::new();
    let mut batch = None;
    let mut i = 0;
    let mut only_operands = false;
    // Starts a command: its first word if attached, then words up to `;`.
    let mut take = |first: Option<Vec<u8>>, i: &mut usize, is_batch: bool| {
        let mut cmd: Vec<Vec<u8>> = first.into_iter().collect();
        while let Some(a) = args.get(*i) {
            *i += 1;
            if a == b";" {
                break;
            }
            cmd.push(a.clone());
        }
        if cmd.is_empty() {
            let name = if is_batch { "exec-batch" } else { "exec" };
            return Err(CliError::Usage(missing_value(name)));
        }
        // `-x` twice runs several commands; `-x` with `-X` is a conflict.
        match batch {
            Some(b) if b != is_batch => {
                let (a, b) = if b {
                    ("exec-batch", "exec")
                } else {
                    ("exec", "exec-batch")
                };
                return Err(CliError::Usage(format!(
                    "error: the argument '{}' cannot be used with '{}'\n\nUsage: fd {} \
                     [pattern] [path]...\n{TRY_HELP}",
                    display(a),
                    display(b),
                    display(a)
                )));
            }
            _ => batch = Some(is_batch),
        }
        cmds.push(cmd);
        Ok(())
    };
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if only_operands || a.len() < 2 || a[0] != b'-' {
            rest.push(a.clone());
            continue;
        }
        if a == b"--" {
            only_operands = true;
            rest.push(a.clone());
            continue;
        }
        if let Some(body) = a.strip_prefix(b"--") {
            let (name, inline) = match body.iter().position(|&b| b == b'=') {
                Some(p) => (&body[..p], Some(body[p + 1..].to_vec())),
                None => (body, None),
            };
            match by_long(&String::from_utf8_lossy(name)) {
                Some(o) if o.long == Some("exec") || o.long == Some("exec-batch") => {
                    take(inline, &mut i, o.long == Some("exec-batch"))?;
                }
                Some(o) if o.arg == Arg::Required && inline.is_none() => {
                    rest.push(a.clone());
                    if let Some(v) = args.get(i) {
                        rest.push(v.clone());
                        i += 1;
                    }
                }
                _ => rest.push(a.clone()),
            }
            continue;
        }
        // A cluster of short options.
        let mut k = 1;
        let mut handled = false;
        while k < a.len() {
            let c = a[k] as char;
            let opt = OPTS.iter().find(|o| o.short == Some(c));
            match opt {
                Some(o) if c == 'x' || c == 'X' => {
                    if k > 1 {
                        rest.push(a[..k].to_vec());
                    }
                    let first = (k + 1 < a.len()).then(|| a[k + 1..].to_vec());
                    take(first, &mut i, o.short == Some('X'))?;
                    handled = true;
                    break;
                }
                Some(o) if o.arg == Arg::Required => {
                    rest.push(a.clone());
                    if k + 1 == a.len()
                        && let Some(v) = args.get(i)
                    {
                        rest.push(v.clone());
                        i += 1;
                    }
                    handled = true;
                    break;
                }
                _ => k += 1,
            }
        }
        if !handled {
            rest.push(a.clone());
        }
    }
    Ok((rest, batch.map(|b| (b, cmds))))
}

/// The option a long name or an unambiguous prefix of one names (as [`Spec::parse`]).
fn by_long(name: &str) -> Option<&'static Opt> {
    if let Some(o) = OPTS.iter().find(|o| o.long == Some(name)) {
        return Some(o);
    }
    let mut it = OPTS
        .iter()
        .filter(|o| o.long.is_some_and(|l| l.starts_with(name)));
    match (it.next(), it.next()) {
        (Some(o), None) => Some(o),
        _ => None,
    }
}
