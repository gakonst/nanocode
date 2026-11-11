use alloc::vec;
use alloc::vec::Vec;

use crate::*;

fn ere(pat: &str) -> Regex {
    Regex::new(
        pat.as_bytes(),
        Options {
            extended: true,
            utf8: true,
            ..Options::default()
        },
    )
    .unwrap()
}

fn caps(pat: &str, text: &str) -> Option<Vec<Option<(usize, usize)>>> {
    ere(pat).exec(text.as_bytes(), ExecOptions::default())
}

#[test]
fn leftmost_longest_and_gnu_submatches() {
    assert_eq!(caps("b+", "abbbc"), Some(vec![Some((1, 4))]));
    // Leftmost, then longest: not the first alternative.
    assert_eq!(caps("a|ab|abc", "xabcd"), Some(vec![Some((1, 4))]));
    // Inside the longest match, GNU takes alternatives in order.
    assert_eq!(
        caps("(a|ab)(c|bcd)(d*)", "abcd"),
        Some(vec![Some((0, 4)), Some((0, 1)), Some((1, 4)), Some((4, 4))])
    );
    // A group inside a repetition keeps what an earlier iteration set.
    assert_eq!(
        caps("((a)|b)*", "ab"),
        Some(vec![Some((0, 2)), Some((1, 2)), Some((0, 1))])
    );
    assert_eq!(caps("(a*)*", "aa"), Some(vec![Some((0, 2)), Some((0, 2))]));
    assert_eq!(caps("x", "abc"), None);
}

#[test]
fn errors() {
    let e = |p: &str, ext: bool| {
        Regex::new(
            p.as_bytes(),
            Options {
                extended: ext,
                ..Options::default()
            },
        )
        .unwrap_err()
        .kind
    };
    assert_eq!(e("*a", true), ErrorKind::BadRepeat);
    assert_eq!(e("a{", true), ErrorKind::Brace);
    assert_eq!(e("a{2,1}", true), ErrorKind::BadBrace);
    assert_eq!(e("(a", true), ErrorKind::Paren);
    assert_eq!(e("[a", true), ErrorKind::Brack);
    assert_eq!(e("[[:foo:]]", true), ErrorKind::Ctype);
    assert_eq!(e("[z-a]", true), ErrorKind::Range);
    assert_eq!(e("a\\", true), ErrorKind::Escape);
    assert_eq!(e("(a)|\\1", true), ErrorKind::Subreg);
    assert_eq!(e("c**", false), ErrorKind::BadRepeat);
    assert_eq!(e("a\\)", false), ErrorKind::Paren);
    assert_eq!(
        Error::new(ErrorKind::Brack).message(),
        "Unmatched [, [^, [:, [., or [="
    );
}

#[test]
fn back_references() {
    let bre = |p: &str| {
        Regex::new(p.as_bytes(), Options::default())
            .unwrap()
            .exec(b"xabab", ExecOptions::default())
    };
    assert_eq!(bre("\\(ab\\)\\1"), Some(vec![Some((1, 5)), Some((1, 3))]));
    assert_eq!(
        caps("(a*)\\1", "aaaaa"),
        Some(vec![Some((0, 4)), Some((0, 2))])
    );
}

#[test]
fn linear_without_back_references() {
    // Classic exponential cases for backtracking matchers.
    let text = "a".repeat(20_000);
    let re = ere("(a*)*b");
    assert_eq!(re.exec(text.as_bytes(), ExecOptions::default()), None);
    let re = ere("(a|aa)*$");
    let m = re.exec(text.as_bytes(), ExecOptions::default()).unwrap();
    assert_eq!(m[0], Some((0, 20_000)));
    let re = ere("(x+x+)+y");
    assert_eq!(
        re.exec("x".repeat(5_000).as_bytes(), ExecOptions::default()),
        None
    );
}

#[test]
fn utf8_and_bytes() {
    assert_eq!(caps("^.$", "é"), Some(vec![Some((0, 2))]));
    let c = Regex::new(
        b"^.$",
        Options {
            extended: true,
            ..Options::default()
        },
    )
    .unwrap();
    assert!(c.exec("é".as_bytes(), ExecOptions::default()).is_none());
    // A byte that is not UTF-8 matches only itself.
    assert!(caps(".", "\u{fffd}").is_some());
    let re = ere("^.$");
    assert!(re.exec(b"\xff", ExecOptions::default()).is_none());
    let icase = Regex::new(
        "É".as_bytes(),
        Options {
            extended: true,
            icase: true,
            utf8: true,
            ..Options::default()
        },
    )
    .unwrap();
    assert!(icase.is_match("é".as_bytes()));
}

#[test]
fn newline_mode() {
    let re = Regex::new(
        b"^b.$",
        Options {
            extended: true,
            newline: true,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(re.find(b"a\nbc\nd", ExecOptions::default()), Some((2, 4)));
    assert_eq!(re.find(b"a\nb\n\nd", ExecOptions::default()), None);
}

#[test]
fn search_from_an_offset_sees_the_text_before() {
    let nb = ere(r"\B");
    let eo = ExecOptions::default();
    // Inside "abcd" every position between letters is not a word boundary.
    assert_eq!(nb.find_at(b"abcd", 1, eo), Some((1, 1)));
    assert_eq!(nb.find_at(b"abcd", 2, eo), Some((2, 2)));
    let b = ere(r"\<b");
    assert_eq!(b.find_at(b"ab b", 1, eo), Some((3, 4)));
    let caret = ere("^a");
    assert_eq!(caret.find_at(b"aa", 1, eo), None);
    let g = ere("(b+)");
    assert_eq!(g.exec_at(b"abba bb", 4, eo).unwrap()[1], Some((5, 7)));
}
