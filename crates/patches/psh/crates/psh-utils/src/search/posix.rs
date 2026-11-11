//! POSIX BRE/ERE for the search utilities (D1131): `psh_regex` parses (GNU syntax,
//! errors and bracket rules) and translates to Rust's regex syntax; the ripgrep crates
//! match. Patterns the translation cannot carry (back-references) or whose spans can
//! differ (leftmost-longest) are matched by `psh_regex` itself where spans matter.

pub use psh_regex::{Error, Options, Translation, to_rust_syntax};

/// Translates `pattern` for a line-oriented search (`grep`): newline-sensitive, UTF-8.
pub fn for_lines(pattern: &[u8], extended: bool) -> Result<Translation, Error> {
    to_rust_syntax(
        pattern,
        Options {
            extended,
            icase: false,
            newline: true,
            utf8: true,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rs(p: &str, ere: bool) -> String {
        for_lines(p.as_bytes(), ere).unwrap().pattern.unwrap()
    }

    fn matches(p: &str, ere: bool, text: &str) -> Option<(usize, usize)> {
        let re = regex::bytes::Regex::new(&rs(p, ere)).unwrap();
        re.find(text.as_bytes()).map(|m| (m.start(), m.end()))
    }

    #[test]
    fn bre_operators() {
        assert_eq!(matches(r"\(ab\)\{2\}", false, "xababy"), Some((1, 5)));
        assert_eq!(matches(r"a\+b\?", false, "caab"), Some((1, 4)));
        assert_eq!(matches(r"x\|ab", false, "cab"), Some((1, 3)));
        // Without backslashes they are literals in BRE.
        assert_eq!(matches(r"a+(b)", false, "a+(b)"), Some((0, 5)));
        assert_eq!(matches(r"a{2}", false, "aa a{2}"), Some((3, 7)));
        // A leading `*` (also after `\(` and `^`) is a literal.
        assert_eq!(matches(r"*a", false, "x*a"), Some((1, 3)));
        assert_eq!(matches(r"\(*a\)", false, "*a"), Some((0, 2)));
        assert_eq!(matches(r"^*a", false, "*a"), Some((0, 2)));
        // `$` is an anchor only at the end; `^` only at the start.
        assert_eq!(matches(r"a$b", false, "a$b"), Some((0, 3)));
        assert_eq!(matches(r"a^b", false, "a^b"), Some((0, 3)));
        assert_eq!(matches(r"b$", false, "ab\nc"), Some((1, 2)));
    }

    #[test]
    fn ere_operators() {
        assert_eq!(matches(r"(ab){2}", true, "xababy"), Some((1, 5)));
        assert_eq!(matches(r"a|b+", true, "cbb"), Some((1, 3)));
        assert_eq!(matches(r"\(a\)", true, "(a)"), Some((0, 3)));
        assert_eq!(matches(r"a{,2}b", true, "aaab"), Some((1, 4)));
    }

    #[test]
    fn brackets() {
        // `\` is literal in a bracket; `]` first is a member.
        assert_eq!(matches(r"[\(]", false, "a\\b"), Some((1, 2)));
        assert_eq!(matches(r"[]x]", false, "a]"), Some((1, 2)));
        assert_eq!(matches(r"[^]x]", false, "]xa"), Some((2, 3)));
        assert_eq!(matches(r"[[:digit:]]+", true, "ab12c"), Some((2, 4)));
        assert_eq!(matches(r"[[:alpha:]]", true, "1é"), Some((1, 3)));
        assert_eq!(matches(r"[a-c[:space:]]+", true, "x b a"), Some((1, 5)));
        assert_eq!(matches(r"[[.-.]]", true, "a-"), Some((1, 2)));
        assert_eq!(matches(r"[[=a=]]", true, "ba"), Some((1, 2)));
        // A negated set does not match a newline in a line search.
        assert_eq!(matches(r"a[^x]b", true, "a\nb"), None);
        assert_eq!(matches(r"a.b", true, "a\nb"), None);
        // Metacharacters as members.
        assert_eq!(
            matches(r"[.*+?(){}|^$]+", true, "a.*+?(){}|^$b"),
            Some((1, 12))
        );
        assert_eq!(matches(r"[a&&b]", true, "&"), Some((0, 1)));
        assert_eq!(matches(r"[-~-]", true, "-"), Some((0, 1)));
        assert_eq!(matches(r"[~]", true, "~"), Some((0, 1)));
    }

    #[test]
    fn gnu_escapes() {
        assert_eq!(matches(r"\<is\>", false, "this is"), Some((5, 7)));
        assert_eq!(matches(r"\bx\b", true, "ax x"), Some((3, 4)));
        assert_eq!(matches(r"\w+\W", true, "  ab_1-"), Some((2, 7)));
        assert_eq!(matches(r"\s\S", true, "ab c"), Some((2, 4)));
        // An unknown escape is the character.
        assert_eq!(matches(r"\.\d", true, "a.d"), Some((1, 3)));
    }

    #[test]
    fn errors_are_gnus() {
        let e = |p: &str, ere| for_lines(p.as_bytes(), ere).unwrap_err().message();
        assert_eq!(e("a[b", false), "Unmatched [, [^, [:, [., or [=");
        assert_eq!(e(r"\(a", false), "Unmatched ( or \\(");
        assert_eq!(e(r"a\{1", false), "Unmatched \\{");
        assert_eq!(e("(a", true), "Unmatched ( or \\(");
        assert_eq!(e("[[:nope:]]", true), "Invalid character class name");
        assert_eq!(e("a\\", true), "Trailing backslash");
        assert_eq!(e("[b-a]", true), "Invalid range end");
    }

    #[test]
    fn backrefs_and_ambiguity() {
        let t = for_lines(br"\(a\)\1", false).unwrap();
        assert_eq!(t.pattern, None);
        assert!(!for_lines(b"ab*c", true).unwrap().ambiguous);
        assert!(!for_lines(b"a.*b", true).unwrap().ambiguous);
        assert!(!for_lines(b"(ab){3}x+", true).unwrap().ambiguous);
        assert!(for_lines(b"ab|abcd", true).unwrap().ambiguous);
        assert!(for_lines(b"a?(ab)?", true).unwrap().ambiguous);
        assert!(for_lines(b"(ab*)*", true).unwrap().ambiguous);
        assert!(for_lines(b"(a*){2}", true).unwrap().ambiguous);
    }

    #[test]
    fn c_locale_is_bytes() {
        let t = to_rust_syntax(
            b"[^a].\xff",
            Options {
                extended: true,
                utf8: false,
                newline: true,
                ..Options::default()
            },
        )
        .unwrap();
        let re = regex::bytes::Regex::new(&t.pattern.unwrap()).unwrap();
        assert_eq!(re.find(b"a\xfe\xfd\xff").map(|m| m.range()), Some(1..4));
    }

    /// psh-regex's differential corpus (recorded from GNU libc): wherever the
    /// translation exists and is unambiguous, the regex crate finds the same match;
    /// where it is ambiguous, the same lines match.
    #[test]
    fn agrees_with_the_gnu_corpus() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../psh-regex/tests/gnu.txt");
        let text = std::fs::read_to_string(path).unwrap();
        let unhex = |h: &str| -> Vec<u8> {
            if h == "-" {
                return Vec::new();
            }
            (0..h.len() / 2)
                .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap())
                .collect()
        };
        let (mut checked, mut failures) = (0, Vec::new());
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let (known, line) = match line.strip_prefix("! ") {
                Some(rest) => (true, rest),
                None => (false, line),
            };
            let mut f = line.splitn(4, ' ');
            let (flags, pat, txt, rest) = (
                f.next().unwrap(),
                unhex(f.next().unwrap()),
                unhex(f.next().unwrap()),
                f.next().unwrap(),
            );
            let want = if known {
                rest.split(" | ").nth(1).unwrap()
            } else {
                rest
            };
            if flags.contains('B') {
                continue;
            }
            let opts = Options {
                extended: flags.contains('E'),
                icase: flags.contains('I'),
                newline: flags.contains('N'),
                utf8: flags.contains('U'),
            };
            let t = match to_rust_syntax(&pat, opts) {
                Ok(t) => t,
                Err(e) => {
                    if !want.starts_with("ERR") {
                        failures.push(format!("{line}: error {e}"));
                    }
                    continue;
                }
            };
            if want.starts_with("ERR") {
                // Errors psh-regex's compiler finds (sizes) are not the parser's.
                continue;
            }
            let Some(p) = t.pattern else { continue };
            let re = match regex::bytes::RegexBuilder::new(&p)
                .case_insensitive(opts.icase)
                .size_limit(1 << 26)
                .build()
            {
                Ok(re) => re,
                Err(e) => {
                    failures.push(format!("{line}: {p:?} does not build: {e}"));
                    continue;
                }
            };
            checked += 1;
            let got = re.find(&txt).map(|m| (m.start(), m.end()));
            let want = want.split(' ').next().filter(|w| *w != "NOMATCH").map(|w| {
                let (a, b) = w.split_once(',').unwrap();
                (a.parse().unwrap(), b.parse().unwrap())
            });
            let ok = if t.ambiguous {
                got.is_some() == want.is_some()
            } else {
                got == want
            };
            if !ok {
                failures.push(format!(
                    "{flags} {:?} {:?}: {p:?} want {want:?}, got {got:?}",
                    String::from_utf8_lossy(&pat),
                    String::from_utf8_lossy(&txt)
                ));
            }
        }
        assert!(checked > 2500, "{checked} cases");
        assert!(
            failures.is_empty(),
            "{} of {checked} differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
