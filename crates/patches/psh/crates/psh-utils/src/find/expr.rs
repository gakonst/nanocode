//! `find`'s command line: the leading options, the start paths and the expression,
//! parsed into a tree. GNU's grammar and messages (`!`/`-not` binds tightest, then the
//! implicit or explicit `-a`, then `-o`, then `,`); BSD's leading options and primaries
//! where GNU has none (`-E`, `-s`, `-x`, `-d`, `-depth N`, `+mode`).

use psh_regex::Regex;

/// How symbolic links are followed (`-P`, `-H`, `-L`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Follow {
    Never,
    Roots,
    Always,
}

/// The options that are not tests: they hold for the whole walk.
#[derive(Debug)]
pub struct Global {
    pub follow: Follow,
    pub max_depth: Option<usize>,
    pub min_depth: usize,
    /// Directories after their contents (`-depth`, `-d`, implied by `-delete`).
    pub depth_first: bool,
    pub xdev: bool,
    /// `-E`, `-regextype posix-extended`.
    pub regex_extended: bool,
}

/// `+N` (more than), `-N` (less than), `N` (exactly).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Lt,
    Eq,
    Gt,
}

impl Cmp {
    pub fn test(self, value: i128, n: i128) -> bool {
        match self {
            Cmp::Lt => value < n,
            Cmp::Eq => value == n,
            Cmp::Gt => value > n,
        }
    }
}

/// Which time of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Access,
    Change,
    Modify,
    Birth,
}

/// How `-perm` compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermHow {
    Exact,
    /// `-MODE`: all of the bits.
    All,
    /// `/MODE` (BSD `+MODE`): any of the bits.
    Any,
}

/// A name pattern (`-name`, `-path`, `-lname`), fnmatch without `FNM_PATHNAME` or
/// `FNM_PERIOD`: `*` matches `/` and leading dots.
#[derive(Debug)]
pub struct Glob(crate::search::glob::GlobMatcher);

impl Glob {
    fn new(pat: &[u8], icase: bool) -> Glob {
        let text = String::from_utf8_lossy(pat);
        let build = |p: &str| {
            crate::search::glob::GlobBuilder::new(p)
                .literal_separator(false)
                .backslash_escape(true)
                .case_insensitive(icase)
                .build()
        };
        // Braces are not special to fnmatch.
        let escaped = text.replace('{', "[{]").replace('}', "[}]");
        let glob = build(&escaped).or_else(|_| {
            // What fnmatch takes literally (an unclosed `[`) the glob parser rejects.
            build(&crate::search::glob::escape(&text))
        });
        Glob(glob.expect("an escaped glob compiles").compile_matcher())
    }

    pub fn matches(&self, s: &[u8]) -> bool {
        self.0.is_match(crate::search::walk::to_path(s))
    }
}

/// One node of the expression.
#[derive(Debug)]
pub enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Comma(Box<Expr>, Box<Expr>),
    Const(bool),
    Name(Glob),
    Path(Glob),
    Lname(Glob),
    Regex(Regex),
    /// The type letters (`fdlpscbD`), and whether to look through links (`-xtype`).
    Type(Vec<u8>, bool),
    /// The size in `unit`-byte blocks, rounded up.
    Size(Cmp, u64, u64),
    /// The age in `unit` seconds; minutes round up as GNU's `-mmin` does.
    Age(Field, Cmp, i64, i64),
    /// Newer than a reference time (an index into [`Parsed::refs`]).
    Newer(Field, usize),
    Perm(PermHow, u32),
    Empty,
    Links(Cmp, u64),
    Inum(Cmp, u64),
    Uid(Cmp, u64),
    Gid(Cmp, u64),
    User(Vec<u8>),
    Group(Vec<u8>),
    NoUser,
    NoGroup,
    Access(psh_engine::host::Access),
    /// The same file as a reference (an index into [`Parsed::refs`]).
    SameFile(usize),
    /// BSD `-depth N`.
    Depth(Cmp, u64),
    Prune,
    Quit,
    Print(u8),
    Printf(Vec<u8>),
    /// An index into [`Parsed::execs`].
    Exec(usize),
    Delete,
}

/// What `-newer`, `-newerXY` and `-samefile` compare with, resolved before the walk.
#[derive(Debug)]
pub enum Ref {
    /// A file's time (`-newer`, `-anewer`, `-newerXY`).
    File(Vec<u8>, Field),
    /// `-newermt DATE`.
    Date(Vec<u8>),
    /// `-samefile`.
    Same(Vec<u8>),
}

/// `-exec`, `-execdir`: the command with `{}` placeholders; with `+`, the files are
/// collected and appended.
#[derive(Debug)]
pub struct Exec {
    pub argv: Vec<Vec<u8>>,
    pub batch: bool,
    pub in_dir: bool,
}

#[derive(Debug)]
pub struct Parsed {
    pub global: Global,
    pub paths: Vec<Vec<u8>>,
    pub expr: Expr,
    pub refs: Vec<Ref>,
    pub execs: Vec<Exec>,
    /// `-printf` warnings, printed before the walk.
    pub warnings: Vec<String>,
}

pub enum Error {
    /// GNU's message; status 1.
    Usage(String),
    /// Something psh does not implement: the real program gets the command line.
    Unsupported(String),
}

fn usage<T>(msg: impl Into<String>) -> Result<T, Error> {
    Err(Error::Usage(msg.into()))
}

fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

struct Parser<'a> {
    args: &'a [Vec<u8>],
    i: usize,
    global: Global,
    refs: Vec<Ref>,
    execs: Vec<Exec>,
    has_action: bool,
    /// `-depth` given (not only implied by `-delete`), `-delete` and `-prune` seen.
    depth_given: bool,
    has_delete: bool,
    has_prune: bool,
    warnings: Vec<String>,
    utf8: bool,
}

/// Parses the arguments after `find`.
pub fn parse(args: &[Vec<u8>], utf8: bool) -> Result<Parsed, Error> {
    let mut p = Parser {
        args,
        i: 0,
        global: Global {
            follow: Follow::Never,
            max_depth: None,
            min_depth: 0,
            depth_first: false,
            xdev: false,
            regex_extended: false,
        },
        refs: Vec::new(),
        execs: Vec::new(),
        has_action: false,
        depth_given: false,
        has_delete: false,
        has_prune: false,
        warnings: Vec::new(),
        utf8,
    };
    // Leading options: `-H -L -P` (GNU and BSD), BSD's `-E -s -x -d`.
    while let Some(a) = args.get(p.i) {
        if a.len() < 2
            || a[0] != b'-'
            || a == b"--"
            || !a[1..].iter().all(|c| b"HLPEsxd".contains(c))
        {
            if a == b"--" {
                p.i += 1;
            }
            break;
        }
        for &c in &a[1..] {
            match c {
                b'H' => p.global.follow = Follow::Roots,
                b'L' => p.global.follow = Follow::Always,
                b'P' => p.global.follow = Follow::Never,
                b'E' => p.global.regex_extended = true,
                b'x' => p.global.xdev = true,
                b'd' => {
                    p.global.depth_first = true;
                    p.depth_given = true;
                }
                // Sorted traversal: psh always sorts.
                _ => {}
            }
        }
        p.i += 1;
    }
    let mut paths = Vec::new();
    while let Some(a) = args.get(p.i) {
        if is_expression_start(a) {
            break;
        }
        paths.push(a.clone());
        p.i += 1;
    }
    if paths.is_empty() {
        paths.push(b".".to_vec());
    }
    let mut expr = if p.i < args.len() {
        let e = p.comma()?;
        if let Some(a) = args.get(p.i) {
            if a == b")" {
                return usage("you have too many ')'");
            }
            return usage(stray(args, p.i));
        }
        e
    } else {
        Expr::Const(true)
    };
    if p.has_delete && p.has_prune && !p.depth_given {
        return usage(
            "The -delete action automatically turns on -depth, but -prune does nothing when -depth is in effect.  If you want to carry on anyway, just explicitly use the -depth option.",
        );
    }
    if !p.has_action {
        expr = Expr::And(Box::new(expr), Box::new(Expr::Print(b'\n')));
    }
    Ok(Parsed {
        global: p.global,
        paths,
        expr,
        refs: p.refs,
        execs: p.execs,
        warnings: p.warnings,
    })
}

/// GNU's message for a word where an expression should be, with its hint when the
/// word follows a pattern (`find . -name *.txt` with the glob expanded).
fn stray(args: &[Vec<u8>], i: usize) -> String {
    let mut msg = format!("paths must precede expression: `{}'", s(&args[i]));
    if let Some(p) = i.checked_sub(2).map(|k| &args[k])
        && matches!(
            p.as_slice(),
            b"-name"
                | b"-iname"
                | b"-path"
                | b"-ipath"
                | b"-wholename"
                | b"-iwholename"
                | b"-lname"
                | b"-ilname"
                | b"-regex"
                | b"-iregex"
        )
    {
        msg.push_str(&format!(
            "\nfind: possible unquoted pattern after predicate `{}'?",
            s(p)
        ));
    }
    msg
}

/// Where the start paths end: an option-like word, `!`, `(`.
fn is_expression_start(a: &[u8]) -> bool {
    (a.len() > 1 && a[0] == b'-') || a == b"!" || a == b"(" || a == b")" || a == b","
}

impl Parser<'_> {
    fn peek(&self) -> Option<&[u8]> {
        self.args.get(self.i).map(Vec::as_slice)
    }

    fn comma(&mut self) -> Result<Expr, Error> {
        let mut e = self.or()?;
        while self.peek() == Some(b",") {
            self.i += 1;
            if self.at_end_of_operand() {
                return usage("expected an expression after ','");
            }
            let r = self.or()?;
            e = Expr::Comma(Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn at_end_of_operand(&self) -> bool {
        matches!(self.peek(), None | Some(b")"))
    }

    fn or(&mut self) -> Result<Expr, Error> {
        let mut e = self.and()?;
        while matches!(self.peek(), Some(b"-o" | b"-or")) {
            let op = s(self.peek().unwrap_or_default());
            self.i += 1;
            if self.at_end_of_operand()
                || matches!(self.peek(), Some(b"-o" | b"-or" | b"-a" | b"-and" | b","))
            {
                return usage(format!("expected an expression after '{op}'"));
            }
            let r = self.and()?;
            e = Expr::Or(Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn and(&mut self) -> Result<Expr, Error> {
        let mut e = self.not()?;
        loop {
            match self.peek() {
                None | Some(b"-o" | b"-or" | b")" | b",") => return Ok(e),
                Some(b"-a" | b"-and") => {
                    let op = s(self.peek().unwrap_or_default());
                    self.i += 1;
                    if self.at_end_of_operand()
                        || matches!(self.peek(), Some(b"-o" | b"-or" | b"-a" | b"-and" | b","))
                    {
                        return usage(format!("expected an expression after '{op}'"));
                    }
                }
                _ => {}
            }
            let r = self.not()?;
            e = Expr::And(Box::new(e), Box::new(r));
        }
    }

    fn not(&mut self) -> Result<Expr, Error> {
        match self.peek() {
            Some(b"!" | b"-not") => {
                let op = s(self.peek().unwrap_or_default());
                self.i += 1;
                if self.at_end_of_operand()
                    || matches!(self.peek(), Some(b"-o" | b"-or" | b"-a" | b"-and" | b","))
                {
                    return usage(format!("expected an expression after '{op}'"));
                }
                Ok(Expr::Not(Box::new(self.not()?)))
            }
            Some(b"(") => {
                self.i += 1;
                if self.peek() == Some(b")") {
                    return usage("invalid expression; empty parentheses are not allowed.");
                }
                if self.peek().is_none() {
                    return usage(
                        "invalid expression; I was expecting to find a ')' somewhere but did not see one.",
                    );
                }
                let e = self.comma()?;
                if self.peek() != Some(b")") {
                    return usage(
                        "invalid expression; I was expecting to find a ')' somewhere but did not see one.",
                    );
                }
                self.i += 1;
                Ok(e)
            }
            Some(op @ (b"-o" | b"-or" | b"-a" | b"-and" | b",")) => {
                let op = s(op);
                usage(format!(
                    "invalid expression; you have used a binary operator '{op}' with nothing before it."
                ))
            }
            Some(b")") => usage("invalid expression; you have too many ')'"),
            _ => self.primary(),
        }
    }

    /// The argument of the primary `name`.
    fn arg(&mut self, name: &str) -> Result<Vec<u8>, Error> {
        match self.args.get(self.i) {
            Some(a) => {
                self.i += 1;
                Ok(a.clone())
            }
            None => usage(format!("missing argument to `{name}'")),
        }
    }

    fn number_arg(&mut self, name: &str) -> Result<(Cmp, u64), Error> {
        let a = self.arg(name)?;
        parse_cmp(&a)
            .ok_or_else(|| Error::Usage(format!("invalid argument `{}' to `{name}'", s(&a))))
    }

    fn depth_arg(&mut self, name: &str) -> Result<usize, Error> {
        let a = self.arg(name)?;
        let ok = !a.is_empty() && a.iter().all(u8::is_ascii_digit);
        match std::str::from_utf8(&a)
            .ok()
            .filter(|_| ok)
            .and_then(|t| t.parse().ok())
        {
            Some(n) => Ok(n),
            None => usage(format!(
                "Expected a positive decimal integer argument to {name}, but got {}",
                crate::common::quote_in(self.utf8, &a)
            )),
        }
    }

    fn regex(&self, pat: &[u8], icase: bool) -> Result<Regex, Error> {
        let given = pat;
        let (pat, extended) = if self.global.regex_extended {
            (pat.to_vec(), true)
        } else {
            (emacs_to_bre(pat), false)
        };
        let opts = psh_regex::Options {
            extended,
            icase,
            newline: false,
            utf8: self.utf8,
        };
        Regex::new(&pat, opts).map_err(|e| {
            Error::Usage(format!(
                "failed to compile regular expression '{}': {}",
                s(given),
                e.message()
            ))
        })
    }

    fn primary(&mut self) -> Result<Expr, Error> {
        let a = self.args[self.i].clone();
        self.i += 1;
        let name = s(&a);
        let e = match name.as_str() {
            // Options: true where they stand, effective everywhere.
            "-maxdepth" => {
                self.global.max_depth = Some(self.depth_arg("-maxdepth")?);
                Expr::Const(true)
            }
            "-mindepth" => {
                self.global.min_depth = self.depth_arg("-mindepth")?;
                Expr::Const(true)
            }
            "-depth" => {
                // BSD `-depth N` compares the depth; GNU's takes no argument.
                if let Some(n) = self.peek().and_then(parse_cmp) {
                    self.i += 1;
                    Expr::Depth(n.0, n.1)
                } else {
                    self.global.depth_first = true;
                    self.depth_given = true;
                    Expr::Const(true)
                }
            }
            "-d" => {
                self.global.depth_first = true;
                self.depth_given = true;
                Expr::Const(true)
            }
            "-xdev" | "-mount" | "-x" => {
                self.global.xdev = true;
                Expr::Const(true)
            }
            "-follow" => {
                self.global.follow = Follow::Always;
                Expr::Const(true)
            }
            "-noleaf" | "-ignore_readdir_race" | "-noignore_readdir_race" | "-warn" | "-nowarn" => {
                Expr::Const(true)
            }
            "-regextype" => {
                let t = self.arg("-regextype")?;
                match t.as_slice() {
                    b"posix-extended" | b"egrep" | b"posix-egrep" | b"awk" | b"posix-awk"
                    | b"gnu-awk" => self.global.regex_extended = true,
                    b"emacs"
                    | b"posix-basic"
                    | b"grep"
                    | b"posix-minimal-basic"
                    | b"findutils-default" => self.global.regex_extended = false,
                    _ => {
                        return usage(format!(
                            "Unknown regular expression type {}; valid types are ‘findutils-default’, ‘ed’, ‘emacs’, ‘gnu-awk’, ‘grep’, ‘posix-awk’, ‘awk’, ‘posix-basic’, ‘posix-egrep’, ‘egrep’, ‘posix-extended’, ‘posix-minimal-basic’, ‘sed’.",
                            crate::common::quote_in(self.utf8, &t)
                        ));
                    }
                }
                Expr::Const(true)
            }
            "-true" => Expr::Const(true),
            "-false" => Expr::Const(false),
            "-name" | "-iname" => {
                let p = self.arg(&name)?;
                Expr::Name(Glob::new(&p, name == "-iname"))
            }
            "-path" | "-ipath" | "-wholename" | "-iwholename" => {
                let p = self.arg(&name)?;
                Expr::Path(Glob::new(&p, name.starts_with("-i")))
            }
            "-lname" | "-ilname" => {
                let p = self.arg(&name)?;
                Expr::Lname(Glob::new(&p, name == "-ilname"))
            }
            "-regex" | "-iregex" => {
                // `-regextype` is positional: it applies to the patterns after it.
                let p = self.arg(&name)?;
                Expr::Regex(self.regex(&p, name == "-iregex")?)
            }
            "-type" | "-xtype" => {
                let t = self.arg(&name)?;
                let mut letters = Vec::new();
                for (k, part) in t.split(|&b| b == b',').enumerate() {
                    match part {
                        [c] if b"fdlpscbD".contains(c) => {
                            if letters.contains(c) {
                                return usage(format!(
                                    "Duplicate file type ‘{}’ in the argument list to {name}.",
                                    *c as char
                                ));
                            }
                            letters.push(*c)
                        }
                        [] if k > 0 || t.is_empty() => {
                            return usage(format!(
                                "Arguments to {name} should contain only one letter"
                            ));
                        }
                        _ if part.len() > 1 => {
                            return usage(format!(
                                "Must separate multiple arguments to {name} using: ','"
                            ));
                        }
                        _ => return usage(format!("Unknown argument to {name}: {}", s(part))),
                    }
                }
                Expr::Type(letters, name == "-xtype")
            }
            "-size" => {
                let a = self.arg("-size")?;
                let bad = || Error::Usage(format!("invalid -size type `{}'", s(&a[a.len() - 1..])));
                let (num, unit) = match a.last() {
                    Some(c) if c.is_ascii_alphabetic() => {
                        let unit = match c {
                            b'c' => 1,
                            b'w' => 2,
                            b'b' => 512,
                            b'k' | b'K' => 1024,
                            b'M' => 1 << 20,
                            b'G' => 1 << 30,
                            b'T' => 1 << 40,
                            b'P' => 1 << 50,
                            _ => return Err(bad()),
                        };
                        (&a[..a.len() - 1], unit)
                    }
                    _ => (&a[..], 512),
                };
                let (cmp, n) = parse_cmp(num).ok_or_else(|| {
                    Error::Usage(format!("invalid argument `{}' to `-size'", s(&a)))
                })?;
                Expr::Size(cmp, n, unit)
            }
            "-mtime" | "-atime" | "-ctime" | "-Btime" | "-mmin" | "-amin" | "-cmin" | "-Bmin" => {
                let field = field_of(name.as_bytes()[1]);
                let (cmp, n) = self.number_arg(&name)?;
                let unit = if name.ends_with("min") { 60 } else { 86400 };
                Expr::Age(field, cmp, n as i64, unit)
            }
            "-newer" | "-anewer" | "-cnewer" => {
                let f = self.arg(&name)?;
                let field = match name.as_str() {
                    "-anewer" => Field::Access,
                    "-cnewer" => Field::Change,
                    _ => Field::Modify,
                };
                self.refs.push(Ref::File(f, Field::Modify));
                Expr::Newer(field, self.refs.len() - 1)
            }
            n if n.starts_with("-newer") && n.len() == 8 => {
                let (x, y) = (n.as_bytes()[6], n.as_bytes()[7]);
                let ok = |c: u8| b"acmB".contains(&c);
                if !ok(x) || !(ok(y) || y == b't') {
                    return Err(Error::Unsupported(name));
                }
                let r = self.arg(&name)?;
                self.refs.push(if y == b't' {
                    Ref::Date(r)
                } else {
                    Ref::File(r, field_of(y))
                });
                Expr::Newer(field_of(x), self.refs.len() - 1)
            }
            "-perm" => {
                let a = self.arg("-perm")?;
                let (how, spec) = match a.first() {
                    Some(b'-') => (PermHow::All, &a[1..]),
                    Some(b'/' | b'+') => (PermHow::Any, &a[1..]),
                    _ => (PermHow::Exact, &a[..]),
                };
                let Some(mode) = crate::modes::parse(spec) else {
                    return usage(format!(
                        "invalid mode {}",
                        crate::common::quote_in(self.utf8, &a)
                    ));
                };
                Expr::Perm(how, mode.apply(0, false, 0))
            }
            "-empty" => Expr::Empty,
            "-links" => {
                let (c, n) = self.number_arg("-links")?;
                Expr::Links(c, n)
            }
            "-inum" => {
                let (c, n) = self.number_arg("-inum")?;
                Expr::Inum(c, n)
            }
            "-uid" => {
                let (c, n) = self.number_arg("-uid")?;
                Expr::Uid(c, n)
            }
            "-gid" => {
                let (c, n) = self.number_arg("-gid")?;
                Expr::Gid(c, n)
            }
            "-user" => {
                let u = self.arg("-user")?;
                match std::str::from_utf8(&u)
                    .ok()
                    .and_then(|t| t.parse::<u64>().ok())
                {
                    Some(n) => Expr::Uid(Cmp::Eq, n),
                    None => Expr::User(u),
                }
            }
            "-group" => {
                let g = self.arg("-group")?;
                match std::str::from_utf8(&g)
                    .ok()
                    .and_then(|t| t.parse::<u64>().ok())
                {
                    Some(n) => Expr::Gid(Cmp::Eq, n),
                    None => Expr::Group(g),
                }
            }
            "-nouser" => Expr::NoUser,
            "-nogroup" => Expr::NoGroup,
            "-readable" => Expr::Access(psh_engine::host::Access::Read),
            "-writable" => Expr::Access(psh_engine::host::Access::Write),
            "-executable" => Expr::Access(psh_engine::host::Access::Execute),
            "-samefile" => {
                let f = self.arg("-samefile")?;
                self.refs.push(Ref::Same(f));
                Expr::SameFile(self.refs.len() - 1)
            }
            "-prune" => {
                self.has_prune = true;
                Expr::Prune
            }
            "-quit" => Expr::Quit,
            "-print" => {
                self.has_action = true;
                Expr::Print(b'\n')
            }
            "-print0" => {
                self.has_action = true;
                Expr::Print(0)
            }
            "-printf" => {
                self.has_action = true;
                let f = self.arg("-printf")?;
                self.warnings.extend(super::printf::check(&f));
                Expr::Printf(f)
            }
            "-delete" => {
                self.has_action = true;
                self.has_delete = true;
                self.global.depth_first = true;
                Expr::Delete
            }
            "-exec" | "-execdir" => {
                self.has_action = true;
                let mut argv = Vec::new();
                let batch = loop {
                    let Some(w) = self.args.get(self.i) else {
                        return usage(format!("missing argument to `{name}'"));
                    };
                    self.i += 1;
                    if w == b";" {
                        break false;
                    }
                    if w == b"+" && argv.last().is_some_and(|l: &Vec<u8>| l == b"{}") {
                        argv.pop();
                        break true;
                    }
                    argv.push(w.clone());
                };
                if argv.is_empty() {
                    return usage(format!("missing argument to `{name}'"));
                }
                if batch && argv.iter().any(|a| a.windows(2).any(|w| w == b"{}")) {
                    return usage(format!(
                        "Only one instance of {{}} is supported with {name} ... +"
                    ));
                }
                self.execs.push(Exec {
                    argv,
                    batch,
                    in_dir: name == "-execdir",
                });
                Expr::Exec(self.execs.len() - 1)
            }
            _ if a.first() == Some(&b'-') => return Err(Error::Unsupported(name)),
            _ => return usage(stray(self.args, self.i - 1)),
        };
        Ok(e)
    }
}

fn field_of(c: u8) -> Field {
    match c {
        b'a' => Field::Access,
        b'c' => Field::Change,
        b'B' => Field::Birth,
        _ => Field::Modify,
    }
}

/// `+N`, `-N`, `N`.
fn parse_cmp(a: &[u8]) -> Option<(Cmp, u64)> {
    let (cmp, digits) = match a.first()? {
        b'+' => (Cmp::Gt, &a[1..]),
        b'-' => (Cmp::Lt, &a[1..]),
        _ => (Cmp::Eq, a),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some((cmp, std::str::from_utf8(digits).ok()?.parse().ok()?))
}

/// GNU's default `-regex` syntax is Emacs's: like a GNU BRE with `+` and `?` as
/// operators (`\+`, `\?` there) and `{` literal.
fn emacs_to_bre(p: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(p.len() + 4);
    let mut i = 0;
    while i < p.len() {
        match p[i] {
            b'\\' if i + 1 < p.len() => {
                match p[i + 1] {
                    // Emacs has no intervals: `\{` is a brace.
                    b'{' | b'}' => out.push(p[i + 1]),
                    c => out.extend_from_slice(&[b'\\', c]),
                }
                i += 2;
            }
            b'+' | b'?' => {
                out.extend_from_slice(&[b'\\', p[i]]);
                i += 1;
            }
            b'[' => {
                // A bracket expression, copied as is.
                let start = i;
                i += 1;
                if p.get(i) == Some(&b'^') {
                    i += 1;
                }
                if p.get(i) == Some(&b']') {
                    i += 1;
                }
                while i < p.len() && p[i] != b']' {
                    if p[i] == b'[' && matches!(p.get(i + 1), Some(b':' | b'.' | b'=')) {
                        let close = p[i + 1];
                        i += 2;
                        while i + 1 < p.len() && !(p[i] == close && p[i + 1] == b']') {
                            i += 1;
                        }
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                i = (i + 1).min(p.len());
                out.extend_from_slice(&p[start..i]);
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emacs_syntax() {
        assert_eq!(
            emacs_to_bre(br".*\.\(js\|ts\)"),
            br".*\.\(js\|ts\)".to_vec()
        );
        assert_eq!(emacs_to_bre(b"a+b?"), br"a\+b\?".to_vec());
        assert_eq!(emacs_to_bre(b"[+?]x{2}"), b"[+?]x{2}".to_vec());
    }

    #[test]
    fn globs_are_fnmatch() {
        assert!(Glob::new(b"*.txt", false).matches(b".hidden.txt"));
        assert!(Glob::new(b"./*/x", false).matches(b"./a/b/x"));
        assert!(Glob::new(b"{a,b}", false).matches(b"{a,b}"));
        assert!(Glob::new(b"[", false).matches(b"["));
        assert!(Glob::new(b"*.TXT", true).matches(b"a.txt"));
        assert!(Glob::new(br"\*", false).matches(b"*"));
    }
}
