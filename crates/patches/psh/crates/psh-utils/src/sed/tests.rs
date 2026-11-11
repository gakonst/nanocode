//! sed on the engine's test host (in-memory files), each script also run with every
//! host operation pending once. Behavior against GNU and BSD is L4 (tests/utils/sed).

use std::rc::Rc;

use psh_engine::testhost::TestHost;
use psh_engine::{Script, Shell, Step};

fn run_on(src: &str, stdin: &str, latency: bool) -> (i32, String, String, Rc<TestHost>) {
    let host = Rc::new(TestHost::new());
    host.set_latency(latency);
    host.set_stdin(stdin.as_bytes());
    host.fs.write_file("/tmp/f", b"one\ntwo\nthree\n");
    let mut cfg = host.config();
    cfg.utilities = crate::policy(false);
    let mut shell = Shell::new(cfg);
    shell.start(Script::command(src));
    let status = loop {
        match shell.step(13) {
            Step::Done(s) => break s,
            Step::Yield => {}
            other => panic!("{other:?} running {src:?}"),
        }
    };
    let out = String::from_utf8_lossy(&host.stdout()).into_owned();
    let err = String::from_utf8_lossy(&host.stderr()).into_owned();
    (status, out, err, host)
}

#[track_caller]
fn check(src: &str, stdin: &str, want: &str) {
    let a = run_on(src, stdin, false);
    let b = run_on(src, stdin, true);
    assert_eq!(
        (a.0, &a.1, &a.2),
        (b.0, &b.1, &b.2),
        "latency changed {src:?}"
    );
    assert_eq!(a.1, want, "stdout of {src:?} (stderr: {})", a.2);
}

#[track_caller]
fn file_after(src: &str, path: &str) -> String {
    let (_, _, err, host) = run_on(src, "", false);
    let data = host
        .fs
        .read_file(path)
        .unwrap_or_else(|| panic!("{path} after {src:?}: {err}"));
    String::from_utf8_lossy(&data).into_owned()
}

#[test]
fn commands() {
    check("sed 's/o/0/g' /tmp/f", "", "0ne\ntw0\nthree\n");
    check("sed -n '2p' /tmp/f", "", "two\n");
    check("sed '$!N;s/\\n/+/' /tmp/f", "", "one+two\nthree\n");
    check("sed -n '1!G;h;$p' /tmp/f", "", "three\ntwo\none\n");
    check("sed ':a;N;$!ba;s/\\n/ /g' /tmp/f", "", "one two three\n");
    check("sed '2i\\\nnew' /tmp/f", "", "one\nnew\ntwo\nthree\n");
    check("sed '1a after' /tmp/f", "", "one\nafter\ntwo\nthree\n");
    check("sed '2,3c X' /tmp/f", "", "one\nX\n");
    check("sed -n '/two/=' /tmp/f", "", "2\n");
    check("sed y/abc/xyz/", "abcabc\n", "xyzxyz\n");
    check("sed -E 's/(\\w+)/[\\U\\1]/'", "hi there\n", "[HI] there\n");
    check("sed 2q /tmp/f", "", "one\ntwo\n");
    check("sed -n l", "a\tb\n", "a\\tb$\n");
    check("sed p", "a", "a\na");
    check("sed 's/x*/-/g'", "abc\n", "-a-b-c-\n");
    check("sed -s -n '$p' /tmp/f /tmp/f", "", "three\nthree\n");
    check("sed -z 's/^/>/'", "a\0b\0", ">a\0>b\0");
    // e runs shell commands (a fresh shell, as sh -c).
    check("sed '1e echo hi' /tmp/f", "", "hi\none\ntwo\nthree\n");
    check("sed 's/.*/echo [&]/e'", "x\n", "[x]\n");
    check("sed e", "echo a; echo b\n", "a\nb\n");
}

#[test]
fn errors() {
    let (st, _, err, _) = run_on("sed 's/a/b'", "", false);
    assert_eq!(
        (st, err.as_str()),
        (
            1,
            "sed: -e expression #1, char 5: unterminated `s' command\n"
        )
    );
    let (st, _, err, _) = run_on("sed -e p -e k", "", false);
    assert_eq!(
        (st, err.as_str()),
        (1, "sed: -e expression #2, char 1: unknown command: `k'\n")
    );
    let (st, _, err, _) = run_on("sed 'b x'", "", false);
    assert_eq!(
        (st, err.as_str()),
        (4, "sed: can't find label for jump to `x'\n")
    );
    let (st, out, err, _) = run_on("sed p /nosuch /tmp/f", "", false);
    assert_eq!(st, 2);
    assert_eq!(err, "sed: can't read /nosuch: No such file or directory\n");
    assert_eq!(out.lines().count(), 6);
}

#[test]
fn in_place() {
    assert_eq!(
        file_after("sed -i s/one/1/ /tmp/f", "/tmp/f"),
        "1\ntwo\nthree\n"
    );
    assert_eq!(
        file_after("sed -i '' s/one/1/ /tmp/f", "/tmp/f"),
        "1\ntwo\nthree\n"
    );
    assert_eq!(
        file_after("sed -i -e s/one/1/ /tmp/f", "/tmp/f"),
        "1\ntwo\nthree\n"
    );
    assert_eq!(
        file_after("sed -i.bak s/one/1/ /tmp/f", "/tmp/f.bak"),
        "one\ntwo\nthree\n"
    );
    assert_eq!(
        file_after("sed -i .orig s/one/1/ /tmp/f", "/tmp/f.orig"),
        "one\ntwo\nthree\n"
    );
    assert_eq!(
        file_after("sed -i .o -e s/one/1/ /tmp/f", "/tmp/f.o"),
        "one\ntwo\nthree\n"
    );
    assert_eq!(file_after("sed -i 2q /tmp/f", "/tmp/f"), "one\ntwo\n");
    let (st, _, err, _) = run_on("sed -i s/a/b/", "", false);
    assert_eq!((st, err.as_str()), (4, "sed: no input files\n"));
}

/// Every branch of D016's rule for a bare `-i` (D1141).
#[test]
fn in_place_forms() {
    let suffix = |args: &[&str]| -> Option<Vec<u8>> {
        let argv: Vec<Vec<u8>> = std::iter::once("sed")
            .chain(args.iter().copied())
            .map(|a| a.as_bytes().to_vec())
            .collect();
        match super::parse_args(&argv, true) {
            Ok(o) => o.in_place,
            Err(_) => panic!("{args:?}"),
        }
    };
    let s = |v: &str| Some(v.as_bytes().to_vec());
    // `''` next: BSD's empty suffix.
    assert_eq!(suffix(&["-i", "", "s/a/b/", "f"]), s(""));
    assert_eq!(suffix(&["-i", "", "-e", "s/a/b/", "f"]), s(""));
    // Attached: a suffix in both.
    assert_eq!(suffix(&["-i.bak", "s/a/b/", "f"]), s(".bak"));
    assert_eq!(suffix(&["--in-place=.bak", "s/a/b/", "f"]), s(".bak"));
    // A script with operands after it: GNU.
    assert_eq!(suffix(&["-i", "s/a/b/", "f"]), s(""));
    assert_eq!(suffix(&["-i", "1d", "f", "g"]), s(""));
    // An option next: GNU.
    assert_eq!(suffix(&["-i", "-e", "s/a/b/", "f"]), s(""));
    assert_eq!(suffix(&["-n", "-i", "-n", "p", "f"]), s(""));
    // A dotted word without command characters, then a script: BSD.
    assert_eq!(suffix(&["-i", ".bak", "s/a/b/", "f"]), s(".bak"));
    assert_eq!(suffix(&["-i", ".bak", "-e", "s/a/b/", "f"]), s(".bak"));
    assert_eq!(suffix(&["-Ei", ".bak", "s/a+/b/", "f"]), s(".bak"));
    // With -e, a plain word is a file.
    assert_eq!(suffix(&["-e", "p", "-i", "f"]), s(""));
    // Nothing after: GNU (and "no input files").
    assert_eq!(suffix(&["-i", "s/a/b/"]), s(""));
}
