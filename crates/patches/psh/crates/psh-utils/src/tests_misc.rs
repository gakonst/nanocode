//! base64, the checksums, cmp, which, printenv and env's flags on the test host.
//! Behavior against GNU and BSD is L4.

use super::{check, check_all};

#[test]
fn base64() {
    check("base64 /tmp/g", "YSBiIGMK\n");
    check("base64 /tmp/g | base64 -d", "a b c\n");
    check("printf foobar | base64 -w 4", "Zm9v\nYmFy\n");
    check("base64 -i /tmp/g -o /tmp/o; base64 -D -i /tmp/o", "a b c\n");
    check_all(
        "echo 'Zm9v!' | base64 -d",
        "",
        1,
        "foo",
        "base64: invalid input\n",
    );
    check("echo 'Zm9v!' | base64 -d -i", "foo");
}

#[test]
fn sums() {
    check(
        "md5sum /tmp/g",
        "3da904d79fb03e6e3936ff2127039b1a  /tmp/g\n",
    );
    check("md5sum /tmp/g > /tmp/s; md5sum -c /tmp/s", "/tmp/g: OK\n");
    check("md5 -q /tmp/g", "3da904d79fb03e6e3936ff2127039b1a\n");
    check("shasum -a 256 /tmp/g | shasum -c", "/tmp/g: OK\n");
}

#[test]
fn cmp() {
    check_all("cmp /tmp/f /tmp/f; echo $?", "", 0, "0\n", "");
    check_all(
        "cmp /tmp/f /tmp/g; echo $?",
        "",
        0,
        "/tmp/f /tmp/g differ: byte 1, line 1\n1\n",
        "",
    );
    check_all(
        "head -n 1 /tmp/f > /tmp/h; cmp /tmp/f /tmp/h; echo $?",
        "",
        0,
        "1\n",
        "cmp: EOF on /tmp/h after byte 4, line 1\n",
    );
}

#[test]
fn which() {
    // A program on PATH wins (`$(which cat)` runs); a name only a utility provides
    // reports it; -a lists both.
    check("which cat", "/bin/cat\n");
    check("which base64", "base64: shell builtin\n");
    check("which -a cat", "cat: shell builtin\n/bin/cat\n");
    check("which argv", "/bin/argv\n");
    check_all("which nothere; echo $?", "", 0, "1\n", "");
}

#[test]
fn printenv() {
    check("export A=1; printenv A", "1\n");
    check_all("printenv NOPE; echo $?", "", 0, "1\n", "");
}

#[test]
fn env_flags() {
    check("env -S 'echo a  b'", "a b\n");
    check("cd /; env -C /tmp pwd", "/tmp\n");
    check_all(
        "env -C /nodir pwd; echo $?",
        "",
        0,
        "125\n",
        "env: cannot change directory to '/nodir': No such file or directory\n",
    );
}

#[test]
fn date() {
    // The test host's clock: 2023-11-14T22:13:20Z, a Tuesday.
    check("date", "Tue Nov 14 22:13:20 UTC 2023\n");
    check("date +%s", "1700000000\n");
    check("date -d tuesday +%F", "2023-11-14\n");
    check("date -d 'next tuesday' +%F", "2023-11-21\n");
    check("date -d 'last tuesday' +%F", "2023-11-07\n");
    check("date -d friday +%F", "2023-11-17\n");
    check("date -d 'last friday' +%F", "2023-11-10\n");
    check("date -d '2 friday' +%F", "2023-11-24\n");
    check("date -d 'mon 10:00' '+%F %T'", "2023-11-20 10:00:00\n");
    check("date -d yesterday +%F", "2023-11-13\n");
    check("date -d '1 hour ago' +%T", "21:13:20\n");
    check("date -d '+ 2 days' +%F", "2023-11-16\n");
    // A POSIX TZ rule needs no zone files.
    check("TZ=EST5EDT date", "Tue Nov 14 17:13:20 EST 2023\n");
    check("TZ=EST5EDT date -u +%Z", "UTC\n");
    check(
        "date -j -f %Y-%m-%d 2024-01-15 '+%F %T'",
        "2024-01-15 22:13:20\n",
    );
    check("date -v+1d -v-2H '+%F %T'", "2023-11-15 20:13:20\n");
    check_all(
        "date -d nonsense",
        "",
        1,
        "",
        "date: invalid date 'nonsense'\n",
    );
}

#[test]
fn diff() {
    check_all("diff /tmp/f /tmp/f; echo $?", "", 0, "0\n", "");
    check(
        "diff /tmp/g /tmp/nonl",
        "1c1,2\n< a b c\n---\n> x\n> y\n\\ No newline at end of file\n",
    );
    check(
        "head -n 2 /tmp/f > /tmp/h; diff -u --label a --label b /tmp/f /tmp/h",
        "--- a\n+++ b\n@@ -1,4 +1,2 @@\n one\n two\n-three\n-four\n",
    );
    check_all(
        "diff /tmp/f /tmp/nope; echo $?",
        "",
        0,
        "2\n",
        "diff: /tmp/nope: No such file or directory\n",
    );
    check("diff -q /tmp/f /tmp/g", "Files /tmp/f and /tmp/g differ\n");
}
