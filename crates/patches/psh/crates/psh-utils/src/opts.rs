//! Option parsing for utilities (D1082): options are data, one parser for all, with GNU
//! conventions.
//!
//! - Options and operands may mix (`head file -n 3`); `--` ends the options; a lone `-`
//!   is an operand (stdin). With `Spec::posix_order`, the first operand ends the
//!   options instead (BSD and POSIX utilities, `timeout`, `env`, `xargs` whose operands
//!   are a command).
//! - Short options cluster (`-vn`); a value attaches (`-n5`) or is the next word
//!   (`-n 5`).
//! - Long options take `--name=value` or `--name value` (required values), or only
//!   `--name=value` (optional values); an unambiguous prefix names an option (`--lin`).
//!
//! Utilities with forms outside this (`head -5`, `tail +3`) rewrite their argv first.

use std::fmt;

/// Whether an option takes a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arg {
    None,
    /// Always: attached or the next word.
    Required,
    /// Long options only: `--name=value`, else none.
    Optional,
}

/// One option: its short letter, its long name (without `--`), and its value.
#[derive(Clone, Copy, Debug)]
pub struct Opt {
    pub short: Option<char>,
    pub long: Option<&'static str>,
    pub arg: Arg,
}

/// `-x`
pub const fn flag(short: char) -> Opt {
    Opt {
        short: Some(short),
        long: None,
        arg: Arg::None,
    }
}

/// `-x` / `--long`
pub const fn both(short: char, long: &'static str) -> Opt {
    Opt {
        short: Some(short),
        long: Some(long),
        arg: Arg::None,
    }
}

/// `--long`
pub const fn long(long: &'static str) -> Opt {
    Opt {
        short: None,
        long: Some(long),
        arg: Arg::None,
    }
}

impl Opt {
    /// The same option with a required value.
    pub const fn value(self) -> Opt {
        Opt {
            arg: Arg::Required,
            ..self
        }
    }

    /// The same (long) option with an optional value.
    pub const fn optional(self) -> Opt {
        Opt {
            arg: Arg::Optional,
            ..self
        }
    }

    /// The name used in messages and as the key of parsed options: the long name if
    /// there is one, else the letter.
    pub fn key(&self) -> String {
        match (self.long, self.short) {
            (Some(l), _) => l.to_string(),
            (None, Some(s)) => s.to_string(),
            (None, None) => String::new(),
        }
    }
}

/// A utility's options.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub name: &'static str,
    pub opts: &'static [Opt],
    /// The first operand ends the options.
    pub posix_order: bool,
}

/// One parsed option: which one (its index in `Spec::opts`) and its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub index: usize,
    pub value: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    /// In command-line order (later options override earlier ones where that matters).
    pub opts: Vec<Found>,
    pub operands: Vec<Vec<u8>>,
}

impl Parsed {
    /// Whether the option with this long name (or letter, for short-only options) was
    /// given.
    pub fn has(&self, spec: &Spec, key: &str) -> bool {
        self.opts.iter().any(|f| spec.opts[f.index].key() == key)
    }

    /// The value of the last occurrence of an option.
    pub fn last(&self, spec: &Spec, key: &str) -> Option<&[u8]> {
        self.opts
            .iter()
            .rev()
            .find(|f| spec.opts[f.index].key() == key)
            .and_then(|f| f.value.as_deref())
    }

    /// Every option's key and value, in order.
    pub fn iter<'s>(
        &'s self,
        spec: &'s Spec,
    ) -> impl Iterator<Item = (String, Option<&'s [u8]>)> + 's {
        self.opts
            .iter()
            .map(|f| (spec.opts[f.index].key(), f.value.as_deref()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptError {
    /// Not an option of this utility: as written (`-x` or `--name`).
    Unknown(String),
    /// A value is required: the option as written.
    MissingValue(String),
    /// `--name=value` for an option without a value.
    UnexpectedValue(String),
    /// A long prefix that names several options.
    Ambiguous(String),
}

impl OptError {
    /// The option as the user wrote it.
    pub fn option(&self) -> &str {
        match self {
            OptError::Unknown(o)
            | OptError::MissingValue(o)
            | OptError::UnexpectedValue(o)
            | OptError::Ambiguous(o) => o,
        }
    }

    /// Unknown options fall through to the real program; the others are usage errors
    /// the real program would report the same way.
    pub fn is_unknown(&self) -> bool {
        matches!(self, OptError::Unknown(_))
    }
}

impl fmt::Display for OptError {
    /// GNU's wording.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OptError::Unknown(o) if o.starts_with("--") => write!(f, "unrecognized option '{o}'"),
            OptError::Unknown(o) => write!(f, "invalid option -- '{}'", o.trim_start_matches('-')),
            OptError::MissingValue(o) if o.starts_with("--") => {
                write!(f, "option '{o}' requires an argument")
            }
            OptError::MissingValue(o) => {
                write!(
                    f,
                    "option requires an argument -- '{}'",
                    o.trim_start_matches('-')
                )
            }
            OptError::UnexpectedValue(o) => write!(f, "option '{o}' doesn't allow an argument"),
            OptError::Ambiguous(o) => write!(f, "option '{o}' is ambiguous"),
        }
    }
}

impl Spec {
    fn by_short(&self, c: char) -> Option<usize> {
        self.opts.iter().position(|o| o.short == Some(c))
    }

    /// The option a long name or an unambiguous prefix of one names.
    fn by_long(&self, name: &str) -> Result<usize, OptError> {
        if let Some(i) = self.opts.iter().position(|o| o.long == Some(name)) {
            return Ok(i);
        }
        let matches: Vec<usize> = (0..self.opts.len())
            .filter(|&i| self.opts[i].long.is_some_and(|l| l.starts_with(name)))
            .collect();
        match matches.as_slice() {
            [i] => Ok(*i),
            [] => Err(OptError::Unknown(format!("--{name}"))),
            _ => Err(OptError::Ambiguous(format!("--{name}"))),
        }
    }

    /// Parses `args` (without argv[0]).
    pub fn parse(&self, args: &[Vec<u8>]) -> Result<Parsed, OptError> {
        let mut out = Parsed::default();
        let mut i = 0;
        let mut only_operands = false;
        while i < args.len() {
            let a = &args[i];
            i += 1;
            if only_operands || a.len() < 2 || a[0] != b'-' {
                out.operands.push(a.clone());
                if self.posix_order && !only_operands {
                    only_operands = true;
                }
                continue;
            }
            if a == b"--" {
                only_operands = true;
                continue;
            }
            if let Some(rest) = a.strip_prefix(b"--") {
                let text = String::from_utf8_lossy(rest);
                let (name, inline) = match text.split_once('=') {
                    Some((n, v)) => (n.to_string(), Some(v.as_bytes().to_vec())),
                    None => (text.to_string(), None),
                };
                let index = self.by_long(&name)?;
                let opt = &self.opts[index];
                let full = format!("--{}", opt.long.unwrap_or(&name));
                let value = match (opt.arg, inline) {
                    (Arg::None, Some(_)) => return Err(OptError::UnexpectedValue(full)),
                    (Arg::None, None) => None,
                    (Arg::Optional, v) => v,
                    (Arg::Required, Some(v)) => Some(v),
                    (Arg::Required, None) => {
                        let Some(v) = args.get(i) else {
                            return Err(OptError::MissingValue(full));
                        };
                        i += 1;
                        Some(v.clone())
                    }
                };
                out.opts.push(Found { index, value });
                continue;
            }
            // A cluster of short options.
            let text = String::from_utf8_lossy(&a[1..]).into_owned();
            for (pos, c) in text.char_indices() {
                let index = self
                    .by_short(c)
                    .ok_or_else(|| OptError::Unknown(format!("-{c}")))?;
                if self.opts[index].arg == Arg::Required {
                    let rest = &text[pos + c.len_utf8()..];
                    let value = if !rest.is_empty() {
                        // The bytes after the letter, as given (not through the lossy text).
                        a[1 + pos + c.len_utf8()..].to_vec()
                    } else if let Some(v) = args.get(i) {
                        i += 1;
                        v.clone()
                    } else {
                        return Err(OptError::MissingValue(format!("-{c}")));
                    };
                    out.opts.push(Found {
                        index,
                        value: Some(value),
                    });
                    break;
                }
                out.opts.push(Found { index, value: None });
            }
        }
        Ok(out)
    }

    /// The value-taking short letters, sorted (for the classifier check).
    pub fn short_values(&self) -> String {
        let mut s: Vec<char> = self
            .opts
            .iter()
            .filter(|o| o.arg == Arg::Required)
            .filter_map(|o| o.short)
            .collect();
        s.sort_unstable();
        s.into_iter().collect()
    }

    /// The long options with a required value, as `--name`, sorted.
    pub fn long_values(&self) -> Vec<String> {
        let mut l: Vec<String> = self
            .opts
            .iter()
            .filter(|o| o.arg == Arg::Required)
            .filter_map(|o| o.long.map(|n| format!("--{n}")))
            .collect();
        l.sort();
        l
    }

    /// Whether `-x` or `--name` (as the classifier writes options) is an option here.
    pub fn accepts(&self, option: &str) -> bool {
        if let Some(name) = option.strip_prefix("--") {
            self.opts.iter().any(|o| o.long == Some(name))
        } else {
            option
                .strip_prefix('-')
                .and_then(|r| r.chars().next())
                .is_some_and(|c| self.by_short(c).is_some())
        }
    }
}

/// Checks a spec against the classifier's table (D1082): the same value-taking options,
/// and every effect option accepted. Returns the differences.
pub fn check_against_classifier(spec: &Spec) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(u) = psh_classify::utils::utility(spec.name) else {
        return vec![format!("{}: not in psh_classify::utils", spec.name)];
    };
    let mut want: Vec<char> = u.short_values.chars().collect();
    want.sort_unstable();
    let want: String = want.into_iter().collect();
    if spec.short_values() != want {
        problems.push(format!(
            "{}: short options with values: utility '{}', classifier '{}'",
            spec.name,
            spec.short_values(),
            want
        ));
    }
    let mut want_long: Vec<String> = u.long_values.iter().map(|s| s.to_string()).collect();
    want_long.sort();
    if spec.long_values() != want_long {
        problems.push(format!(
            "{}: long options with values: utility {:?}, classifier {:?}",
            spec.name,
            spec.long_values(),
            want_long
        ));
    }
    for (opt, _) in u.effects {
        if !spec.accepts(opt) {
            problems.push(format!(
                "{}: classifier effect option {opt} not accepted",
                spec.name
            ));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: Spec = Spec {
        name: "head",
        opts: &[
            both('n', "lines").value(),
            both('c', "bytes").value(),
            both('q', "quiet"),
            long("silent"),
            both('v', "verbose"),
            long("color").optional(),
            long("line-buffered"),
            flag('z'),
        ],
        posix_order: false,
    };

    fn args(a: &[&str]) -> Vec<Vec<u8>> {
        a.iter().map(|s| s.as_bytes().to_vec()).collect()
    }

    #[test]
    fn gnu_forms() {
        let p = SPEC
            .parse(&args(&[
                "-n5",
                "a",
                "-qv",
                "--bytes",
                "7",
                "--lines=3",
                "-",
                "--",
                "-z",
            ]))
            .unwrap();
        assert_eq!(p.operands, args(&["a", "-", "-z"]));
        assert_eq!(p.last(&SPEC, "lines"), Some(&b"3"[..]));
        assert_eq!(p.last(&SPEC, "bytes"), Some(&b"7"[..]));
        assert!(p.has(&SPEC, "quiet") && p.has(&SPEC, "verbose") && !p.has(&SPEC, "z"));
        let p = SPEC
            .parse(&args(&["-n", "4", "--color", "--color=always"]))
            .unwrap();
        assert_eq!(p.last(&SPEC, "lines"), Some(&b"4"[..]));
        assert_eq!(
            p.iter(&SPEC)
                .map(|(k, v)| (k, v.map(|v| v.to_vec())))
                .collect::<Vec<_>>(),
            vec![
                ("lines".to_string(), Some(b"4".to_vec())),
                ("color".to_string(), None),
                ("color".to_string(), Some(b"always".to_vec()))
            ]
        );
    }

    #[test]
    fn errors() {
        assert_eq!(
            SPEC.parse(&args(&["-x"])),
            Err(OptError::Unknown("-x".into()))
        );
        assert_eq!(
            SPEC.parse(&args(&["--nope"])),
            Err(OptError::Unknown("--nope".into()))
        );
        assert_eq!(
            SPEC.parse(&args(&["-n"])),
            Err(OptError::MissingValue("-n".into()))
        );
        assert_eq!(
            SPEC.parse(&args(&["--line"])),
            Err(OptError::Ambiguous("--line".into()))
        );
        assert_eq!(
            SPEC.parse(&args(&["--quiet=1"])),
            Err(OptError::UnexpectedValue("--quiet".into()))
        );
        assert_eq!(
            OptError::Unknown("-x".into()).to_string(),
            "invalid option -- 'x'"
        );
        assert_eq!(
            OptError::Unknown("--nope".into()).to_string(),
            "unrecognized option '--nope'"
        );
    }

    #[test]
    fn posix_order_stops_at_the_first_operand() {
        const SPEC2: Spec = Spec {
            name: "timeout",
            opts: &[both('s', "signal").value()],
            posix_order: true,
        };
        let p = SPEC2
            .parse(&args(&["-s", "KILL", "5", "cmd", "-s", "x"]))
            .unwrap();
        assert_eq!(p.operands, args(&["5", "cmd", "-s", "x"]));
    }

    #[test]
    fn classifier_check() {
        assert_eq!(check_against_classifier(&SPEC), Vec::<String>::new());
        const WRONG: Spec = Spec {
            name: "head",
            opts: &[both('n', "lines").value()],
            posix_order: false,
        };
        assert_eq!(check_against_classifier(&WRONG).len(), 2);
    }
}
