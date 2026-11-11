//! grep in-process over the test host (the inline backend and the virtual walk).
//! Behavior against GNU and BSD grep is L4 (tests/utils/grep).

use std::rc::Rc;

use psh_engine::testhost::TestHost;
use psh_engine::{Script, Shell, Step};

fn inline(src: &str) -> (i32, String, String) {
    let host = Rc::new(TestHost::new());
    let fs = &host.fs;
    fs.write_file("/w/a.txt", b"foo one\nbar\nfoo two\n");
    fs.write_file("/w/.hidden", b"foo hidden\n");
    fs.write_file("/w/.gitignore", b"*.txt\n");
    fs.write_file("/w/sub/c.rs", b"fn foo() {}\n");
    fs.write_file("/w/bin", b"zap\0bar\nzap\n");
    fs.write_file("/w/pats", b"one\ntwo\n");
    let mut cfg = host.config();
    cfg.utilities = crate::policy(false);
    cfg.cwd = b"/w".to_vec();
    let mut shell = Shell::new(cfg);
    shell.start(Script::command(src));
    let status = loop {
        match shell.step(13) {
            Step::Done(s) => break s,
            Step::Yield => {}
            other => panic!("{other:?} running {src:?}"),
        }
    };
    (
        status,
        String::from_utf8_lossy(&host.stdout()).into_owned(),
        String::from_utf8_lossy(&host.stderr()).into_owned(),
    )
}

#[track_caller]
fn out(src: &str) -> String {
    let (_, o, e) = inline(src);
    assert_eq!(e, "", "stderr of {src:?}");
    o
}

#[test]
fn basics() {
    assert_eq!(out("grep foo a.txt"), "foo one\nfoo two\n");
    assert_eq!(out("grep -n foo a.txt"), "1:foo one\n3:foo two\n");
    assert_eq!(out("grep -c foo a.txt sub/c.rs"), "a.txt:2\nsub/c.rs:1\n");
    assert_eq!(out("grep -f pats a.txt"), "foo one\nfoo two\n");
    assert_eq!(out("printf 'x\\ny\\n' | grep -v x"), "y\n");
    assert_eq!(out("echo abcabd | grep -o 'ab[cd]'"), "abc\nabd\n");
    assert_eq!(out("echo xabcdy | grep -oE 'ab|abcd'"), "abcd\n");
    assert_eq!(out(r"echo xabab | grep -o '\(ab\)\1'"), "abab\n");
    assert_eq!(out("grep -A1 one a.txt"), "foo one\nbar\n");
}

#[test]
fn recursion() {
    // Hidden files are searched and .gitignore is not read; no `./` without a path.
    assert_eq!(
        out("grep -r foo"),
        ".hidden:foo hidden\na.txt:foo one\na.txt:foo two\nsub/c.rs:fn foo() {}\n"
    );
    assert_eq!(out("grep -rl foo ."), "./.hidden\n./a.txt\n./sub/c.rs\n");
    assert_eq!(out("grep -rl --include='*.rs' foo ."), "./sub/c.rs\n");
    assert_eq!(out("grep -rl --exclude-dir=sub foo"), ".hidden\na.txt\n");
}

#[test]
fn status_and_errors() {
    assert_eq!(inline("grep nope a.txt").0, 1);
    let (st, o, e) = inline("grep foo nope a.txt");
    assert_eq!((st, o.as_str()), (2, "a.txt:foo one\na.txt:foo two\n"));
    assert_eq!(e, "grep: nope: No such file or directory\n");
    assert_eq!(inline("grep -q foo nope a.txt").0, 0);
    assert_eq!(inline("grep -sq x nope").0, 2);
    let (st, o, e) = inline("grep zap bin");
    assert_eq!(
        (st, o.as_str(), e.as_str()),
        (0, "", "grep: bin: binary file matches\n")
    );
    assert_eq!(inline("grep -I zap bin").0, 1);
    let (st, _, e) = inline("grep 'a\\{1' a.txt");
    assert_eq!((st, e.as_str()), (2, "grep: Unmatched \\{\n"));
    let (st, _, e) = inline("grep");
    assert_eq!(st, 2);
    assert!(e.starts_with("Usage: grep [OPTION]... PATTERNS [FILE]...\n"));
}

#[test]
fn line_ends() {
    // CRLF is a line end on Windows (D030): `$` matches before it and spans leave the
    // CR out; -U keeps it part of the line. Elsewhere a CR is an ordinary character.
    let crlf = cfg!(windows);
    let n = |c: bool| if c { "1\n" } else { "0\n" };
    assert_eq!(out(r"printf 'ax\r\nb\r\n' | grep -c 'x$'"), n(crlf));
    assert_eq!(out(r"printf 'ax\r\nb\r\n' | grep -c -U 'x$'"), "0\n");
    assert_eq!(out(r"printf 'ax\r\nb\r\n' | grep -cx ax"), n(crlf));
    let o = out(r"printf 'ax\r\nb\r\n' | grep -o 'x.*'");
    assert_eq!(o, if crlf { "x\n" } else { "x\r\n" });
    let c = out(r"printf 'ax\r\n' | grep --color=always 'x.*'");
    let want = if crlf {
        "a\x1b[01;31m\x1b[Kx\x1b[m\x1b[K\r\n"
    } else {
        "a\x1b[01;31m\x1b[Kx\r\x1b[m\x1b[K\n"
    };
    assert_eq!(c, want);
    // -z: NUL ends a line and a newline is ordinary.
    assert_eq!(
        out(r"printf 'a\nb\0c\0' | grep -z 'a.b' | tr '\0' @"),
        "a\nb@"
    );
}

#[test]
fn falls_through() {
    // No real grep on the test host: what psh leaves to it fails clearly.
    let (st, _, e) = inline("grep -P '(?<=a)b' a.txt");
    assert_eq!(st, 2);
    assert_eq!(e, "grep: unsupported -P pattern (psh built-in grep)\n");
    let (st, _, e) = inline("grep -T foo a.txt");
    assert_eq!(st, 2);
    assert!(e.contains("unsupported option '-T'"), "{e}");
}
