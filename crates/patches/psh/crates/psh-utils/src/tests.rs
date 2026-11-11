//! The utilities on the engine's test host (in-memory fs, fake programs), each script
//! also run with every host operation pending once. Behavior against real GNU and BSD
//! is L4 (tests/utils, psh-spec).

use std::rc::Rc;

use psh_engine::testhost::TestHost;
use psh_engine::{Script, Shell, Step};

struct Out {
    status: i32,
    stdout: String,
    stderr: String,
}

fn run_on(src: &str, stdin: &[u8], latency: bool) -> Out {
    let host = Rc::new(TestHost::new());
    host.set_latency(latency);
    host.set_stdin(stdin);
    host.fs.write_file("/tmp/f", b"one\ntwo\nthree\nfour\n");
    host.fs.write_file("/tmp/g", b"a b c\n");
    host.fs.write_file("/tmp/nonl", b"x\ny");
    host.fs.mkdir_all("/tmp/dir");
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
    assert_eq!(shell.task_count(), 0, "tasks left after {src:?}");
    Out {
        status,
        stdout: String::from_utf8_lossy(&host.stdout()).into_owned(),
        stderr: String::from_utf8_lossy(&host.stderr()).into_owned(),
    }
}

fn run_in(src: &str, stdin: &str) -> Out {
    let a = run_on(src, stdin.as_bytes(), false);
    let b = run_on(src, stdin.as_bytes(), true);
    assert_eq!(
        (a.status, &a.stdout, &a.stderr),
        (b.status, &b.stdout, &b.stderr),
        "latency changed {src:?}"
    );
    a
}

#[track_caller]
fn check(src: &str, stdout: &str) {
    let o = run_in(src, "");
    assert_eq!(o.stdout, stdout, "stdout of {src:?} (stderr: {})", o.stderr);
}

#[track_caller]
fn check_all(src: &str, stdin: &str, status: i32, stdout: &str, stderr: &str) {
    let o = run_in(src, stdin);
    assert_eq!(
        (o.status, o.stdout.as_str(), o.stderr.as_str()),
        (status, stdout, stderr),
        "{src:?}"
    );
}

/// Utilities the classifier handles with rules in code (wrappers and the like run other
/// commands), not with a table entry (D1090).
const CLASSIFIED_IN_CODE: &[&str] = &["timeout", "env", "xargs", "nice", "nohup", "find"];

#[test]
fn specs_match_the_classifier() {
    let mut problems = Vec::new();
    for spec in crate::SPECS {
        if CLASSIFIED_IN_CODE.contains(&spec.name) {
            continue;
        }
        problems.extend(crate::opts::check_against_classifier(spec));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Utilities with an option parser of their own (copied with the utility), checked
/// against the classifier from their own flag tables.
const OWN_PARSER: &[&str] = &[
    "rg", "git", "tar", "gzip", "zip", "unzip", "patch", "xxd", "od", "hexdump", "strings",
    "base32", "tac", "rev", "fold", "expand", "unexpand", "column", "join", "shuf", "split",
    "expr", "factor", "envsubst", "truncate", "sponge", "yes", "uuidgen",
];

#[test]
fn force_names_from_the_variable() {
    // `all` is every F/W utility; S utilities (cat, awk) run in-process anyway.
    let all = crate::force_names(b"all");
    assert!(!all.contains(&b"cat".to_vec()) && !all.contains(&b"awk".to_vec()));
    assert_eq!(
        crate::force_names(b"awk,,sed"),
        vec![b"awk".to_vec(), b"sed".to_vec()]
    );
}

#[test]
fn every_entry_has_a_spec() {
    for e in crate::REGISTRY {
        assert!(
            crate::SPECS.iter().any(|s| s.name == e.names[0]) || OWN_PARSER.contains(&e.names[0]),
            "{}",
            e.names[0]
        );
    }
}

/// rg's flags with values (from ripgrep's table) are what the classifier knows (D1090).
#[test]
fn rg_flags_match_the_classifier() {
    let (short, long) = crate::rg::value_flags();
    let u = psh_classify::utils::utility("rg").expect("rg in psh_classify::utils");
    let mut want: Vec<char> = u.short_values.chars().collect();
    want.sort_unstable();
    let want: String = want.into_iter().collect();
    let mut want_long: Vec<String> = u.long_values.iter().map(|s| s.to_string()).collect();
    want_long.sort();
    assert_eq!((short, long), (want, want_long));
}

#[test]
fn cat() {
    check("cat f", "one\ntwo\nthree\nfour\n");
    check_all("cat", "in\n", 0, "in\n", "");
    check("cat g - nonl", "a b c\nx\ny");
    check("echo mid | cat g - g", "a b c\nmid\na b c\n");
    // /dev/fd/N names the shell's descriptor: a process substitution, a redirection.
    check("cat <(echo sub)", "sub\n");
    check("cat /dev/fd/3 3<g", "a b c\n");
    check("cat -n g nonl", "     1\ta b c\n     2\tx\n     3\ty");
    check("printf 'a\\n\\n\\n\\nb\\n' | cat -s", "a\n\nb\n");
    check("printf 'a\\n\\nb\\n' | cat -b", "     1\ta\n\n     2\tb\n");
    check("printf 'a\\tb\\001\\n' | cat -A", "a^Ib^A$\n");
    check("printf 'a\\tb\\n' | cat -T", "a^Ib\n");
    check("printf 'a\\n' | cat -E", "a$\n");
    check_all(
        "cat missing g; echo $?",
        "",
        0,
        "a b c\n1\n",
        "cat: missing: No such file or directory\n",
    );
    check_all("cat dir", "", 1, "", "cat: dir: Is a directory\n");
}

#[test]
fn head() {
    check("head -n 2 f", "one\ntwo\n");
    check("head -2 f", "one\ntwo\n");
    check("head -c 5 f", "one\nt");
    check("head -n -1 f", "one\ntwo\nthree\n");
    check("head -c -15 f", "one\n");
    check(
        "head f g",
        "==> f <==\none\ntwo\nthree\nfour\n\n==> g <==\na b c\n",
    );
    check("head -q -n1 f g", "one\na b c\n");
    check("head -v -n1 g", "==> g <==\na b c\n");
    check_all("printf 'a\\nb\\nc\\n' | head -n1", "", 0, "a\n", "");
    // head stops reading: the writer gets EPIPE, nothing hangs.
    check(
        "i=0; while :; do echo $i; i=$((i+1)); done | head -n 3",
        "0\n1\n2\n",
    );
    check_all(
        "head -n x f",
        "",
        1,
        "",
        "head: invalid number of lines: 'x'\n",
    );
    // An unknown option runs the real program (the test host has a fake `head`), or
    // fails clearly when there is none.
    check_all("head --frobnicate f", "", 0, "", "");
    check_all(
        "tail --frobnicate f",
        "",
        2,
        "",
        "tail: unsupported option '--frobnicate' (psh built-in tail)\n",
    );
    check_all(
        "head -n",
        "",
        1,
        "",
        "head: option requires an argument -- 'n'\nTry 'head --help' for more information.\n",
    );
}

#[test]
fn tail() {
    check("tail -n 2 f", "three\nfour\n");
    check("tail -1 f", "four\n");
    check("tail -n +3 f", "three\nfour\n");
    check("tail +3 f", "three\nfour\n");
    check("tail -c 3 f", "ur\n");
    check("tail -c +17 f", "ur\n");
    check("tail -r f", "four\nthree\ntwo\none\n");
    check("tail -n 2 -r f", "four\nthree\n");
    check("tail nonl", "x\ny");
    check("echo q | tail", "q\n");
    check("tail -n1 f g", "==> f <==\nfour\n\n==> g <==\na b c\n");
}

#[test]
fn wc() {
    check("wc f", " 4  4 19 f\n");
    check("wc -l f", "4 f\n");
    check("wc -l < f", "4\n");
    check("printf 'a b\\nc\\n' | wc", "      2       3       6\n");
    check("printf 'x\\n' | wc -l", "1\n");
    check("wc f g", " 4  4 19 f\n 1  3  6 g\n 5  7 25 total\n");
    check("cat f | wc -c f -", "     19 f\n     19 -\n     38 total\n");
    check("wc -L f", "5 f\n");
    check("wc --total=only -l f g", "5\n");
}

#[test]
fn env() {
    check("env -i B=2 env", "B=2\n");
    check("env -i", "");
    check("X=1 env -u X sh_not_there=1 printf '%s\n' ok", "ok\n");
    check("env -i A=1 B=2 printf '%s\n' x", "x\n");
    check_all(
        "f() { echo fn; }; env f",
        "",
        127,
        "",
        "psh: line 1: f: command not found\n",
    );
    check("env - C=3 env", "C=3\n");
}

#[test]
fn sleep_and_timeout() {
    check_all("sleep 0.01; echo done", "", 0, "done\n", "");
    check_all(
        "sleep 1m2",
        "",
        1,
        "",
        "sleep: invalid time interval '1m2'\nTry 'sleep --help' for more information.\n",
    );
    // The test host's time is virtual (a sleep ends at once): `hang` never ends.
    check_all("timeout 0.05 hang; echo $?", "", 0, "124\n", "");
    check_all("timeout -s KILL 0.05 hang; echo $?", "", 0, "137\n", "");
    check_all(
        "timeout --preserve-status 0.05 hang; echo $?",
        "",
        0,
        "143\n",
        "",
    );
    check_all("timeout 5 echo fast; echo $?", "", 0, "fast\n0\n", "");
    check_all("timeout 1 false; echo $?", "", 0, "1\n", "");
    check_all("gtimeout 0 true; echo $?", "", 0, "0\n", "");
    check_all(
        "timeout -v 0.01 hang",
        "",
        124,
        "",
        "timeout: sending signal TERM to command 'hang'\n",
    );
    check_all(
        "timeout x sleep 1",
        "",
        125,
        "",
        "timeout: invalid time interval 'x'\nTry 'timeout --help' for more information.\n",
    );
    // An infinite in-process loop is stopped too.
    check_all(
        "timeout 0.05 tail -f f > /dev/null; echo $?",
        "",
        0,
        "124\n",
        "",
    );
}

mod files;

// The text filters (sort uniq cut tr paste nl tee seq comm).
#[path = "tests_text.rs"]
mod text;

#[test]
fn xargs() {
    check_all("xargs echo", "a 'b c'\nd\\ e\n", 0, "a b c d e\n", "");
    check_all("xargs -n1 echo x", "a b\n", 0, "x a\nx b\n", "");
    check_all(
        "xargs -I{} echo [{}]",
        "  one two\nthree\n",
        0,
        "[one two]\n[three]\n",
        "",
    );
    check_all("xargs -L2", "a\nb\nc\n", 0, "a b\nc\n", "");
    check_all("xargs -0 echo", "a b\0c\0", 0, "a b c\n", "");
    check_all("xargs -d, -n2 echo", "a,b,c", 0, "a b\nc\n", "");
    check_all("xargs -E X echo", "a X b\n", 0, "a\n", "");
    // Empty input runs nothing (D1170).
    check_all("xargs echo ran", "\n", 0, "", "");
    check_all("xargs false", "a\n", 123, "", "");
    check_all("xargs -P2 -n1 echo", "a\n", 0, "a\n", "");
    check_all(
        "xargs echo",
        "it's\n",
        1,
        "",
        "xargs: unmatched single quote; by default quotes are special to xargs unless you use the -0 option\n",
    );
    check_all("xargs -t echo", "a\n", 0, "a\n", "echo a\n");
}

#[test]
fn find() {
    check_all(
        "cd /tmp && find . -maxdepth 1 -type f",
        "",
        0,
        "./f\n./g\n./nonl\n",
        "",
    );
    check_all("find /tmp -name dir", "", 0, "/tmp/dir\n", "");
    check_all(
        "find /tmp -name f -exec cat {} \\;",
        "",
        0,
        "one\ntwo\nthree\nfour\n",
        "",
    );
    check_all(
        "find /tmp -type f -exec wc -l {} + | sort",
        "",
        0,
        " 1 /tmp/g\n 1 /tmp/nonl\n 4 /tmp/f\n 6 total\n",
        "",
    );
    check_all(
        "find /tmp/dir -printf '%f %d %y\\n'",
        "",
        0,
        "dir 0 d\n",
        "",
    );
    check_all(
        "find /tmp/nosuch; echo $?",
        "",
        0,
        "1\n",
        "find: '/tmp/nosuch': No such file or directory\n",
    );
    // No real find on the test host: an unknown primary fails clearly.
    check_all(
        "find /tmp -fancy",
        "",
        2,
        "",
        "find: unsupported option '-fancy' (psh built-in find)\n",
    );
    check_all(
        "cd /tmp && find . -name 'f' -o -name g | xargs echo",
        "",
        0,
        "./f ./g\n",
        "",
    );
}

#[test]
fn xargs_unsupported() {
    check_all(
        "xargs -p echo",
        "a\n",
        2,
        "",
        "xargs: unsupported option '-p' (psh built-in xargs)\n",
    );
}

// base64, the checksums, cmp, which, printenv, env's flags (0020).
#[path = "tests_misc.rs"]
mod misc;

#[test]
fn jq() {
    check_all("jq -c .a", "{\"a\":[1,2]}\n", 0, "[1,2]\n", "");
    check_all("jq -r '.[]'", "[\"x\",1]\n", 0, "x\n1\n", "");
    check_all("jq -n '1+1'", "", 0, "2\n", "");
    // Files: a blocking utility on the inline backend cannot wait for a slow host, so
    // this runs without the latency pass.
    let o = run_on("jq -c . /tmp/nonl", b"", false);
    assert_eq!(
        (o.status, o.stdout.as_str(), o.stderr.as_str()),
        (
            5,
            "",
            "jq: parse error: Invalid numeric literal at line 2, column 0\n"
        )
    );
    check_all(
        "jq .a.b",
        "{\"a\":1}\n",
        5,
        "",
        "jq: error (at <stdin>:1): Cannot index number with string \"b\"\n",
    );
    check_all("jq -n -e false", "", 1, "false\n", "");
    check_all(
        "jq -n 'nosuch'",
        "",
        3,
        "",
        "jq: error: nosuch/0 is not defined at <top-level>, line 1:\nnosuch\njq: 1 compile error\n",
    );
    // An option psh does not implement, with no jq on the test host.
    check_all(
        "jq --frobnicate .",
        "",
        2,
        "",
        "jq: unsupported option '--frobnicate' (psh built-in jq)\n",
    );
}
