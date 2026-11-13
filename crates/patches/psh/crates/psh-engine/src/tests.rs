//! Engine tests on the test host. Every script runs twice, once with latency (every
//! host operation `Pending` once): both runs must agree.

use std::rc::Rc;

use crate::testhost::TestHost;
use crate::{Script, Shell, Step};

pub(crate) struct Out {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

fn run_on(host: Rc<TestHost>, src: &str) -> Out {
    let mut shell = Shell::new(host.config());
    shell.start(Script::command(src));
    let mut steps = 0;
    let status = loop {
        steps += 1;
        assert!(steps < 1_000_000, "no progress");
        match shell.step(7) {
            Step::Done(s) => break s,
            Step::Yield => {}
            // The test host finishes everything during a step: nothing can wake a
            // blocked shell later.
            Step::Blocked => panic!("deadlock running {src:?}"),
            Step::Idle => panic!("idle"),
        }
    };
    assert_eq!(shell.task_count(), 0, "tasks left after {src:?}");
    assert_eq!(host.live_procs(), 0, "processes left after {src:?}");
    assert_eq!(host.live_pipes(), 0, "pipes left after {src:?}");
    Out {
        status,
        stdout: String::from_utf8_lossy(&host.stdout()).into_owned(),
        stderr: String::from_utf8_lossy(&host.stderr()).into_owned(),
    }
}

pub(crate) fn run_setup(src: &str, setup: impl Fn(&TestHost)) -> Out {
    let a = {
        let h = Rc::new(TestHost::new());
        setup(&h);
        run_on(h, src)
    };
    let b = {
        let h = Rc::new(TestHost::new());
        h.set_latency(true);
        setup(&h);
        run_on(h, src)
    };
    assert_eq!(
        (a.status, &a.stdout, &a.stderr),
        (b.status, &b.stdout, &b.stderr),
        "latency changed {src:?}"
    );
    a
}

pub(crate) fn run(src: &str) -> Out {
    run_setup(src, |_| {})
}

#[track_caller]
pub(crate) fn check(src: &str, stdout: &str) {
    let o = run(src);
    assert_eq!(o.stdout, stdout, "stdout of {src:?} (stderr: {})", o.stderr);
}

#[track_caller]
pub(crate) fn check_status(src: &str, status: i32) {
    let o = run(src);
    assert_eq!(o.status, status, "status of {src:?} (stderr: {})", o.stderr);
}

#[test]
fn echo_and_status() {
    check("echo hi", "hi\n");
    check("echo -n a; echo b", "ab\n");
    check("echo -e 'a\\tb'", "a\tb\n");
    check("echo -x", "-x\n");
    check_status("true", 0);
    check_status("false", 1);
    check("false; echo $?; true; echo $?", "1\n0\n");
    check("! true; echo $?; ! false; echo $?", "1\n0\n");
    check_status("exit 3", 3);
    check("(exit 4); echo $?", "4\n");
}

#[test]
fn and_or_lists() {
    check(
        "true && echo a; false && echo b; false || echo c; true || echo d",
        "a\nc\n",
    );
    check("false && echo a || echo b", "b\n");
}

#[test]
fn variables_and_quoting() {
    check("x=1; echo $x ${x} \"$x\" '$x'", "1 1 1 $x\n");
    check("x='a  b'; echo $x; echo \"$x\"", "a b\na  b\n");
    check(
        "echo ${u:-def} ${u-d2}; u=; echo ${u:-def}x ${u-d2}x",
        "def d2\ndefx x\n",
    );
    check("echo ${u:=set}; echo $u", "set\nset\n");
    check("x=val; echo ${x:+alt} ${u:+alt}x", "alt x\n");
    check("echo a\\ b \"c\\\"d\" $'e\\nf'", "a b c\"d e\nf\n");
    check("x=abc; echo ${#x}", "3\n");
    check(
        "x=a.b.c; echo ${x%.*} ${x%%.*} ${x#*.} ${x##*.}",
        "a.b a b.c c\n",
    );
    check(
        "x=hello; echo ${x/l/L} ${x//l/L} ${x^^}",
        "heLlo heLLo HELLO\n",
    );
    check("a=1 b=$a; echo $b", "1\n");
}

#[test]
fn word_splitting() {
    check("x='a b  c'; argv $x", "[a]\n[b]\n[c]\n");
    check("x=' a '; argv $x", "[a]\n");
    check("IFS=:; x='a::b:'; argv $x", "[a]\n[]\n[b]\n");
    check("e=; argv $e \"$e\"", "[]\n");
    check(
        "set -- 'a b' c; argv \"$@\"; argv $@; argv \"$*\"",
        "[a b]\n[c]\n[a]\n[b]\n[c]\n[a b c]\n",
    );
    check("set --; argv \"$@\"; echo $#", "0\n");
    check("x='a b'; argv x${x}y", "[xa]\n[by]\n");
}

#[test]
fn brace_expansion() {
    check("echo a{b,c}d {1..3} {a..c}", "abd acd 1 2 3 a b c\n");
    check("echo {01..03}", "01 02 03\n");
}

#[test]
fn command_substitution() {
    check("echo $(echo hi)", "hi\n");
    check("x=$(echo a; echo b); echo \"$x\"", "a\nb\n");
    check("echo \"$(echo 'a  b')\" $(echo 'a  b')", "a  b a b\n");
    check("x=$(exit 3); echo $?", "3\n");
    check("echo `echo hi`", "hi\n");
    check("echo $(echo $(echo nested))", "nested\n");
}

#[test]
fn pipelines() {
    check("echo hi | cat", "hi\n");
    check("echo a | cat | cat | cat", "a\n");
    check("echo hi | wc", "1\n");
    check("yes | head -n 2", "y\ny\n");
    check("{ echo a; echo b; } | cat", "a\nb\n");
    check("x=1 | true; echo ${x-unset}", "unset\n");
    check("echo x | false; echo $?", "1\n");
    check("echo $(echo a | cat)", "a\n");
}

#[test]
fn pipe_buffers_do_not_deadlock() {
    // More than a pipe's capacity through a builtin stage and an external.
    let src = "x=0123456789012345678901234567890123456789012345678901234567890123456789; \
               y=$x$x$x$x$x$x$x$x$x$x; z=$y$y$y$y$y$y$y$y$y$y; echo $z$z$z$z$z$z$z$z$z$z | cat | wc";
    check(src, "1\n");
}

#[test]
fn subshells_and_groups() {
    check("x=1; (x=2; echo $x); echo $x", "2\n1\n");
    check("x=1; { x=2; echo $x; }; echo $x", "2\n2\n");
    check("(echo a; exit 3; echo b); echo $?", "a\n3\n");
}

#[test]
fn redirections() {
    let o = run_setup("echo hi > /tmp/f; echo more >> f; cat < f; cat f", |_| {});
    assert_eq!(o.stdout, "hi\nmore\nhi\nmore\n");
    check(
        "echo err 2>&1 >/dev/null; { echo out; echo err >&2; } 2>&1 | cat",
        "out\nerr\n",
    );
    let o = run("echo err >&2");
    assert_eq!(o.stderr, "err\n");
    check(
        "{ echo a; echo b >&2; } &> /tmp/both; cat /tmp/both",
        "a\nb\n",
    );
    check("cat <<EOF\nhello $USER-x\nEOF", "hello -x\n");
    check("x=1; cat <<'EOF'\n$x\nEOF", "$x\n");
    check("cat <<< 'here string'", "here string\n");
    check("cat <<-EOF\n\tindented\n\tEOF", "indented\n");
    check("exec 3>&1; echo via3 >&3", "via3\n");
    let o = run("echo x > /dev/stderr 2>&1");
    assert_eq!((o.stdout.as_str(), o.stderr.as_str()), ("", "x\n"));
}

#[test]
fn redirection_errors() {
    let o = run("cat < /nonexistent; echo $?");
    assert_eq!(o.stdout, "1\n");
    assert_eq!(
        o.stderr,
        "psh: line 1: /nonexistent: No such file or directory\n"
    );
    let o = run("echo hi 1>&7");
    assert_eq!(o.status, 1);
    assert_eq!(o.stderr, "psh: line 1: 7: Bad file descriptor\n");
}

#[test]
fn large_heredoc_into_external() {
    let body = "x".repeat(200_000);
    let src = format!("cat <<EOF | wc\n{body}\nEOF");
    check(&src, "1\n");
}

#[test]
fn externals() {
    let o = run("nosuch; echo $?");
    assert_eq!(o.stdout, "127\n");
    assert_eq!(o.stderr, "psh: line 1: nosuch: command not found\n");
    check("X=1 env | grep_missing_is_fine; true", "");
    check(
        "export A=1; B=2 env",
        "A=1\nB=2\nHOME=/tmp\nPATH=/bin\nPWD=/tmp\nSHLVL=1\n_=/bin/env\n",
    );
    check_status("exit 7 | true; ./missing", 127);
}

#[test]
fn builtins_cd_pwd_test() {
    check("pwd; cd /; pwd; cd /tmp; pwd", "/tmp\n/\n/tmp\n");
    check("cd /nonexist; echo $?", "1\n");
    check("mkdir_missing=1; cd ..; pwd; cd -", "/\n/tmp\n");
    check(
        "[ a = a ] && echo y; test 1 -lt 2 && echo z; [ -d /tmp ] && echo d; [ -f /tmp ] || echo nf",
        "y\nz\nd\nnf\n",
    );
    check(
        "[ ! -e /nope ] && echo ok; [ -n '' ] || echo empty; [ \\( a = b \\) -o a = a ] && echo or",
        "ok\nempty\nor\n",
    );
    let o = run("[ a");
    assert_eq!(o.status, 2);
    assert_eq!(o.stderr, "psh: line 1: [: missing `]'\n");
}

#[test]
fn printf_basics() {
    check("printf '%s-%d\\n' a 1 b 2", "a-1\nb-2\n");
    check(
        "printf '[%5s][%-3d][%03d][%x]\\n' ab 7 5 255",
        "[   ab][7  ][005][ff]\n",
    );
    check("printf '%b' 'a\\tb\\n'", "a\tb\n");
    check("printf -v v '%s' val; echo $v", "val\n");
}

#[test]
fn export_unset_readonly() {
    check("x=1; unset x; echo ${x-gone}", "gone\n");
    check("export x=1; export -p | cat | wc; true", "6\n");
    let o = run("readonly r=1; r=2; echo same line\necho $?");
    assert_eq!(o.stdout, "1\n");
    assert_eq!(o.stderr, "psh: line 1: r: readonly variable\n");
}

#[test]
fn syntax_errors() {
    let o = run("echo a\necho )");
    assert_eq!(o.status, 2);
    assert_eq!(o.stdout, "a\n");
    assert_eq!(
        o.stderr,
        "psh: -c: line 2: syntax error near unexpected token `)'\npsh: -c: line 2: `echo )'\n"
    );
}

#[test]
fn errors_abandon_the_line() {
    let o = run(
        "echo ${a b}; echo same\necho next $?\nf() { echo $((1+)); echo no; }; f; echo no\necho last",
    );
    assert_eq!(o.stdout, "next 1\nlast\n");
    assert_eq!(o.status, 0);
    check("x=$(echo ${a b}; echo no); echo \"[$x] $?\"", "[] 1\n");
    check("until continue; do echo no; done; echo end", "end\n");
}

#[test]
fn expansion_errors_are_fatal() {
    let o = run("echo ${x:?oops}; echo after");
    assert_eq!(o.status, 127);
    assert_eq!(o.stdout, "");
    assert_eq!(o.stderr, "psh: line 1: x: oops\n");
}

#[test]
fn globbing() {
    let o = run_setup("echo /g/*.txt; echo /g/*.none; cd /g; echo *", |h| {
        h.fs.write_file("/g/a.txt", b"");
        h.fs.write_file("/g/b.txt", b"");
        h.fs.write_file("/g/.hidden.txt", b"");
        h.fs.write_file("/g/c.md", b"");
    });
    assert_eq!(o.stdout, "/g/a.txt /g/b.txt\n/g/*.none\na.txt b.txt c.md\n");
}

#[test]
fn sigpipe_in_builtin_stage() {
    // The writer stage dies of SIGPIPE when the reader is gone.
    check("{ while_missing=1; echo a; echo b; } | head -n 1", "a\n");
}

#[test]
fn budget_yields_and_state_persists() {
    let host = Rc::new(TestHost::new());
    let mut shell = Shell::new(host.config());
    shell.start(Script::command("x=1; echo a; echo b; echo c; echo d"));
    let mut yields = 0;
    loop {
        match shell.step(1) {
            Step::Done(_) => break,
            Step::Yield => yields += 1,
            s => panic!("{s:?}"),
        }
    }
    assert!(yields >= 2);
    shell.start(Script::command("echo $x"));
    while !matches!(shell.step(100), Step::Done(_)) {}
    assert_eq!(host.stdout(), b"a\nb\nc\nd\n1\n");
}

#[test]
fn cancel_releases_everything() {
    let host = Rc::new(TestHost::new());
    let mut shell = Shell::new(host.config());
    shell.start(Script::command("echo start; yes | hang | cat; echo never"));
    assert_eq!(shell.step(1000), Step::Blocked);
    assert!(host.live_procs() >= 2);
    let ctl = shell.control();
    ctl.cancel();
    assert_eq!(shell.step(1000), Step::Done(143));
    assert_eq!(shell.task_count(), 0);
    assert_eq!(host.live_procs(), 0);
    assert_eq!(host.live_pipes(), 0);
    assert_eq!(host.stdout(), b"start\n");
    // The shell is usable again, and the old cancel does not reach the new run.
    shell.start(Script::command("echo again"));
    assert_eq!(shell.step(1000), Step::Done(0));
    assert_eq!(host.stdout(), b"start\nagain\n");
}

#[test]
fn pause_and_resume() {
    let host = Rc::new(TestHost::new());
    let mut shell = Shell::new(host.config());
    shell.start(Script::command("echo a; echo b"));
    let ctl = shell.control();
    ctl.pause();
    assert_eq!(shell.step(1000), Step::Blocked);
    assert_eq!(host.stdout(), b"");
    ctl.resume();
    assert_eq!(shell.step(1000), Step::Done(0));
    assert_eq!(host.stdout(), b"a\nb\n");
}

#[test]
fn nesting_limit() {
    let mut src = String::from("echo deep");
    for _ in 0..20 {
        src = format!("$({src})");
        src = format!("echo {src}");
    }
    check(&src, "deep\n");
    let host = Rc::new(TestHost::new());
    let mut cfg = host.config();
    cfg.max_depth = 10;
    let mut shell = Shell::new(cfg);
    shell.start(Script::command(&src));
    while !matches!(shell.step(1000), Step::Done(_)) {}
    assert!(String::from_utf8_lossy(&host.stderr()).contains("maximum nesting level exceeded"));
}

#[test]
fn functions_and_locals() {
    check("f() { echo \"f:$1:$#\"; }; f a b; f", "f:a:2\nf::0\n");
    check("f() { return 3; echo no; }; f; echo $?", "3\n");
    check(
        "x=g; f() { local x=l; echo $x; g; }; g() { echo $x; }; f; echo $x",
        "l\nl\ng\n",
    );
    check(
        "f() { local x; echo \"${x-unset}\"; }; x=1; f; echo $x",
        "unset\n1\n",
    );
    check("function f { echo kw; }; f", "kw\n");
    check(
        "f() { echo $1; }; set -- top; f inner; echo $1",
        "inner\ntop\n",
    );
    check("f() { echo out; } > /tmp/fo; f; cat /tmp/fo", "out\n");
    check("f() { echo hi; }; unset -f f; f; echo $?", "127\n");
    check(
        "echo() { builtin_missing=1; printf 'fn\\n'; }; echo x",
        "fn\n",
    );
    check("f() { echo ${X-u}; }; X=2 f; echo ${X-u}", "2\nu\n");
    let o = run("local x");
    assert_eq!(
        o.stderr,
        "psh: line 1: local: can only be used in a function\n"
    );
}

#[test]
fn control_flow() {
    check(
        "if true; then echo a; elif true; then echo b; else echo c; fi",
        "a\n",
    );
    check(
        "if false; then echo a; elif true; then echo b; else echo c; fi",
        "b\n",
    );
    check("if false; then echo a; fi; echo $?", "0\n");
    check("for i in a 'b c'; do echo \"$i\"; done", "a\nb c\n");
    check("set -- x y; for i; do echo $i; done", "x\ny\n");
    check(
        "i=0; while [ $i -lt 3 ]; do echo $i; i=$((i+1)); done",
        "0\n1\n2\n",
    );
    check(
        "i=0; until [ $i -ge 2 ]; do echo $i; i=$((i+1)); done",
        "0\n1\n",
    );
    check(
        "for i in 1 2 3; do [ $i = 2 ] && continue; echo $i; done",
        "1\n3\n",
    );
    check(
        "for i in a b; do for j in 1 2; do echo $i$j; break 2; done; done",
        "a1\n",
    );
    check(
        "for i in a b; do for j in 1 2; do continue 2; echo no; done; echo no; done; echo $i$j",
        "b1\n",
    );
    check("case abc in a*) echo A;; *) echo star;; esac", "A\n");
    check(
        "case x in a|x) echo hit;& b) echo fell;; c) echo no;; esac",
        "hit\nfell\n",
    );
    check(
        "case ab in a*) echo 1;;& *b) echo 2;; *) echo 3;; esac",
        "1\n2\n",
    );
    check(
        "x='*'; case a in \"$x\") echo lit;; $x) echo glob;; esac",
        "glob\n",
    );
    check("for i in 1 2 3; do echo $i; done | wc", "3\n");
    let o = run("break; echo $?");
    assert_eq!(o.stdout, "0\n");
}

#[test]
fn arithmetic() {
    check(
        "echo $((1 + 2 * 3)) $((7 / 2)) $((-7 % 3)) $((2 ** 10))",
        "7 3 -1 1024\n",
    );
    check("x=5; echo $((x + 1)) $((x++)) $x $((++x))", "6 5 6 7\n");
    check("((1 > 2)); echo $?; ((2 > 1)); echo $?", "1\n0\n");
    check("for ((i = 0; i < 3; i++)); do echo $i; done", "0\n1\n2\n");
    check("for ((;;)); do echo once; break; done", "once\n");
    check("let x=2+3 y=x*2; echo $x $y $?", "5 10 0\n");
    check("a=b; b=7; echo $((a + 1))", "8\n");
    check("let -- x=3; echo $? $x; x=3; let -x; echo $?", "0 3\n0\n");
    let o = run("echo $((1+)); echo after");
    assert_eq!(o.status, 1);
    assert_eq!(o.stdout, "");
    assert_eq!(
        o.stderr,
        "psh: line 1: 1+: arithmetic syntax error: operand expected (error token is \"+\")\n"
    );
    let o = run("((1/0)); echo $?");
    assert_eq!(o.stdout, "1\n");
}

#[test]
fn deep_recursion() {
    // 1000 nested calls need ~15 MiB of stack in debug builds (D1050): run on a
    // thread sized as embedders are told to.
    let t = std::thread::Builder::new()
        .stack_size(
            crate::ShellConfig::DEFAULT_MAX_DEPTH as usize * crate::ShellConfig::STACK_PER_LEVEL
                + (4 << 20),
        )
        .spawn(|| {
            let o = run(
                "f() { if [ $1 -lt 999 ]; then f $(( $1 + 1 )); else echo depth $1; fi; }; f 0",
            );
            assert_eq!(o.stdout, "depth 999\n");
            // Past the limit: an error, not a stack overflow.
            let o = run("f() { f; }; f; echo after");
            assert!(
                o.stderr
                    .contains("f: maximum function nesting level exceeded (1000)"),
                "{}",
                o.stderr
            );
            assert_eq!(o.status, 1);
        })
        .unwrap();
    t.join().unwrap();
}

#[test]
fn loops_yield_at_safepoints() {
    let host = Rc::new(TestHost::new());
    let mut shell = Shell::new(host.config());
    shell.start(Script::command("while true; do :; done"));
    for _ in 0..100 {
        assert_eq!(shell.step(50), Step::Yield);
    }
    shell.control().cancel();
    assert_eq!(shell.step(50), Step::Done(143));
}

#[test]
fn background_jobs_at_exit() {
    // Without a grace they are released (the test host's fakes just stop); with one,
    // the shell waits for them, then abandons what is left.
    for grace in [None, Some(std::time::Duration::from_millis(10))] {
        let host = Rc::new(TestHost::new());
        let mut cfg = host.config();
        cfg.exit_grace = grace;
        let mut shell = Shell::new(cfg);
        shell.start(Script::command("hang & { echo bg; } & echo fg"));
        let st = loop {
            match shell.step(100) {
                Step::Done(s) => break s,
                Step::Yield => {}
                s => panic!("{s:?}"),
            }
        };
        assert_eq!(st, 0);
        assert_eq!(shell.task_count(), 0);
        assert_eq!(host.live_procs(), 0);
        let out = String::from_utf8(host.stdout()).unwrap();
        assert!(out.contains("fg\n") && out.contains("bg\n"), "{out}");
    }
}

#[test]
fn non_utf8_scripts_are_byte_exact() {
    let host = Rc::new(TestHost::new());
    let mut shell = Shell::new(host.config());
    shell.start(Script {
        source: b"x='\xff\xfe'; echo \"$x\" a\xe9b; echo `echo \xff`\necho )".to_vec(),
        arg0: b"psh".to_vec(),
        args: Vec::new(),
        dash_c: true,
        options: Vec::new(),
        stdin: false,
    });
    while !matches!(shell.step(100), Step::Done(_)) {}
    assert_eq!(host.stdout(), b"\xff\xfe a\xe9b\n\xff\n");
    assert!(host.stderr().ends_with(b"line 2: `echo )'\n"));
}

/// Runs with `expand_aliases` on (`shopt` is not built yet), with and without latency.
fn run_aliases(src: &str) -> Out {
    let go = |latency: bool| {
        let host = Rc::new(TestHost::new());
        host.set_latency(latency);
        let mut shell = Shell::new(host.config());
        shell.vp.as_mut().unwrap().expand_aliases = true;
        shell.start(Script::command(src));
        let status = loop {
            match shell.step(7) {
                Step::Done(s) => break s,
                Step::Yield => {}
                other => panic!("{other:?} running {src:?}"),
            }
        };
        Out {
            status,
            stdout: String::from_utf8_lossy(&host.stdout()).into_owned(),
            stderr: String::from_utf8_lossy(&host.stderr()).into_owned(),
        }
    };
    let (a, b) = (go(false), go(true));
    assert_eq!(
        (a.status, &a.stdout, &a.stderr),
        (b.status, &b.stdout, &b.stderr)
    );
    a
}

#[test]
fn aliases() {
    // Expected output is bash 5.3's (with `shopt -s expand_aliases`).
    let o = run_aliases("alias e=echo; e one\ne two; e three\nalias ll='echo ll '\nll e x");
    assert_eq!(o.stdout, "two\nthree\nll echo x\n");
    let o = run_aliases("alias hi='echo hello world'\nhi || echo no\necho hi\n'hi' || echo quoted");
    assert_eq!(o.stdout, "hello world\nhi\nquoted\n");
    assert!(
        o.stderr.contains("line 4: hi: command not found"),
        "{}",
        o.stderr
    );
    let o = run_aliases(
        "alias b='x y' a=\"it's\" c=\nalias\nalias a nope; echo $?\nunalias a nope; echo $?\nalias",
    );
    assert_eq!(
        o.stdout,
        "alias a='it'\\''s'\nalias b='x y'\nalias c=''\nalias a='it'\\''s'\n1\n1\nalias b='x y'\nalias c=''\n"
    );
    assert_eq!(
        o.stderr,
        "psh: line 3: alias: nope: not found\npsh: line 4: unalias: nope: not found\n"
    );
    // A subshell's aliases stay in it; backquote bodies expand aliases too.
    let o = run_aliases("(alias s=echo)\ns x\nalias t='echo T'\necho `t x`");
    assert_eq!(o.stdout, "T x\n");
    // Line numbers of commands from alias text are those of the alias word.
    let o = run_aliases("alias bad='nosuchcmd_x'\n\nbad");
    assert!(
        o.stderr.contains("line 3: nosuchcmd_x: command not found"),
        "{}",
        o.stderr
    );
}

#[test]
fn indexed_arrays() {
    check(
        "a=(x 'y z'); a[5]=q; echo ${a[1]} ${a[-1]} ${#a[@]} ${!a[@]}",
        "y z q 3 0 1 5\n",
    );
    check(
        "a=(x 'y z'); argv \"${a[@]}\"; argv \"${a[*]}\"",
        "[x]\n[y z]\n[x y z]\n",
    );
    check(
        "a=(1 2); a+=(3 4); echo ${a[@]}; unset 'a[1]'; echo ${a[@]} ${!a[@]}",
        "1 2 3 4\n1 3 4 0 2 3\n",
    );
    check("a=(a b c d); echo ${a[@]:1:2} ${a[@]: -1}", "b c d\n");
    check(
        "a=([3]=x y [0]=z); declare -p a",
        "declare -a a=([0]=\"z\" [3]=\"x\" [4]=\"y\")\n",
    );
    check(
        "a=(1 2 3); echo $((a[1] + a[2])); (( a[0] += 5 )); echo $a",
        "5\n6\n",
    );
    check("i=1; a[i+1]=v; echo ${a[2]}", "v\n");
    let o = run("a=(1); a[-5]=x; echo same\necho next");
    assert_eq!(o.stdout, "next\n");
    assert_eq!(o.stderr, "psh: line 1: a[-5]: bad array subscript\n");
}

#[test]
fn associative_arrays() {
    check(
        "declare -A h=([k]=v [b]='2 3'); echo ${h[k]} \"${h[b]}\"; declare -p h",
        "v 2 3\ndeclare -A h=([k]=\"v\" [b]=\"2 3\" )\n",
    );
    check(
        "declare -A h; h[a]=1; h[b]=2; h[c]=3; h[d]=4; echo ${!h[@]}",
        "d c b a\n",
    );
    check(
        "declare -A h=(k1 v1 k2 v2); echo ${h[k2]}; unset 'h[k2]'; echo ${#h[@]}",
        "v2\n1\n",
    );
    check("declare -A h; k='a b'; h[$k]=x; echo \"${h[a b]}\"", "x\n");
}

#[test]
fn declare_attributes() {
    check(
        "declare -i n=2+3; n+=1; echo $n; declare -l l=ABC; declare -u u=abc; echo $l $u",
        "6\nabc ABC\n",
    );
    check("declare -r r=1; r=2; echo no\necho $r", "1\n");
    check("x=1; declare -n ref=x; ref=5; echo $x ${!ref}", "5 x\n");
    check(
        "f() { declare a=local; g=global; }; a=outer; f; echo $a $g",
        "outer global\n",
    );
    check("f() { declare -g a=global; }; f; echo $a", "global\n");
    check("export -n PATH; declare -x e=1; env | wc", "5\n");
    check(
        "f() { echo hi; }; declare -f f; declare -F f",
        "f () \n{ \n    echo hi\n}\nf\n",
    );
}

#[test]
fn substrings_and_transforms() {
    check(
        "x=hello; echo ${x:1:3} ${x: -2} ${x:1:-1} ${x::2}",
        "ell lo ell he\n",
    );
    check("set -- a b c; echo ${@:2} ${@:0:1}", "b c psh\n");
    check("x=\"it's\"; echo ${x@Q}", "'it'\\''s'\n");
}

#[test]
fn empty_quoted_word_is_a_field() {
    check(
        r#"f() { echo $#; }; f a ""; f ''; f "$@"; f "" "$@""#,
        "2\n1\n0\n1\n",
    );
}

#[test]
fn mapfile_and_process_substitution() {
    check(
        "mapfile -t a < <(echo x; echo y); declare -p a",
        "declare -a a=([0]=\"x\" [1]=\"y\")\n",
    );
    check(
        "readarray -O 2 -n 1 a <<< $'p\\nq'; declare -p a",
        "declare -a a=([2]=$'p\\n')\n",
    );
    check(
        "echo hi > >(cat); while read l; do echo got $l; done < <(echo 1; echo 2)",
        "hi\ngot 1\ngot 2\n",
    );
    check("echo <(true) >(true)", "/dev/fd/63 /dev/fd/62\n");
}

#[test]
fn path_lookups_split_path_as_the_host_says() {
    // Split on `;`, `/x:y` is one absolute entry; split on `:`, `y;/bin` would be a
    // relative one (D1910).
    let src = "PATH='/x:y;/bin'; type -a cat; cat </dev/null && echo ran";
    let o = run_setup(src, |h| h.set_path_sep(b';'));
    assert_eq!(o.stdout, "cat is /bin/cat\nran\n", "{}", o.stderr);
    let o = run(src);
    assert!(o.stdout.contains("cat is y;/bin/cat\n"), "{}", o.stdout);
}

#[test]
fn special_variables_and_operands() {
    check("echo a b; echo $_; x=1; echo \"[$_]\"", "a b\nb\n[]\n");
    check(
        "f() { echo \"in:$_\"; }; : seed; f arg; echo $_",
        "in:seed\narg\n",
    );
    check(
        "echo $BASH_SUBSHELL $(echo $BASH_SUBSHELL) $( (echo $BASH_SUBSHELL) )",
        "0 1 2\n",
    );
    check(
        "RANDOM=7; a=$RANDOM; RANDOM=7; [ $a = $RANDOM ] && echo seeded; echo $((RANDOM < 32768))",
        "seeded\n1\n",
    );
    check("echo ${BASH_VERSINFO[0]} ${PSH_VERSION:+psh}", "5 psh\n");
    check(
        "argv ${u-a b} \"${u-'q'}\" ${u-\"c d\"}",
        "[a]\n[b]\n['q']\n[c d]\n",
    );
    check("set -- one '' two; IFS=x; argv $@", "[one]\n[]\n[two]\n");
}

#[test]
fn read_file_substitution() {
    let o = run_setup("x=$(< /tmp/f); echo \"[$x]\"; y=$(< /nope); echo $?", |h| {
        h.fs.write_file("/tmp/f", b"hello\n\n")
    });
    assert_eq!(o.stdout, "[hello]\n1\n");
}

#[test]
fn extglob_and_glob_options() {
    let setup = |h: &TestHost| {
        for f in [
            "/g/x.c",
            "/g/y.h",
            "/g/.hid",
            "/g/a/z.c",
            "/g/a/b/w.c",
            "/g/README",
        ] {
            h.fs.write_file(f, b"");
        }
    };
    let o = run_setup(
        "cd /g; shopt -s extglob\necho !(*.c); echo @(x|y).*; case f.c in *.@(c|h)) echo yes;; esac; x=f.bar; echo ${x%.@(bar|baz)}",
        setup,
    );
    assert_eq!(o.stdout, "README a y.h\nx.c y.h\nyes\nf\n", "{}", o.stderr);
    let o = run_setup(
        "cd /g; shopt -s globstar; echo **/*.c; echo a/**; echo **/",
        setup,
    );
    assert_eq!(
        o.stdout, "a/b/w.c a/z.c x.c\na/ a/b a/b/w.c a/z.c\na/ a/b/\n",
        "{}",
        o.stderr
    );
    let o = run_setup(
        "cd /g; shopt -s dotglob; echo *; shopt -s nocaseglob; echo [r]eadme; shopt -s nocasematch; case ABC in abc) echo ci;; esac",
        setup,
    );
    assert_eq!(
        o.stdout, ".hid README a x.c y.h\nREADME\nci\n",
        "{}",
        o.stderr
    );
}

#[test]
fn tilde_user_and_drive_paths() {
    check(
        "echo ~root ~nosuchuser_x ~root/a",
        "/root ~nosuchuser_x /root/a\n",
    );
    assert_eq!(crate::builtins::normalize(b"C:/x/../y/./z"), b"C:/y/z");
    assert_eq!(crate::builtins::normalize(b"C:/.."), b"C:/");
    assert_eq!(crate::builtins::normalize(br"c:\a\..\b"), b"c:/b");
    assert_eq!(crate::builtins::normalize(b"/a/../b"), b"/b");
}

#[test]
fn expansion_climb() {
    // Array slices: a negative length is an error only inside the elements.
    check(
        "a=(a b c); echo \"[${a[@]:3:-1}]\"; s=hello; echo ${s[@]:1:2}",
        "[]\nel\n",
    );
    // patsub_replacement.
    check(
        "x=abc; echo ${x/b/<&>} ${x/b/\\&} \"${x//?/[&]}\"; r='\\&'; echo ${x/b/$r}",
        "a<b>c a&c [a][b][c]\na&c\n",
    );
    // Function substitutions run in the shell.
    check(
        "x=${ echo hi; y=1; }; echo $x $y; REPLY=o; z=${| REPLY=v; }; echo $z $REPLY",
        "hi 1\nv o\n",
    );
    // `{var}` redirections outlast the command.
    check(
        "exec {fd}>/f; echo $fd; echo hi >&$fd; exec {fd}>&-; cat /f; : {g}>/g; echo x >&$g; cat /g",
        "10\nhi\nx\n",
    );
    // `~:` ends a tilde prefix; assignment operands use assignment rules.
    check(
        "HOME=/h; echo ~:x; x=~:${u-~:~}; echo $x",
        "/h:x\n/h:/h:/h\n",
    );
}

#[test]
fn globignore() {
    let setup = |h: &TestHost| {
        for f in ["/g/a.txt", "/g/b.txt", "/g/.c.txt"] {
            h.fs.write_file(f, b"");
        }
    };
    let o = run_setup(
        "cd /g; GLOBIGNORE=b.txt; echo *.txt; unset GLOBIGNORE; echo *.txt",
        setup,
    );
    assert_eq!(o.stdout, ".c.txt a.txt\na.txt b.txt\n", "{}", o.stderr);
}

#[test]
fn command_subst_crlf() {
    use crate::exec::strip_newlines_with;
    let strip = |s: &[u8], crlf| {
        let mut v = s.to_vec();
        strip_newlines_with(&mut v, crlf);
        v
    };
    assert_eq!(strip(b"sub\r\n", true), b"sub");
    assert_eq!(strip(b"a\r\n\r\n\n", true), b"a");
    assert_eq!(strip(b"a\r\nb\r", true), b"a\r\nb\r");
    assert_eq!(strip(b"sub\r\n", false), b"sub\r");
    // End to end, by the host's rule: `$(…)`, backticks and `${ …; }`.
    let src = "x=$(printf 'a\\r\\n\\r\\n'); y=`printf 'a\\r\\n\\r\\n'`; z=${ printf 'a\\r\\n\\r\\n'; }; echo ${#x} ${#y} ${#z}";
    assert_eq!(run_setup(src, |_| {}).stdout, "4 4 4\n");
    assert_eq!(
        run_setup(src, |h: &TestHost| h.set_strips_crlf(true)).stdout,
        "1 1 1\n"
    );
}

#[test]
fn host_info_variables() {
    // The test host reports `HostInfo::default()`.
    let i = crate::host::HostInfo::default();
    check(
        "echo $UID $EUID ${GROUPS[0]} $HOSTTYPE; GROUPS=(7); echo ${GROUPS[0]}; (UID=1) 2>/dev/null || echo ro",
        &format!(
            "{} {} {} {}\n{}\nro\n",
            i.uid, i.euid, i.groups[0], i.hosttype, i.groups[0]
        ),
    );
    check(
        "declare -p UID",
        &format!("declare -ir UID=\"{}\"\n", i.uid),
    );
}

#[test]
fn well_known_variables() {
    check(
        "echo \"$OPTERR $PS4|\"; BASH_ARGV0=x; echo $0; alias q=w; echo ${BASH_ALIASES[q]}; BASH_ALIASES[z]=y; alias z",
        "1 + |\nx\nw\nalias z='y'\n",
    );
    check(
        ": | ( echo $BASH_SUBSHELL ); ( echo $BASH_SUBSHELL ) | cat; echo $(: | ( echo $BASH_SUBSHELL ))",
        "1\n1\n2\n",
    );
    check("unset BASH_SOURCE 2>/dev/null || echo kept", "kept\n");
}

#[test]
fn prefix_assignments_and_temp_unset() {
    // Each prefix assignment sees the earlier ones while they expand, not the shell.
    check(
        "f() { echo \"$A $B\"; }; A=1 B=[$A] f; echo \"[$A]\"",
        "1 [1]\n[]\n",
    );
    // Unsetting a temporary binding in the function reveals the global one, which later
    // assignments then change.
    check(
        "x=g; f() { unset x; echo \"[$x]\"; x=new; }; x=t f; echo $x",
        "[g]\nnew\n",
    );
    // `$_` is not exported once a command updates it.
    check(
        "export _=x; echo a b; declare -p _",
        "a b\ndeclare -- _=\"b\"\n",
    );
    check(
        "s=abc; echo \"[${s::1}] [${s::}]\"; (echo ${s:}) 2>/dev/null || echo bad",
        "[a] []\nbad\n",
    );
}

/// A host with OS pipes: an external gets the pipe itself, as a child of bash does.
#[track_caller]
fn check_os(src: &str, stdout: &str) {
    let o = run_setup(src, |h| h.set_os_pipes(true));
    assert_eq!(o.stdout, stdout, "stdout of {src:?} (stderr: {})", o.stderr);
}

#[test]
fn an_external_in_a_loop_leaves_the_loops_input() {
    // The body's program does not read: every line is still the loop's.
    check_os(
        r"printf '1\n2\n3\n' | while read x; do /bin/true; echo got $x; done",
        "got 1\ngot 2\ngot 3\n",
    );
    check_os(
        r"printf '1\n2\n3\n' | while read x; do argv a; /bin/true; argv b; done",
        "[a]\n[b]\n[a]\n[b]\n[a]\n[b]\n",
    );
    // An external writes the lines.
    check_os(
        r"printf 'a\nb\n' > f; cat f | while IFS= read -r x; do /bin/true; echo got $x; done",
        "got a\ngot b\n",
    );
    // until and for, a here-document, a here-string and a redirected file.
    check_os(
        r"printf '1\n2\n' | until ! read x; do /bin/true; echo $x; done",
        "1\n2\n",
    );
    check_os(
        r"printf '1\n2\n' | for i in a b; do read x; /bin/true; echo $i$x; done",
        "a1\nb2\n",
    );
    check_os(
        "while read x; do /bin/true; echo $x; done <<EOF\na\nb\nEOF",
        "a\nb\n",
    );
    check_os(
        r"while read x; do /bin/true; echo $x; done <<< $'a\nb'",
        "a\nb\n",
    );
    check_os(
        r"printf 'a b\nc d\n' > f; while IFS=' ' read -r x y; do /bin/true; echo $y $x; done < f",
        "b a\nd c\n",
    );
    // Nested loops, each with its own input.
    check_os(
        r"printf 'x\ny\n' | while read a; do /bin/true; printf '1\n2\n' | while read b; do /bin/true; echo $a$b; done; done",
        "x1\nx2\ny1\ny2\n",
    );
}

#[test]
fn an_external_that_reads_takes_the_loops_input() {
    // As in bash: what the body's program reads is gone for the loop.
    check_os(
        r"printf '1\n2\n3\n' | while read x; do head -n 1; echo got $x; done",
        "2\ngot 1\ngot 3\n",
    );
    check_os(
        r"printf '1\n2\n3\n' | while read x; do cat; echo got $x; done",
        "2\n3\ngot 1\n",
    );
    check_os(
        r"printf '1\n2\n3\n4\n' | { read a; head -n 1; read b; echo $a $b; cat; }",
        "2\n1 3\n4\n",
    );
    check_os(
        "while read x; do head -n 1; echo got $x; done <<EOF\na\nb\nc\nEOF",
        "b\ngot a\ngot c\n",
    );
}

#[test]
fn a_pipe_larger_than_the_os_pipe_reaches_an_external_in_order() {
    // The test host's OS pipe holds 4096 bytes: the rest follows as the external reads,
    // and what it leaves is the next reader's.
    check_os(
        "i=0; while [ $i -lt 2000 ]; do echo line$i; i=$((i+1)); done > f; all=$(cat f); \
         { read a; /bin/true; head -n 1; cat | wc -l; } <<< $all; echo $a; \
         cat f | { read a; /bin/true; n=0; while read x; do n=$((n+1)); done; echo $a $n; }",
        "line1\n1998\nline0\nline0 1999\n",
    );
}

#[test]
fn a_program_writes_through_a_file_the_host_cannot_give_it() {
    // The shell's stdout and stderr are an embedder's sinks: the program gets pipes, and
    // what it writes arrives in order with the shell's own output.
    let o = run_setup(
        "echo before; argv a b; cat nofile; echo x | cat; argv c >&2; echo after",
        |h| h.set_gives_stdio(false),
    );
    assert_eq!(o.stdout, "before\n[a]\n[b]\nx\nafter\n", "{}", o.stderr);
    assert_eq!(o.stderr, "cat: no such file\n[c]\n");
    assert_eq!(o.status, 0);
}

#[test]
fn time_reports_what_its_pipeline_took() {
    // The test host's clocks stand still: the lines are bash's, the times zero.
    let o = run("time echo hi");
    assert_eq!(o.stdout, "hi\n");
    assert_eq!(o.stderr, "\nreal\t0m0.000s\nuser\t0m0.000s\nsys\t0m0.000s\n");
    let o = run("time -p echo hi | cat; echo $?");
    assert_eq!(o.stdout, "hi\n0\n");
    assert_eq!(o.stderr, "real 0.00\nuser 0.00\nsys 0.00\n");
    // The pipeline's own redirections do not take the report; the group's do.
    let o = run("time -p false 2>/dev/null; echo $?; { time ! false; } 2>/dev/null; echo $?");
    assert_eq!(o.stdout, "1\n0\n");
    assert_eq!(o.stderr, "real 0.00\nuser 0.00\nsys 0.00\n");
    let o = run("TIMEFORMAT='%R %1U %lS %P %%'; time true; TIMEFORMAT=; time true; time");
    assert_eq!(o.stderr, "0.000 0.0 0m0.000s 0.00 %\n");
    let o = run("TIMEFORMAT=%Q; time echo ran");
    assert_eq!(o.stdout, "ran\n");
    assert!(
        o.stderr
            .ends_with("TIMEFORMAT: `Q': invalid format character\n"),
        "{}",
        o.stderr
    );
    // bash reports before an exit inside the pipeline, and not for a return.
    let o = run("f() { time -p return 4; }; f; echo $?; time -p exit 3");
    assert_eq!(o.stdout, "4\n");
    assert_eq!(
        (o.status, o.stderr.as_str()),
        (3, "real 0.00\nuser 0.00\nsys 0.00\n")
    );
    // A job reports too, and `--` is bash's `-p`.
    let o = run("time -p true & wait; time -- echo x");
    assert_eq!(o.stdout, "x\n");
    assert_eq!(
        o.stderr,
        "real 0.00\nuser 0.00\nsys 0.00\n".repeat(2)
    );
    check("times", "0m0.000s 0m0.000s\n0m0.000s 0m0.000s\n");
}
