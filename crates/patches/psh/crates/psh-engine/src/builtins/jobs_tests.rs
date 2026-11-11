//! Tests of `wait`, `kill`, `jobs`, `disown` and `$!` on the test host (D1900).

use crate::tests::{check, check_status, run};

#[test]
fn wait_returns_job_statuses() {
    check("(exit 3) & wait $!; echo $?", "3\n");
    check("{ exit 4; } & wait %1; echo $?", "4\n");
    check("true & false & wait; echo $?", "0\n");
    check(
        "(exit 5) & p=$!; wait $p; echo $?; wait $p; echo $?",
        "5\n5\n",
    );
    check("(exit 6) & (exit 7) & wait %1 %2; echo $?", "7\n");
    check("echo x & wait -n; echo $?", "x\n0\n");
    check_status("wait -n", 127);
    check_status("wait", 0);
}

#[test]
fn wait_errors() {
    let o = run("wait 123456; echo $?; wait %3; echo $?; wait zz; echo $?");
    assert_eq!(o.stdout, "127\n127\n1\n");
    assert!(
        o.stderr
            .contains("wait: pid 123456 is not a child of this shell")
    );
    assert!(o.stderr.contains("wait: %3: no such job"));
    assert!(o.stderr.contains("wait: `zz': not a pid or valid job spec"));
    check_status("wait -z", 2);
}

#[test]
fn bang_is_the_programs_pid_for_a_simple_command() {
    // The test host numbers its fake processes from 5001.
    check("cat </dev/null & echo $!", "5001\n");
    // A compound job has a pid of its own, above every real one.
    let o = run("{ cat </dev/null; } & echo $!");
    let pid: u64 = o.stdout.trim().parse().unwrap();
    assert!(pid > 1 << 22, "{pid}");
    // Each vproc started its own jobs: a subshell cannot wait for its parent's.
    check("hang & ( wait $!; echo $? ); kill $!", "127\n");
}

#[test]
fn kill_ends_jobs() {
    check("hang & kill $!; wait $!; echo $?", "143\n");
    check("hang & kill -9 %1; wait %1; echo $?", "137\n");
    check("hang & kill -s HUP %%; wait; echo $?", "0\n");
    // A compound job ends at once; its program is released, not killed.
    check("{ hang; echo no; } & kill $!; wait $!; echo $?", "143\n");
    // Background jobs ignore SIGINT.
    check("{ true; } & kill -INT $!; wait $!; echo $?", "0\n");
}

#[test]
fn kill_errors_and_lists() {
    check("kill -l TERM KILL; kill -l 143", "15\n9\nTERM\n");
    let o = run("kill; echo $?; kill -s; echo $?; kill -s BOGUS 1; echo $?; kill %4; echo $?");
    assert_eq!(o.stdout, "2\n1\n1\n1\n");
    assert!(o.stderr.contains("kill: usage:"));
    assert!(
        o.stderr
            .contains("kill: BOGUS: invalid signal specification")
    );
    assert!(o.stderr.contains("kill: %4: no such job"));
    let o = run("kill -l | head -1");
    assert!(
        o.stdout.starts_with(" 1) SIGHUP\t 2) SIGINT"),
        "{}",
        o.stdout
    );
}

#[test]
fn jobs_lists_and_reports_once() {
    check(
        "hang & (exit 2) & wait %2; jobs; kill %1; wait; jobs",
        "[1]+  Running                    hang &\n",
    );
    check("true & wait $!; jobs; echo end", "end\n");
    check("hang & jobs -p; kill %1", "5001\n");
    check(
        "hang & jobs | cat; kill %1",
        "[1]+  Running                    hang &\n",
    );
    check("hang & ( jobs ); kill %1", "");
}

#[test]
fn disown_hides_a_job() {
    check("hang & disown; jobs; echo $?; kill $!", "0\n");
    let o = run("disown");
    assert!(o.stderr.contains("disown: current: no such job"));
    assert_eq!(o.status, 1);
}

// Scripts the host cannot start run in psh (D1300).

#[test]
fn a_file_the_host_rejects_runs_as_a_script() {
    use crate::tests::run_setup;
    let o = run_setup(
        "Y=1; export X=2; f() { :; }; /s a b; echo st=$?; cd / && ./s c",
        |h| {
            h.fs.write_file(
                "/s",
                b"echo \"$0 [$*] x=$X y=${Y-unset}\"\ntype f >/dev/null 2>&1 || echo nof\nexit 3\n",
            )
        },
    );
    assert_eq!(
        o.stdout,
        "/s [a b] x=2 y=unset\nnof\nst=3\n./s [c] x=2 y=unset\nnof\n"
    );
    let o = run_setup("/s", |h| h.fs.write_file("/s", b"nosuch\n"));
    assert!(
        o.stderr.contains("/s: line 1: nosuch: command not found"),
        "{}",
        o.stderr
    );
    let o = run_setup("/b; echo $?", |h| h.fs.write_file("/b", b"\x7fELF\x00\x01"));
    assert_eq!(o.stdout, "126\n");
    assert!(
        o.stderr
            .contains("/b: cannot execute binary file: Exec format error")
    );
}

#[test]
fn kill_on_the_shell_reaches_its_traps() {
    // `$$`, 0 and the shell's own group (D1284, D1904).
    check(
        "trap 'echo got' USR1; kill -USR1 $$; kill -10 0; kill -s USR1 -- -$$; echo end",
        "got\ngot\ngot\nend\n",
    );
}
