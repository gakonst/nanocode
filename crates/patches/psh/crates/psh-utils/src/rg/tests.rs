//! rg in-process: inline over the test host (the virtual walk), and on threads over
//! the real file system, two at once. Behavior against the real rg is L4
//! (tests/utils/rg).

use std::rc::Rc;

use psh_engine::testhost::TestHost;
use psh_engine::{Script, Shell, Step};

/// Runs `src` on the test host with a small tree under /w (a git repository).
fn inline(src: &str) -> (i32, String, String) {
    let host = Rc::new(TestHost::new());
    let fs = &host.fs;
    fs.mkdir_all("/w/.git");
    fs.write_file("/w/.gitignore", b"*.log\n");
    fs.write_file("/w/a.txt", b"foo one\nbar\nfoo two\n");
    fs.write_file("/w/b.log", b"foo ignored\n");
    fs.write_file("/w/.hidden", b"foo hidden\n");
    fs.write_file("/w/sub/c.rs", b"fn foo() {}\n");
    fs.write_file("/w/sub/.ignore", b"skip.rs\n");
    fs.write_file("/w/sub/skip.rs", b"foo skipped\n");
    fs.write_file("/w/pats", b"one\ntwo\n");
    fs.write_file("/w/rc", b"# config\n--line-number\n");
    fs.write_file("/w/crlf", b"ax\r\nb\r\n");
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
fn virtual_walk() {
    // stdin is /dev/null: rg searches the cwd, sorted, without `./`.
    assert_eq!(
        out("rg foo </dev/null"),
        "a.txt:foo one\na.txt:foo two\nsub/c.rs:fn foo() {}\n"
    );
    assert_eq!(out("rg -l foo . </dev/null"), "./a.txt\n./sub/c.rs\n");
    assert_eq!(
        out("rg --files --hidden </dev/null"),
        ".gitignore\n.hidden\na.txt\ncrlf\npats\nrc\nsub/.ignore\nsub/c.rs\n"
    );
    assert_eq!(
        out("rg -c foo -uu </dev/null"),
        ".hidden:1\na.txt:2\nb.log:1\nsub/c.rs:1\nsub/skip.rs:1\n"
    );
    // A glob whitelists past ignore files, as in ripgrep.
    assert_eq!(
        out("rg -g '*.rs' foo </dev/null"),
        "sub/c.rs:fn foo() {}\nsub/skip.rs:foo skipped\n"
    );
    assert_eq!(out("rg -g '!*.txt' foo </dev/null"), "sub/c.rs:fn foo() {}\n");
    assert_eq!(out("rg -t rust -n foo sub"), "sub/c.rs:1:fn foo() {}\n");
    assert_eq!(out("cd sub && rg foo </dev/null"), "c.rs:fn foo() {}\n");
    // One file: no file name.
    assert_eq!(out("rg -n two a.txt"), "3:foo two\n");
    assert_eq!(out("rg -f pats a.txt"), "foo one\nfoo two\n");
}

#[test]
fn stdin_config_and_status() {
    assert_eq!(out("echo xfoo | rg foo"), "xfoo\n");
    assert_eq!(
        out("echo xfoo | rg foo - a.txt"),
        "<stdin>:xfoo\na.txt:foo one\na.txt:foo two\n"
    );
    assert_eq!(out("echo one | rg -f - a.txt"), "foo one\n");
    assert_eq!(out("RIPGREP_CONFIG_PATH=/w/rc rg two a.txt"), "3:foo two\n");
    assert_eq!(
        out("RIPGREP_CONFIG_PATH=/w/rc rg --no-config two a.txt"),
        "foo two\n"
    );
    assert_eq!(
        inline("RIPGREP_CONFIG_PATH=/w/nope rg two a.txt"),
        (0, "foo two\n".into(), "rg: failed to read the file specified in RIPGREP_CONFIG_PATH: /w/nope: entity not found\n".into())
    );
    assert_eq!(inline("rg zzz a.txt"), (1, String::new(), String::new()));
    let (st, o, e) = inline("rg foo nope a.txt");
    assert_eq!((st, o.as_str()), (2, "a.txt:foo one\na.txt:foo two\n"));
    assert!(e.starts_with("rg: nope: "), "{e}");
    assert_eq!(inline("rg -q foo nope a.txt").0, 0);
    assert_eq!(inline("rg '(' a.txt").0, 2);
    assert_eq!(
        inline("rg"),
        (
            2,
            String::new(),
            "rg: ripgrep requires at least one pattern to execute a search\n"
                .into()
        )
    );
    assert!(out("rg --version").starts_with("ripgrep 15.2.0 (psh)\n"));
    assert!(out("rg -h").starts_with("ripgrep 15.2.0 (psh)\nAndrew Gallant"));
}

/// CRLF is a line end for matching on Windows (D030); output stays `\n` there.
#[test]
fn crlf() {
    let windows = cfg!(windows);
    assert_eq!(out("rg -c 'x$' crlf"), if windows { "1\n" } else { "" });
    assert_eq!(out("rg -c --crlf 'x$' crlf"), "1\r\n");
    assert_eq!(out("rg -c --no-crlf 'x$' crlf"), "");
    assert_eq!(out("rg -l x crlf a.txt"), "crlf\n");
    assert_eq!(
        out("rg -o 'a.*' crlf"),
        if windows { "ax\n" } else { "ax\r\n" }
    );
}

/// `--json` through deser-json: grep-printer's messages, rg's sorted summary.
#[test]
fn json() {
    let o = out("rg --json -n two a.txt");
    let lines: Vec<&str> = o.lines().collect();
    assert_eq!(lines.len(), 4, "{o}");
    assert_eq!(
        lines[0],
        r#"{"type":"begin","data":{"path":{"text":"a.txt"}}}"#
    );
    assert_eq!(
        lines[1],
        r#"{"type":"match","data":{"path":{"text":"a.txt"},"lines":{"text":"foo two\n"},"line_number":3,"absolute_offset":12,"submatches":[{"match":{"text":"two"},"start":4,"end":7}]}}"#
    );
    assert!(lines[2].starts_with(r#"{"type":"end","data":{"path":{"text":"a.txt"},"binary_offset":null,"stats":{"elapsed":{"secs":"#));
    assert!(lines[3].starts_with(r#"{"data":{"elapsed_total":{"human":"#));
    assert!(lines[3].ends_with(r#""matched_lines":1,"matches":1,"searches":1,"searches_with_match":1}},"type":"summary"}"#));
}

#[test]
fn falls_through_without_a_program() {
    // The test host has no rg program.
    for (args, what) in [
        ("--pre cat foo", "--pre"),
        ("-z foo", "-z/--search-zip"),
        ("-P foo", "-P/--pcre2"),
        ("--debug foo", "--debug/--trace"),
    ] {
        assert_eq!(
            inline(&format!("rg {args} a.txt")),
            (
                2,
                String::new(),
                format!("rg: unsupported {what} (psh built-in rg)\n")
            )
        );
    }
}

/// The thread backend: the real file system, the native walk.
#[cfg(unix)]
fn threaded(dir: &std::path::Path, src: &str) -> i32 {
    let mut cfg = psh_engine::ShellConfig::new(Rc::new(
        psh_engine::std_host::StdHost::new(),
    ));
    cfg.utilities = crate::policy(false);
    cfg.cwd = dir.to_string_lossy().into_owned().into_bytes();
    cfg.stdin = None;
    cfg.stdout = None;
    let mut shell = Shell::new(cfg);
    shell.start(Script::command(src));
    psh_engine::std_host::drive_until(
        &mut shell,
        Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
    )
    .expect("the script timed out")
}

/// Two rg runs at once in one process keep their own message and error state: the
/// one that errors (and has messages off) does not change the other's status or
/// stderr, and the parallel search prints in walk order.
#[cfg(unix)]
#[test]
fn concurrent_runs() {
    let dir =
        std::env::temp_dir().join(format!("psh-rg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for i in 0..40 {
        let d = dir.join(format!("t/d{}", i % 7));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("f{i:02}")), format!("x\nfoo {i}\n"))
            .unwrap();
    }
    let read = |name: &str| {
        std::fs::read_to_string(dir.join(name)).unwrap_or_default()
    };
    let src = "{ sleep 0.3; echo foo; } | rg foo > o1 2> e1 & \
               rg --no-messages -j4 foo t nope > o2 2> e2 & \
               rg -j1 foo t > o3; wait %1; echo $? > s1; wait %2; echo $? > s2";
    assert_eq!(threaded(&dir, src), 0);
    assert_eq!(
        (read("o1"), read("e1"), read("s1")),
        ("foo\n".into(), String::new(), "0\n".into())
    );
    assert_eq!((read("e2"), read("s2")), (String::new(), "2\n".into()));
    // Multi-threaded output is the single-threaded output, in walk order.
    assert_eq!(read("o2"), read("o3"));
    assert_eq!(read("o3").lines().count(), 40);
    let mut sorted: Vec<&str> = Vec::new();
    let o3 = read("o3");
    for l in o3.lines() {
        sorted.push(l);
    }
    let mut want = sorted.clone();
    want.sort();
    assert_eq!(sorted, want);
    // A closed pipe ends the search with status 0.
    assert_eq!(
        threaded(
            &dir,
            "rg -j4 x t | head -1 > o4; echo ${PIPESTATUS[0]} > s4"
        ),
        0
    );
    assert_eq!((read("o4").lines().count(), read("s4")), (1, "0\n".into()));
    let _ = std::fs::remove_dir_all(&dir);
}
