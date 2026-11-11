//! The file utilities (0015) on the test host: an in-memory fs with symlinks, virtual
//! time (now = 2023-11-14T22:13:20Z, UTC).

use std::rc::Rc;

use psh_engine::testhost::TestHost;
use psh_engine::{Script, Shell, Step};

struct Run {
    status: i32,
    stdout: String,
    stderr: String,
    host: Rc<TestHost>,
}

fn run_with(src: &str, latency: bool, setup: &dyn Fn(&TestHost)) -> Run {
    let host = Rc::new(TestHost::new());
    host.set_latency(latency);
    host.fs.write_file("/tmp/f", b"one\ntwo\n");
    host.fs.mkdir_all("/tmp/dir/sub");
    host.fs.write_file("/tmp/dir/sub/x", b"x\n");
    host.fs.symlink("dir/sub", "/tmp/ln");
    host.fs.symlink("nowhere", "/tmp/dangling");
    setup(&host);
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
    Run {
        status,
        stdout: String::from_utf8_lossy(&host.stdout()).into_owned(),
        stderr: String::from_utf8_lossy(&host.stderr()).into_owned(),
        host,
    }
}

/// Runs with and without latency; both must agree.
fn run_setup(src: &str, setup: &dyn Fn(&TestHost)) -> Run {
    let a = run_with(src, false, setup);
    let b = run_with(src, true, setup);
    assert_eq!(
        (a.status, &a.stdout, &a.stderr),
        (b.status, &b.stdout, &b.stderr),
        "latency changed {src:?}"
    );
    a
}

fn run(src: &str) -> Run {
    run_setup(src, &|_| {})
}

#[track_caller]
fn check(src: &str, stdout: &str) {
    let r = run(src);
    assert_eq!(r.stdout, stdout, "stdout of {src:?} (stderr: {})", r.stderr);
}

#[track_caller]
fn check_all(src: &str, status: i32, stdout: &str, stderr: &str) {
    let r = run(src);
    assert_eq!(
        (r.status, r.stdout.as_str(), r.stderr.as_str()),
        (status, stdout, stderr),
        "{src:?}"
    );
}

#[test]
fn basename_dirname() {
    check("basename /a/b/c.txt .txt", "c\n");
    check("basename -s .c a.c b/d.c", "a\nd\n");
    check("basename -a x/ y//", "x\ny\n");
    check("basename .c .c; basename //", ".c\n/\n");
    check_all(
        "basename a b c",
        1,
        "",
        "basename: extra operand 'c'\nTry 'basename --help' for more information.\n",
    );
    check("dirname /a//b// a x/y", "/a\n.\nx\n");
    check("dirname -z / x/y | wc -c", "4\n");
}

#[test]
fn readlink_realpath() {
    check("readlink ln dangling", "dir/sub\nnowhere\n");
    check_all("readlink f; echo $?", 0, "1\n", "");
    check_all("readlink -v f", 1, "", "readlink: f: Invalid argument\n");
    check(
        "readlink -f ln/x ln/../f dangling",
        "/tmp/dir/sub/x\n/tmp/dir/f\n/tmp/nowhere\n",
    );
    check_all("readlink -e dangling; echo $?", 0, "1\n", "");
    check("readlink -m a/../../b", "/b\n");
    check("cd dir; readlink -f ../ln", "/tmp/dir/sub\n");
    check(
        "realpath ln f missing",
        "/tmp/dir/sub\n/tmp/f\n/tmp/missing\n",
    );
    check_all(
        "realpath missing/x f/ f/x",
        1,
        "",
        "realpath: missing/x: No such file or directory\nrealpath: f/: Not a directory\n\
         realpath: f/x: Not a directory\n",
    );
    check("realpath -m f/x/../y", "/tmp/f/y\n");
    check("realpath -s ln/..", "/tmp\n");
    check("realpath -L ln/..", "/tmp\n");
    check("realpath ln/..", "/tmp/dir\n");
    check("realpath --relative-to=dir/sub f ln", "../../f\n.\n");
    check("realpath --relative-base=/tmp f /bin", "f\n/bin\n");
    check("cd dir; realpath", "/tmp/dir\n");
    check("realpath -q nope/x; echo $?", "1\n");
}

#[test]
fn mkdir_rmdir() {
    let r = run("mkdir a; mkdir -p b/c/d; mkdir -m 700 m; mkdir -pv p/q");
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert_eq!(
        r.stdout,
        "mkdir: created directory 'p'\nmkdir: created directory 'p/q'\n"
    );
    check_all(
        "mkdir f dir x/y",
        1,
        "",
        "mkdir: cannot create directory 'f': File exists\n\
         mkdir: cannot create directory 'dir': File exists\n\
         mkdir: cannot create directory 'x/y': No such file or directory\n",
    );
    check_all(
        "mkdir -p f/x",
        1,
        "",
        "mkdir: cannot create directory 'f': Not a directory\n",
    );
    check_all(
        "mkdir -p dir/sub ./dir//new/../n2 && echo ok",
        0,
        "ok\n",
        "",
    );
    check_all("mkdir -m bad z", 1, "", "mkdir: invalid mode 'bad'\n");
    check_all(
        "rmdir dir",
        1,
        "",
        "rmdir: failed to remove 'dir': Directory not empty\n",
    );
    check_all("rmdir --ignore-fail-on-non-empty dir", 0, "", "");
    let r = run("mkdir -p a/b/c; rmdir -pv a/b/c");
    assert_eq!(
        (r.status, r.stdout.as_str()),
        (
            0,
            "rmdir: removing directory, 'a/b/c'\nrmdir: removing directory, 'a/b'\n\
             rmdir: removing directory, 'a'\n"
        )
    );
    assert!(!r.host.fs.exists("/tmp/a"));
    check_all(
        "rmdir f",
        1,
        "",
        "rmdir: failed to remove 'f': Not a directory\n",
    );
}

#[test]
fn touch() {
    let r = run("touch new f && echo ok");
    assert_eq!(r.stdout, "ok\n");
    assert_eq!(r.host.fs.read_file("/tmp/new").as_deref(), Some(&b""[..]));
    check_all("touch -c none; echo $?", 0, "0\n", "");
    check_all(
        "touch x/y",
        1,
        "",
        "touch: cannot touch 'x/y': No such file or directory\n",
    );
    check_all(
        "touch -d nonsense f",
        1,
        "",
        "touch: invalid date format 'nonsense'\n",
    );
    check_all("touch -t 99 f", 1, "", "touch: invalid date format '99'\n");
}

/// What the dates of `touch -d`/`-t` mean, read back through `ls`-free means: the
/// mtime via the test host.
#[test]
fn touch_dates() {
    use psh_engine::Host;
    let cases: &[(&str, i64)] = &[
        ("-d '2024-01-02 03:04:05'", 1_704_164_645),
        ("-d 2024-01-02T03:04:05Z", 1_704_164_645),
        ("-d '2024-01-02 03:04:05 +0100'", 1_704_164_645 - 3600),
        ("-d @1700000000", 1_700_000_000),
        ("-d '2 days ago'", 1_700_000_000 - 2 * 86400),
        ("-d '-1 hour'", 1_700_000_000 - 3600),
        ("-d '+1 week'", 1_700_000_000 + 7 * 86400),
        ("-d yesterday", 1_700_000_000 - 86400),
        ("-d 'Jan 2 2024'", 1_704_153_600),
        ("-d 2024/01/02", 1_704_153_600),
        ("-d 01/02/2024", 1_704_153_600),
        ("-d '2 Jan 2024 10:00'", 1_704_153_600 + 36000),
        ("-d '1 month ago'", 1_700_000_000 - 31 * 86400),
        ("-d '2024-01-02 3pm'", 1_704_153_600 + 15 * 3600),
        ("-t 202401020304.05", 1_704_164_645),
        ("-t 2401020304", 1_704_164_640),
        ("-r f", 0),
    ];
    for (args, want) in cases {
        let r = run(&format!("touch {args} t"));
        assert_eq!(r.status, 0, "touch {args}: {}", r.stderr);
        let fut = r.host.stat(b"/tmp/t", true);
        let m = futures_ready(fut).unwrap();
        if *want != 0 {
            assert_eq!(m.mtime_ns / 1_000_000_000, *want, "touch {args}");
        }
    }
}

/// Polls a host future that is ready at once (the test host without latency).
fn futures_ready<T>(mut f: psh_engine::host::HostFuture<T>) -> T {
    use std::task::{Context, Poll, Waker};
    let mut cx = Context::from_waker(Waker::noop());
    match f.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("pending"),
    }
}

/// A host without the file system changes (D1150): the utilities fail clearly.
#[test]
fn unsupported_host() {
    let unsupported = |h: &TestHost| h.fs.set_unsupported(true);
    let r = run_setup("mkdir a", &unsupported);
    assert_eq!(
        (r.status, r.stderr.as_str()),
        (
            1,
            "mkdir: cannot create directory 'a': Operation not supported\n"
        )
    );
    let r = run_setup("touch f", &unsupported);
    assert_eq!(
        (r.status, r.stderr.as_str()),
        (1, "touch: setting times of 'f': Operation not supported\n")
    );
    let r = run_setup("rmdir dir/sub", &unsupported);
    assert_eq!(r.status, 1);
    let r = run_setup("readlink -v ln", &unsupported);
    assert_eq!(
        (r.status, r.stderr.as_str()),
        (1, "readlink: ln: Operation not supported\n")
    );
}

#[test]
fn rm() {
    let r = run("rm f; rm -r dir; rm ln dangling; echo $?");
    assert_eq!((r.stdout.as_str(), r.stderr.as_str()), ("0\n", ""));
    assert!(!r.host.fs.exists("/tmp/f") && !r.host.fs.exists("/tmp/dir"));
    check_all(
        "rm nope; rm -f nope; echo $?",
        0,
        "0\n",
        "rm: cannot remove 'nope': No such file or directory\n",
    );
    check_all("rm dir", 1, "", "rm: cannot remove 'dir': Is a directory\n");
    check_all(
        "rm -d dir",
        1,
        "",
        "rm: cannot remove 'dir': Directory not empty\n",
    );
    check_all(
        "rm -r . dir/..",
        1,
        "",
        "rm: refusing to remove '.' or '..' directory: skipping '.'\n\
         rm: refusing to remove '.' or '..' directory: skipping 'dir/..'\n",
    );
    check_all(
        "rm -rf /",
        1,
        "",
        "rm: it is dangerous to operate recursively on '/'\n\
         rm: use --no-preserve-root to override this failsafe\n",
    );
    check(
        "rm -rv dir",
        "removed 'dir/sub/x'\nremoved directory 'dir/sub'\nremoved directory 'dir'\n",
    );
    // A symlink to a directory goes, its target stays.
    let r = run("rm -r ln");
    assert!(r.host.fs.exists("/tmp/dir/sub/x") && !r.host.fs.exists("/tmp/ln"));
    check_all(
        "rm; echo $?; rm -f; echo $?",
        0,
        "1\n0\n",
        "rm: missing operand\nTry 'rm --help' for more information.\n",
    );
}

#[test]
fn mv() {
    let r = run("mv f g && mv dir d2 && mv -v g d2");
    assert_eq!(r.stdout, "renamed 'g' -> 'd2/g'\n", "{}", r.stderr);
    assert_eq!(
        r.host.fs.read_file("/tmp/d2/g").as_deref(),
        Some(&b"one\ntwo\n"[..])
    );
    assert!(r.host.fs.exists("/tmp/d2/sub/x") && !r.host.fs.exists("/tmp/dir"));
    check_all(
        "mv dir dir/sub",
        1,
        "",
        "mv: cannot move 'dir' to a subdirectory of itself, 'dir/sub/dir'\n",
    );
    check_all(
        "mv nope x",
        1,
        "",
        "mv: cannot stat 'nope': No such file or directory\n",
    );
    check_all("mv f f", 1, "", "mv: 'f' and 'f' are the same file\n");
    check_all(
        "mv f dir/sub/x/y",
        1,
        "",
        "mv: cannot move 'f' to 'dir/sub/x/y': Not a directory\n",
    );
    check_all(
        "mv a b c",
        1,
        "",
        "mv: target 'c': No such file or directory\n",
    );
    let r = run("echo new > n; mv -n n f; cat f; mv -b n f; cat f f~");
    assert_eq!(r.stdout, "one\ntwo\nnew\none\ntwo\n");
    // A symlink moves as itself.
    let r = run("mv ln l2; readlink l2");
    assert_eq!(r.stdout, "dir/sub\n");
}

#[test]
fn cp() {
    let r = run("cp f g; cat g; cp -R dir d2; cat d2/sub/x; cp -v f dir");
    assert_eq!(r.stdout, "one\ntwo\nx\n'f' -> 'dir/f'\n", "{}", r.stderr);
    check_all(
        "cp dir e",
        1,
        "",
        "cp: -r not specified; omitting directory 'dir'\n",
    );
    check_all("cp f f", 1, "", "cp: 'f' and 'f' are the same file\n");
    check_all(
        "cp -R dir dir/sub",
        1,
        "",
        "cp: cannot copy a directory, 'dir', into itself, 'dir/sub/dir'\n",
    );
    check_all(
        "cp nope x",
        1,
        "",
        "cp: cannot stat 'nope': No such file or directory\n",
    );
    // -R keeps symlinks; plain cp follows them.
    check("mkdir d; ln -s ../f d/l; cp -R d e; readlink e/l", "../f\n");
    check("cp ln/x y; cat y", "x\n");
    check("cp -a ln l2; readlink l2", "dir/sub\n");
    // Into an existing directory, src/ too (GNU, D016).
    let r = run("mkdir t; cp -R dir/ t; cat t/dir/sub/x");
    assert_eq!(r.stdout, "x\n", "{}", r.stderr);
    check("mkdir t; cp --parents dir/sub/x t; cat t/dir/sub/x", "x\n");
    check("echo a > a; cp -n a f; cat f", "one\ntwo\n");
    check_all(
        "cp; echo $?",
        0,
        "1\n",
        "cp: missing file operand\nTry 'cp --help' for more information.\n",
    );
    check_all(
        "cp f",
        1,
        "",
        "cp: missing destination file operand after 'f'\nTry 'cp --help' for more information.\n",
    );
}

#[test]
fn ln() {
    check("ln -s f l; readlink l; ln f h; cat h", "f\none\ntwo\n");
    check_all(
        "ln -s x ln",
        1,
        "",
        "ln: failed to create symbolic link 'ln/x': File exists\n",
    );
    // -n: the symlink to a directory is replaced, not entered.
    check("ln -sfn f ln; readlink ln", "f\n");
    check("ln -sf f ln; readlink dir/sub/f", "f\n");
    check_all(
        "ln -s f dangling",
        1,
        "",
        "ln: failed to create symbolic link 'dangling': File exists\n",
    );
    check("ln -sv f l; ln -v f h", "'l' -> 'f'\n'h' => 'f'\n");
    check("ln -sr f dir/sub/l; readlink dir/sub/l", "../../f\n");
    check("cd dir; ln -s ../f; readlink f", "../f\n");
    check_all(
        "ln nope x",
        1,
        "",
        "ln: failed to access 'nope': No such file or directory\n",
    );
}

#[test]
fn chmod() {
    use psh_engine::Host;
    let mode = |r: &Run, p: &str| futures_ready(r.host.stat(p.as_bytes(), true)).unwrap().mode;
    let r = run("chmod 755 f");
    assert_eq!(mode(&r, "/tmp/f"), 0o755);
    let r = run("chmod -x,g+w f");
    assert_eq!(mode(&r, "/tmp/f"), 0o664);
    let r = run("chmod -R go-rx dir");
    assert_eq!(
        (mode(&r, "/tmp/dir"), mode(&r, "/tmp/dir/sub/x")),
        (0o700, 0o600)
    );
    check(
        "chmod -v 600 f; chmod -v 600 f; chmod -c 600 f",
        "mode of 'f' changed from 0644 (rw-r--r--) to 0600 (rw-------)\n\
         mode of 'f' retained as 0600 (rw-------)\n",
    );
    check_all(
        "chmod bad f",
        1,
        "",
        "chmod: invalid mode: 'bad'\nTry 'chmod --help' for more information.\n",
    );
    check_all(
        "chmod 644 nope",
        1,
        "",
        "chmod: cannot access 'nope': No such file or directory\n",
    );
    check_all(
        "chmod 755",
        1,
        "",
        "chmod: missing operand after '755'\nTry 'chmod --help' for more information.\n",
    );
    let r = run("chmod --reference=dir f");
    assert_eq!(mode(&r, "/tmp/f"), 0o755);
}

#[test]
fn ls() {
    check("ls", "dangling\ndir\nf\nln\n");
    check("ls -F", "dangling@\ndir/\nf\nln@\n");
    check("ls dir f", "f\n\ndir:\nsub\n");
    check("ls -R dir", "dir:\nsub\n\ndir/sub:\nx\n");
    check("ls -d dir ln", "dir\nln\n");
    // A symlink to a directory on the command line lists the directory.
    check("ls ln", "x\n");
    check("ls -1a dir", ".\n..\nsub\n");
    check(
        "touch -d '2023-11-01 10:00' f; touch -d '2020-01-02 03:04' dir/sub/x; ls -l f dir/sub/x",
        "-rw-r--r-- 1 user users 2 Jan  2  2020 dir/sub/x\n\
         -rw-r--r-- 1 user users 8 Nov  1 10:00 f\n",
    );
    check(
        "ls -l --time-style=long-iso ln",
        "lrwxrwxrwx 1 user users 7 1970-01-01 00:00 ln -> dir/sub\n",
    );
    check_all(
        "ls nope f",
        2,
        "f\n",
        "ls: cannot access 'nope': No such file or directory\n",
    );
    check("ls -m", "dangling, dir, f, ln\n");
    check("ls -C", "dangling  dir  f  ln\n");
    check("COLUMNS=10 ls -C", "dangling\ndir\nf\nln\n");
    check("ls -t -r -1 dir/sub", "x\n");
}

#[test]
fn stat() {
    check(
        "stat -c '%s %n %a %A %F %U %G' f",
        "8 f 644 -rw-r--r-- regular file user users\n",
    );
    check("stat -c '%N' ln", "'ln' -> 'dir/sub'\n");
    check(
        "touch -t 202401020304.05 f; stat -c '%Y %y' f",
        "1704164645 2024-01-02 03:04:05.000000000 +0000\n",
    );
    check(
        "stat -f '%z %N %Lp %Sp %HT' f",
        "8 f 644 -rw-r--r-- Regular File\n",
    );
    check(
        "touch -t 202401020304.05 f; stat -t '%Y-%m-%d' -f '%Sm %m' f",
        "2024-01-02 1704164645\n",
    );
    check("stat -f '%N%SY' ln", "ln -> dir/sub\n");
    check("stat -t f | cut -d' ' -f1,2", "f 8\n");
    check_all("stat -f %Q f", 1, "", "stat: %Q: bad format\n");
    check_all(
        "stat nope",
        1,
        "",
        "stat: cannot stat 'nope': No such file or directory\n",
    );
}

#[test]
fn du() {
    check("du -b -a dir", "2\tdir/sub/x\n2\tdir/sub\n2\tdir\n");
    check("du -s --apparent-size -b dir f", "2\tdir\n8\tf\n");
    check("du -c -b f dir/sub/x", "8\tf\n2\tdir/sub/x\n10\ttotal\n");
    check("du -b --exclude=x dir", "0\tdir/sub\n0\tdir\n");
    check("du -b -d 0 dir", "2\tdir\n");
    check_all(
        "du nope",
        1,
        "",
        "du: cannot access 'nope': No such file or directory\n",
    );
}

#[test]
fn mktemp() {
    // Random names: one run (latency changes them).
    let r = run_with(
        "mktemp -d dXXXX; mktemp fXXXXXX; mktemp -p /tmp/dir q.XXX",
        false,
        &|_| {},
    );
    assert_eq!(r.status, 0, "{}", r.stderr);
    let lines: Vec<&str> = r.stdout.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with('d') && lines[0].len() == 5);
    assert!(lines[2].starts_with("/tmp/dir/q.") && lines[2].len() == 14);
    use psh_engine::Host;
    let d = format!("/tmp/{}", lines[0]);
    let m = futures_ready(r.host.stat(d.as_bytes(), true)).unwrap();
    assert_eq!(m.mode, 0o700);
    let f = format!("/tmp/{}", lines[1]);
    assert_eq!(
        futures_ready(r.host.stat(f.as_bytes(), true)).unwrap().mode,
        0o600
    );
    check_all("mktemp x", 1, "", "mktemp: too few X's in template 'x'\n");
    let r = run_with("TMPDIR=/tmp/dir mktemp -t pre", true, &|_| {});
    assert!(
        r.stdout.starts_with("/tmp/dir/pre.") && r.stdout.len() == 24,
        "{}",
        r.stdout
    );
}

/// Unknown options fall through to the real program, or fail clearly without one (the
/// test host has no ls, cp, stat, ... programs).
#[test]
fn unsupported_options_fail_clearly() {
    for (cmd, opt) in [
        ("ls --frobnicate", "--frobnicate"),
        ("cp -i f g", "-i"),
        ("rm -I f", "-I"),
        ("stat -x f", "-x"),
        ("du --inodes", "--inodes"),
        ("mktemp -z", "-z"),
    ] {
        let name = cmd.split(' ').next().unwrap();
        check_all(
            cmd,
            2,
            "",
            &format!("{name}: unsupported option '{opt}' (psh built-in {name})\n"),
        );
    }
    // GNU's file-system stat needs statfs: a clear failure.
    check_all(
        "stat -f f",
        2,
        "",
        "stat: file system status (-f) is not supported (psh built-in stat)\n",
    );
    check_all(
        "ls --color=always",
        2,
        "",
        "ls: unsupported argument 'always' for '--color' (psh built-in ls)\n",
    );
}
