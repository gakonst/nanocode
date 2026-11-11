//! fd in-process over the test host (the inline backend and the virtual walk).
//! Behavior against the real fd is L4 (tests/utils/fd).

use std::rc::Rc;

use psh_engine::testhost::TestHost;
use psh_engine::{Script, Shell, Step};

fn inline(src: &str) -> (i32, String, String) {
    let host = Rc::new(TestHost::new());
    let fs = &host.fs;
    fs.write_file("/w/.git/HEAD", b"ref\n");
    fs.write_file("/w/.gitignore", b"*.log\n");
    fs.write_file("/w/a.txt", b"x\n");
    fs.write_file("/w/b.rs", b"x\n");
    fs.write_file("/w/c.log", b"x\n");
    fs.write_file("/w/.hid/h.txt", b"x\n");
    fs.write_file("/w/src/main.rs", b"x\n");
    fs.write_file("/w/src/sub/x.TXT", b"x\n");
    fs.write_file("/w/empty", b"");
    fs.write_file("/w/-dash", b"x\n");
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
    let (st, o, e) = inline(src);
    assert_eq!((st, e.as_str()), (0, ""), "status and stderr of {src:?}");
    o
}

#[test]
fn listing_and_filters() {
    assert_eq!(
        out("fd"),
        "./-dash\na.txt\nb.rs\nempty\nsrc/\nsrc/main.rs\nsrc/sub/\nsrc/sub/x.TXT\n"
    );
    assert_eq!(out("fd rs"), "b.rs\nsrc/main.rs\n");
    assert_eq!(out("fd -e txt"), "a.txt\nsrc/sub/x.TXT\n");
    assert_eq!(out("fd TXT"), "src/sub/x.TXT\n");
    assert_eq!(out("fd -tf -d1"), "./-dash\na.txt\nb.rs\nempty\n");
    assert_eq!(out("fd -td"), "src/\nsrc/sub/\n");
    assert_eq!(out("fd -te"), "empty\n");
    assert_eq!(out("fd -H h.txt"), ".hid/h.txt\n");
    assert_eq!(out("fd -I log"), "c.log\n");
    assert_eq!(out("fd -g '*.rs' src"), "src/main.rs\n");
    assert_eq!(out("fd . src"), "src/main.rs\nsrc/sub/\nsrc/sub/x.TXT\n");
    assert_eq!(out("fd -E src -E '*dash'"), "a.txt\nb.rs\nempty\n");
    assert_eq!(
        out("fd --min-depth 2"),
        "src/main.rs\nsrc/sub/\nsrc/sub/x.TXT\n"
    );
    assert_eq!(out("fd -S -1b"), "empty\n");
    assert_eq!(out("fd -0 rs"), "./b.rs\0./src/main.rs\0");
    assert_eq!(out("fd --format '{/.}|{//}' rs"), "b|.\nmain|src\n");
    assert_eq!(out("fd -p 'src/.*rs$'"), "src/main.rs\n");
    assert_eq!(out("fd -C src rs"), "main.rs\n");
    assert_eq!(out("fd --path-separator '#' x.T"), "src#sub#x.TXT\n");
}

#[test]
fn statuses_and_errors() {
    assert_eq!(inline("fd -q rs").0, 0);
    assert_eq!(inline("fd -q zzz").0, 1);
    assert_eq!(inline("fd zzz"), (0, String::new(), String::new()));
    let (st, _, e) = inline("fd x nope");
    assert_eq!(st, 1);
    assert_eq!(
        e,
        "[fd error]: Search path \"nope\" is not a directory.\n[fd error]: No valid search \
         paths given.\n"
    );
    let (st, _, e) = inline("fd src/main");
    assert_eq!(st, 1);
    assert!(
        e.starts_with("[fd error]: The search pattern 'src/main' contains"),
        "{e}"
    );
    let (st, _, e) = inline("fd -t z");
    assert_eq!(st, 2);
    assert!(
        e.starts_with("error: invalid value 'z' for '--type <filetype>'"),
        "{e}"
    );
    let (st, _, e) = inline("fd -g -F x");
    assert_eq!(st, 2);
    assert_eq!(
        e,
        "error: the argument '--glob' cannot be used with '--fixed-strings'\n\nUsage: fd \
         --glob <pattern> [path]...\n\nFor more information, try '--help'.\n"
    );
    let (st, _, e) = inline("fd '('");
    assert_eq!(st, 1);
    assert!(e.starts_with("[fd error]: regex parse error:"), "{e}");
    let (st, _, e) = inline("fd -X echo {} {}");
    assert_eq!(st, 2);
    assert!(e.starts_with("error: Only one placeholder allowed"), "{e}");
}

#[test]
fn commands() {
    assert_eq!(out("fd rs -x echo"), "./b.rs\n./src/main.rs\n");
    assert_eq!(out("fd rs -x echo {/} \\; -H"), "b.rs\nmain.rs\n");
    assert_eq!(out("fd rs -X echo"), "./b.rs ./src/main.rs\n");
    assert_eq!(out("fd rs -X echo -- {.} end"), "-- b src/main end\n");
    assert_eq!(
        out("fd rs --batch-size 1 -X echo"),
        "./b.rs\n./src/main.rs\n"
    );
    assert_eq!(out("fd rs -x echo -0"), "-0 ./b.rs\n-0 ./src/main.rs\n");
    assert_eq!(out("fd -0 rs -x echo"), "./b.rs\n\0./src/main.rs\n\0");
    // Results are relative to the base directory, where the commands run too.
    assert_eq!(out("fd -C src rs -x pwd"), "/w/src\n");
    let (st, o, e) = inline("fd rs -x nonexist");
    assert_eq!((st, o.as_str()), (1, ""));
    assert_eq!(e, "[fd error]: Command not found: \"nonexist\"\n".repeat(2));
    assert_eq!(inline("fd rs -x false").0, 1);
    assert_eq!(inline("fd zzz -X false").0, 0);
}

#[test]
fn help_and_version() {
    assert_eq!(out("fd --version"), "fd 10.5.1\n");
    assert!(out("fd -h").starts_with("A program to find entries"));
    assert!(out("fd --help").contains("Bugs can be reported on GitHub"));
}

#[test]
fn command_line() {
    use super::cli::{Parsed, parse};
    let args = |s: &[&str]| s.iter().map(|a| a.as_bytes().to_vec()).collect::<Vec<_>>();
    let Ok(Parsed::Run(o)) = parse(&args(&["-Hx", "echo", "{}", ";", "-d3", "pat"])) else {
        panic!()
    };
    assert!(o.hidden && !o.exec_batch);
    assert_eq!(o.exec, vec![args(&["echo", "{}"])]);
    assert_eq!(
        (o.max_depth, o.pattern.as_deref()),
        (Some(3), Some(&b"pat"[..]))
    );
    let Ok(Parsed::Run(o)) = parse(&args(&["-u", "--no-hidden", "-S", "-1k", "--and", "-x"]))
    else {
        panic!()
    };
    assert!(!o.hidden && o.no_ignore && o.exprs == args(&["-x"]) && o.size.len() == 1);
    let Ok(Parsed::Run(o)) = parse(&args(&["-g", "--regex", "-F", "-s", "-i"])) else {
        panic!()
    };
    assert!(!o.glob && o.fixed_strings && o.ignore_case && !o.case_sensitive);
}
