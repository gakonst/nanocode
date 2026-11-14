//! Tests of the core builtins (`read`, `eval`, `source`, `command`, `type`, ...) on the
//! test host.

use crate::tests::{check, check_status, run, run_setup};

#[test]
fn read_splits_and_escapes() {
    check("read x y <<< '  a  b c  '; echo \"[$x][$y]\"", "[a][b c]\n");
    check("read <<< '  a b  '; echo \"[$REPLY]\"", "[  a b  ]\n");
    check("read x <<< 'a\\ b\\\\'; echo \"[$x]\"", "[a b\\]\n");
    check("read -r x <<< 'a\\ b'; echo \"[$x]\"", "[a\\ b]\n");
    check("IFS=: read a b <<< '1:2:'; echo \"[$a][$b]\"", "[1][2]\n");
    check("IFS= read a <<< '  x  '; echo \"[$a]\"", "[  x  ]\n");
    check("read -n 2 x <<< abc; echo $x", "ab\n");
    check("read -N 4 x y <<< 'a b c'; echo \"[$x][$y]\"", "[a b ][]\n");
    check("read -d , x <<< 'a,b'; echo $x", "a\n");
    check(
        "printf 'a\\nb' | { read x; read y; echo \"$? $x $y\"; }",
        "1 a b\n",
    );
}

#[test]
fn read_errors() {
    check_status("read -z", 2);
    check_status("read -u 9 x", 1);
    check_status("read -t abc x", 1);
    let o = run("read 'a b' <<< x; echo $?");
    assert_eq!(o.stdout, "1\n");
    assert!(o.stderr.contains("not a valid identifier"), "{}", o.stderr);
    check("readonly r; read r <<< x; echo $?", "1\n");
}

#[test]
fn eval_runs_text() {
    check("eval 'echo a; echo b'", "a\nb\n");
    check("x='echo hi'; eval \"$x\"; eval; echo $?", "hi\n0\n");
    check("eval 'false'; echo $?", "1\n");
    check("f() { eval 'return 3'; echo no; }; f; echo $?", "3\n");
    check("for i in 1 2; do eval break; done; echo $i", "1\n");
    let o = run("eval 'echo )'; echo st=$?");
    assert_eq!(o.stdout, "st=2\n");
    assert!(
        o.stderr.contains("eval: line 1: syntax error"),
        "{}",
        o.stderr
    );
    check("eval -- 'echo x'", "x\n");
    check_status("eval -x", 2);
}

#[test]
fn prefix_assignments_on_builtins_are_temporary() {
    check("x=1 eval 'echo $x'; echo \"[$x]\"", "1\n[]\n");
    check("y=0; y=1 eval 'y=2'; echo $y", "0\n");
    check("x=1 :; echo \"[$x]\"", "[]\n");
    let o = run("x=1 eval env; env");
    assert_eq!(o.stdout.matches("x=1\n").count(), 1, "{}", o.stdout);
}

#[test]
fn source_files() {
    let o = run_setup(
        "set -- o; . /tmp/s.sh a b; echo \"$? [$*]\"; source /tmp/r.sh; echo $?; \
         . nosuch; echo $?; . /tmp; echo $?",
        |h| {
            h.fs.write_file("/tmp/s.sh", b"echo \"in $# $*\"\nv=set\n");
            h.fs.write_file("/tmp/r.sh", b"return 4\necho no\n");
        },
    );
    assert_eq!(o.stdout, "in 2 a b\n0 [o]\n4\n1\n1\n");
    assert!(o.stderr.contains("nosuch: No such file"), "{}", o.stderr);
    assert!(o.stderr.contains(".: /tmp: is a directory"), "{}", o.stderr);
    // Errors inside name the sourced file.
    let o = run_setup(". /tmp/e.sh", |h| {
        h.fs.write_file("/tmp/e.sh", b"\nnosuchcmd\n")
    });
    assert!(
        o.stderr.starts_with("/tmp/e.sh: line 2: nosuchcmd"),
        "{}",
        o.stderr
    );
    check(". /dev/stdin <<< 'echo from-stdin'", "from-stdin\n");
    check_status("source", 2);
    check_status("return 1", 2);
}

#[test]
fn command_and_type() {
    check(
        "f() { echo f; }; command -v f echo if cat",
        "f\necho\nif\n/bin/cat\n",
    );
    check("command -v nosuch; echo $?", "1\n");
    // -p looks in the host's standard path.
    check(
        "command -pv cat; command -pV cat; command -pv nosuch; echo $?",
        "/bin/cat\ncat is /bin/cat\n1\n",
    );
    check("echo() { :; }; command echo hi", "hi\n");
    check(
        "command -V echo; type -t if f cat echo",
        "echo is a shell builtin\nkeyword\nfile\nbuiltin\n",
    );
    check(
        "f() { echo hi; }; type f",
        "f is a function\nf () \n{ \n    echo hi\n}\n",
    );
    check(
        "type cat; cat </dev/null; type cat",
        "cat is /bin/cat\ncat is hashed (/bin/cat)\n",
    );
    check("type nosuch 2>/dev/null; echo $?", "1\n");
    check("builtin echo x; builtin cat 2>/dev/null; echo $?", "x\n1\n");
    check("enable -s | head -n 2", "enable .\nenable :\n");
    check(
        "enable -n true; type -t true; enable true; type -t true",
        "file\nbuiltin\n",
    );
}

#[test]
fn hash_table() {
    check("hash", "hash: hash table empty\n");
    check(
        "hash -p /x/y foo; hash -t foo; hash -l; hash -d foo; hash",
        "/x/y\nbuiltin hash -p /x/y foo\nhash: hash table empty\n",
    );
    check(
        "cat </dev/null; cat </dev/null; hash",
        "hits\tcommand\n   2\t/bin/cat\n",
    );
    check(
        "cat </dev/null; PATH=/other; hash",
        "hash: hash table empty\n",
    );
    check_status("hash -t", 1);
}

#[test]
fn exec_runs_and_ends() {
    check("(exec argv a; echo no); echo $?", "[a]\n0\n");
    check("(exec false); echo $?", "1\n");
    check("(exec nosuch; echo no) 2>/dev/null; echo $?", "127\n");
    check_status("exec -x", 2);
    check("exec; echo still", "still\n");
    check("export q=1; (exec -c env); (exec -a zz argv x)", "[x]\n");
}

#[test]
fn caller_frames() {
    let o = run_setup(". /tmp/s.sh; outer", |h| {
        h.fs.write_file(
            "/tmp/s.sh",
            b"outer() {\n  inner\n}\ninner() {\n  caller 0; caller 1; echo $?; caller\n}\n",
        );
    });
    assert_eq!(o.stdout, "2 outer /tmp/s.sh\n1\n2 /tmp/s.sh\n");
    check("caller; echo $?", "1\n");
    check("f() { caller; }; f", "1 NULL\n");
    check_status("f() { caller x; }; f", 2);
}

#[test]
fn umask_modes() {
    check(
        "umask; umask -S; umask -p",
        "0022\nu=rwx,g=rx,o=rx\numask 0022\n",
    );
    check("umask 027; umask; umask g-w,o-w; umask", "0027\n0027\n");
    check("umask 0124; umask a=,a=u; umask", "0111\n");
    check("(umask 077); umask", "0022\n");
    check("umask 8; echo $?; umask b=r; echo $?", "1\n1\n");
}

#[test]
fn printf_formats() {
    check("printf '[%q]\\n' 'a b' ''", "[a\\ b]\n['']\n");
    check("printf -vx '%s' hi; echo $x", "hi\n");
    check("printf '%b|' '\\141' '\\0141'; echo", "a|a|\n");
    check(
        "printf '%d\\n' 9223372036854775808; echo $?",
        "9223372036854775807\n1\n",
    );
    check("printf '%u\\n' -1", "18446744073709551615\n");
    check("printf '%#.0f\\n' 3", "3.\n");
    check(
        "TZ=UTC0; export TZ; printf '%(%F %T)T\\n' 1234567890",
        "2009-02-13 23:31:30\n",
    );
    check(
        "export TZ=JST-9; printf '[%10.4(%Y)T]\\n' 0",
        "[      1970]\n",
    );
    let o = run("printf '%z'; printf '%]'");
    assert!(
        o.stderr.contains("`%z': missing format character"),
        "{}",
        o.stderr
    );
    assert!(
        o.stderr.contains("`]': invalid format character"),
        "{}",
        o.stderr
    );
}

#[test]
fn help_and_stubs() {
    check("help -s cd", "cd: cd [-L|[-P [-e]]] [-@] [dir]\n");
    check("help nope 2>/dev/null; echo $?", "1\n");
    check("logout 2>/dev/null; echo $?; history; echo $?", "1\n0\n");
    // A bash builtin psh does not have stops the script instead of running a program.
    let o = run("bind; echo after");
    assert_eq!((o.status, o.stdout.as_str()), (2, ""));
    assert!(o.stderr.contains("bind: not supported yet"), "{}", o.stderr);
    check("exit 1 2; echo same\necho next $?", "next 2\n");
}

#[test]
fn read_arrays_and_attributes() {
    check(
        "read -a a <<< ' x  y z '; echo \"${#a[@]} ${a[1]}\"",
        "3 y\n",
    );
    check("IFS='x ' read -a a <<< 'a b x x'; echo ${#a[@]}", "3\n");
    check("a=(9 9 9); read -a a <<< 1; echo ${#a[@]}", "1\n");
    check("declare -A h; read -a h <<< x 2>/dev/null; echo $?", "1\n");
    check("declare -i n; read n <<< '2*3'; echo $n", "6\n");
    check("declare -u u; read u <<< abc; echo $u", "ABC\n");
    check("read -n +2 x <<< abc; echo $x", "ab\n");
}

#[test]
fn echo_xpg() {
    check(
        "shopt -s xpg_echo; echo 'a\\tb'; echo -E 'a\\tb'",
        "a\tb\na\\tb\n",
    );
}

#[test]
fn prefix_path_bypasses_the_hash_table() {
    check(
        "cat </dev/null; PATH=/x cat </dev/null; hash",
        "hits\tcommand\n   1\t/bin/cat\n",
    );
    check("PATH=/x cat </dev/null; hash", "hash: hash table empty\n");
}

#[test]
fn umask_applies_to_created_files() {
    check(
        "cd /tmp; umask 0222; echo x > ro; [ -w ro ] && echo w || echo ro; \
         umask 0; echo x > rw; [ -w rw ] && echo w || echo ro",
        "ro\nw\n",
    );
    // An existing file keeps its bits.
    check(
        "cd /tmp; echo x > f; umask 0222; echo y > f; [ -w f ] && echo w",
        "w\n",
    );
}

#[test]
fn printf_v_targets_and_quoting() {
    check(
        "a=(a b c); printf -v 'a[1]' %s foo; echo \"${a[@]}\"",
        "a foo c\n",
    );
    check(
        "declare -A h; printf -v 'h[k y]' %s v; echo \"${h[k y]}\"",
        "v\n",
    );
    check_status("printf -v 'a[' %s x", 2);
    check_status("printf -v 'a[]' %s x", 2);
    check("readonly r; printf -v r x 2>/dev/null; echo $?", "1\n");
    check(
        "printf '%q %q %q|' '~' 'x:~' 'x#y'; printf '<%#q><%.5Q>\\n' 'a b' 'foo bar'",
        "\\~ x:\\~ x#y|<'a b'><foo\\ b>\n",
    );
}

#[test]
fn read_strips_crlf_only_where_the_host_says() {
    let crlf = |src: &str| run_setup(src, |h| h.set_strips_crlf(true)).stdout;
    let src = "printf 'a b\\r\\nc\\r' | { read x y; read -r z; echo ${#x} ${#y} ${#z}; }";
    // Elsewhere the \r stays part of the last field.
    assert_eq!(run(src).stdout, "1 2 2\n");
    // On such a host it goes before the newline; one before EOF stays.
    assert_eq!(crlf(src), "1 1 2\n");
    // With -r too; not for -n counts or another delimiter (`a\r` after IFS trims
    // the newline).
    assert_eq!(
        crlf(
            "printf 'a\\r\\n' | { read -r x; echo ${#x}; }; \
             printf 'a\\r\\n' | { read -n 3 x; echo ${#x}; }; \
             printf 'a\\r\\nb' | { read -d b x; echo ${#x}; }"
        ),
        "1\n2\n2\n"
    );
}

#[test]
fn directory_stack() {
    // The test host's HOME is /tmp, which `dirs` would show as `~`.
    check(
        "HOME=/x; cd /; pushd /tmp; pwd; dirs -v; popd; pwd",
        "/tmp /\n/tmp\n 0  /tmp\n 1  /\n/\n/\n",
    );
    check(
        "HOME=/x; cd /; pushd -n /tmp >/dev/null; pwd; dirs -p",
        "/\n/\n/tmp\n",
    );
    check(
        "cd /; pushd -n /a >/dev/null; pushd -n /b >/dev/null; dirs; pushd -n +1; dirs",
        "/ /b /a\n/ /b /a\n",
    );
    check(
        "cd /; pushd /bin >/dev/null; cd /; dirs; echo ${DIRSTACK[@]}",
        "/ /\n/ /\n",
    );
    check("cd /tmp; dirs; dirs -l", "~\n/tmp\n");
    check(
        "popd 2>/dev/null; echo $?; pushd 2>/dev/null; echo $?",
        "1\n1\n",
    );
    check_status("dirs a", 2);
    check_status("popd zzz", 2);
    check("dirs +3 2>/dev/null; echo $?", "1\n");
}

#[test]
fn ulimit_limits() {
    check(
        "ulimit; ulimit -n; ulimit -Hn; ulimit -Sc",
        "unlimited\n1024\n4096\n0\n",
    );
    check("ulimit -n 100; ulimit -n; ulimit -Hn", "100\n100\n");
    check("ulimit -Sn 99; ulimit -n; ulimit -Hn", "99\n4096\n");
    check("(ulimit -n 50); ulimit -n", "1024\n");
    check(
        "ulimit -n 100 200; ulimit -n; ulimit -n -- 90; ulimit -n",
        "100\n90\n",
    );
    check("ulimit 2000; ulimit -f; ulimit -s", "2000\n8192\n");
    check(
        "ulimit -c 0 -n",
        "open files                          (-n) 1024\n",
    );
    check(
        "ulimit -a",
        "core file size              (blocks, -c) 0\n\
         file size                   (blocks, -f) unlimited\n\
         open files                          (-n) 1024\n\
         pipe size                (512 bytes, -p) 8\n\
         stack size                  (kbytes, -s) 8192\n\
         cpu time                   (seconds, -t) unlimited\n",
    );
    check("ulimit -Hn 5000 2>/dev/null; echo $?", "1\n");
    check("ulimit -n abc 2>/dev/null; echo $?", "1\n");
    check_status("ulimit -Z", 2);
    check_status("ulimit -x", 2);
    check_status("ulimit --help", 2);
}

#[test]
fn ulimit_on_a_host_without_limits() {
    // The test host has no RLIMIT_AS; a host with none at all rejects -f too.
    check_status("ulimit -v", 2);
}
