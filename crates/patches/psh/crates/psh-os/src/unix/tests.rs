//! The host without the engine: spawn, wait, pipes, abandon, release.

use nanocode_sys::libc;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use psh_engine::host::{ChildFd, ExitStatus, Host, SpawnSpec};

use super::*;

struct Unpark(std::thread::Thread);
impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn waker() -> Waker {
    Waker::from(Arc::new(Unpark(std::thread::current())))
}

/// Polls `f` until ready, parking in between; panics after 20 s.
fn block<T>(mut f: impl FnMut(&mut Context<'_>) -> Poll<T>) -> T {
    let w = waker();
    let mut cx = Context::from_waker(&w);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Poll::Ready(v) = f(&mut cx) {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out");
        std::thread::park_timeout(Duration::from_millis(100));
    }
}

fn host() -> UnixHost {
    UnixHost::new(UnixConfig {
        grace: Duration::from_millis(300),
        ..UnixConfig::default()
    })
}

#[test]
fn fork_path() {
    let h = host();
    h.shared
        .fork_only
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let mut p = h.spawn(sh("exit 4", nulls())).unwrap().proc;
    assert_eq!(block(|cx| p.poll_wait(cx)), ExitStatus::Exited(4));
    drop(p);
    let s = h
        .spawn(sh(
            "echo \"$(pwd)\" >&3",
            vec![
                (0, ChildFd::Null),
                (1, ChildFd::Null),
                (2, ChildFd::Null),
                (3, ChildFd::Pipe { input: false }),
            ],
        ))
        .unwrap();
    let text = String::from_utf8(read_all(&s.pipes[0].1)).unwrap();
    assert_eq!(text, "/\n");
    drop(s);
    let mut spec = sh("", nulls());
    spec.program = b"/nonexistent/program".to_vec();
    assert_eq!(h.spawn(spec).err().unwrap().kind(), io::ErrorKind::NotFound);
    let mut spec = sh("", nulls());
    spec.cwd = b"/nonexistent".to_vec();
    assert_eq!(h.spawn(spec).err().unwrap().kind(), io::ErrorKind::NotFound);
    let mut p = h
        .spawn(sh(
            "exec yes",
            vec![
                (0, ChildFd::Null),
                (1, ChildFd::Pipe { input: false }),
                (2, ChildFd::Null),
            ],
        ))
        .unwrap();
    drop(p.pipes);
    assert_eq!(block(|cx| p.proc.poll_wait(cx)), ExitStatus::Signaled(13));
    drop(p.proc);
    let p = h.spawn(sh("sleep 30", nulls())).unwrap().proc;
    drop(p);
    no_leftovers(&h);
}

fn sh(script: &str, fds: Vec<(u32, ChildFd)>) -> SpawnSpec {
    SpawnSpec {
        program: b"/bin/sh".to_vec(),
        argv: vec![b"sh".to_vec(), b"-c".to_vec(), script.as_bytes().to_vec()],
        env: vec![(b"PATH".to_vec(), b"/usr/bin:/bin".to_vec())],
        cwd: b"/".to_vec(),
        fds,
        ..SpawnSpec::default()
    }
}

fn nulls() -> Vec<(u32, ChildFd)> {
    vec![(0, ChildFd::Null), (1, ChildFd::Null), (2, ChildFd::Null)]
}

fn read_all(f: &Rc<dyn HostFile>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = block(|cx| f.poll_read(cx, &mut buf)).unwrap();
        if n == 0 {
            return out;
        }
        out.extend_from_slice(&buf[..n]);
    }
}

/// Waits until the host has no live groups left (bounded).
fn no_leftovers(h: &UnixHost) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let left = h.leftovers();
        if left.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "leaked groups: {left:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn exit_status_and_signal_death() {
    let h = host();
    let mut p = h.spawn(sh("exit 3", nulls())).unwrap().proc;
    assert_eq!(block(|cx| p.poll_wait(cx)), ExitStatus::Exited(3));
    drop(p);
    let mut p = h.spawn(sh("kill -TERM $$", nulls())).unwrap().proc;
    assert_eq!(block(|cx| p.poll_wait(cx)), ExitStatus::Signaled(15));
    drop(p);
    no_leftovers(&h);
}

#[test]
fn output_pipe_and_shared_dup() {
    let h = host();
    let s = h
        .spawn(sh(
            "echo out; echo err >&2",
            vec![
                (0, ChildFd::Null),
                (1, ChildFd::Pipe { input: false }),
                (2, ChildFd::Dup(1)),
            ],
        ))
        .unwrap();
    let mut p = s.proc;
    let (n, out) = &s.pipes[0];
    assert_eq!(*n, 1);
    assert_eq!(read_all(out), b"out\nerr\n");
    assert_eq!(block(|cx| p.poll_wait(cx)), ExitStatus::Exited(0));
}

#[test]
fn input_pipe_and_extra_fd() {
    let h = host();
    let s = h
        .spawn(sh(
            "cat; echo three >&3",
            vec![
                (0, ChildFd::Pipe { input: true }),
                (1, ChildFd::Pipe { input: false }),
                (2, ChildFd::Null),
                (3, ChildFd::Dup(1)),
            ],
        ))
        .unwrap();
    let mut p = s.proc;
    let input = s.pipes.iter().find(|(n, _)| *n == 0).unwrap().1.clone();
    let output = s.pipes.iter().find(|(n, _)| *n == 1).unwrap().1.clone();
    let mut data: &[u8] = b"hello\n";
    while !data.is_empty() {
        let n = block(|cx| input.poll_write(cx, data)).unwrap();
        data = &data[n..];
    }
    drop(s.pipes);
    drop(input);
    assert_eq!(read_all(&output), b"hello\nthree\n");
    assert_eq!(block(|cx| p.poll_wait(cx)), ExitStatus::Exited(0));
}

#[test]
fn cwd_env_argv0() {
    let h = host();
    let mut spec = sh(
        "echo \"$0 $(pwd) $X\"",
        vec![
            (0, ChildFd::Null),
            (1, ChildFd::Pipe { input: false }),
            (2, ChildFd::Null),
        ],
    );
    spec.argv[0] = b"named".to_vec();
    spec.argv[2] = b"echo \"$X $(pwd)\"".to_vec();
    spec.cwd = b"/tmp".to_vec();
    spec.env.push((b"X".to_vec(), b"y".to_vec()));
    let s = h.spawn(spec).unwrap();
    let out = read_all(&s.pipes[0].1);
    let text = String::from_utf8(out).unwrap();
    assert!(text == "y /tmp\n" || text == "y /private/tmp\n", "{text}");
}

#[test]
fn spawn_errors() {
    let h = host();
    let mut spec = sh("", nulls());
    spec.program = b"/nonexistent/program".to_vec();
    let e = h.spawn(spec).err().unwrap();
    assert_eq!(e.kind(), io::ErrorKind::NotFound);
    let mut spec = sh("", nulls());
    spec.program = b"/".to_vec();
    let e = h.spawn(spec).err().unwrap();
    assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
}

#[test]
fn sigpipe_is_default_in_children() {
    // Rust ignores SIGPIPE; the child must get it back, or `yes | head` never ends.
    let h = host();
    let s = h
        .spawn(sh(
            "exec yes",
            vec![
                (0, ChildFd::Null),
                (1, ChildFd::Pipe { input: false }),
                (2, ChildFd::Null),
            ],
        ))
        .unwrap();
    let mut p = s.proc;
    let out = s.pipes[0].1.clone();
    drop(s.pipes);
    let mut buf = [0u8; 16];
    block(|cx| out.poll_read(cx, &mut buf)).unwrap();
    drop(out);
    assert_eq!(block(|cx| p.poll_wait(cx)), ExitStatus::Signaled(13));
}

#[test]
fn abandon_kills_the_whole_group() {
    let h = host();
    // The child ignores TERM and has a child of its own: KILL after the grace period
    // reaches both.
    let p = h
        .spawn(sh("trap '' TERM; sleep 30 & sleep 30; wait", nulls()))
        .unwrap()
        .proc;
    std::thread::sleep(Duration::from_millis(100));
    let start = Instant::now();
    drop(p);
    assert!(!h.leftovers().is_empty());
    no_leftovers(&h);
    assert!(start.elapsed() >= Duration::from_millis(250));
}

#[test]
fn signal_reaches_the_group() {
    let h = host();
    let mut p = h
        .spawn(sh("sleep 30 & sleep 30; wait", nulls()))
        .unwrap()
        .proc;
    std::thread::sleep(Duration::from_millis(100));
    p.signal(libc::SIGKILL);
    assert_eq!(block(|cx| p.poll_wait(cx)), ExitStatus::Signaled(9));
    drop(p);
    no_leftovers(&h);
}

#[test]
fn release_lets_it_run_and_reaps() {
    let h = host();
    let p = h.spawn(sh("sleep 0.2", nulls())).unwrap().proc;
    let g = p.pid().unwrap() as i32;
    p.release();
    // Still part of the run until it exits; the waiter reaps it.
    assert_eq!(h.leftovers(), vec![g]);
    no_leftovers(&h);
}

#[test]
fn many_spawns_reuse_waiters() {
    let h = host();
    for i in 0..50 {
        let mut p = h
            .spawn(sh(&format!("exit {}", i % 7), nulls()))
            .unwrap()
            .proc;
        assert_eq!(block(|cx| p.poll_wait(cx)), ExitStatus::Exited(i % 7));
    }
    no_leftovers(&h);
}

#[test]
fn fifo_open_waits_off_thread_and_cancels() {
    let dir = std::env::temp_dir().join(format!("psh-os-fifo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("f");
    let c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let h = host();
    let p = fifo.as_os_str().as_bytes().to_vec();
    // An open nobody answers is pending, and dropping it does not leave a thread stuck.
    let mut f = h.open(&p, OpenOptions::read());
    let w = waker();
    assert!(f.as_mut().poll(&mut Context::from_waker(&w)).is_pending());
    drop(f);
    // A reader and a writer meet.
    let mut r = h.open(&p, OpenOptions::read());
    assert!(r.as_mut().poll(&mut Context::from_waker(&w)).is_pending());
    let mut wf = h.open(&p, OpenOptions::write());
    let wr = block(|cx| wf.as_mut().poll(cx)).unwrap();
    let rd = block(|cx| r.as_mut().poll(cx)).unwrap();
    assert!(block(|cx| wr.poll_write(cx, b"x")).is_ok());
    drop(wr);
    assert_eq!(read_all(&rd), b"x");
    std::fs::remove_dir_all(&dir).unwrap();
}

fn piped_out() -> Vec<(u32, ChildFd)> {
    vec![
        (0, ChildFd::Null),
        (1, ChildFd::Pipe { input: false }),
        (2, ChildFd::Null),
    ]
}

#[test]
fn ignored_signals_and_umask_take_effect() {
    let h = host();
    let mut spec = sh(
        "kill -INT $$; kill -USR1 $$; echo alive; umask",
        piped_out(),
    );
    // Linux numbers (D1283): 10 is USR1 on every platform.
    spec.sig_ignore = vec![2, 10];
    spec.umask = Some(0o077);
    let s = h.spawn(spec).unwrap();
    let mut p = s.proc;
    assert_eq!(read_all(&s.pipes[0].1), b"alive\n0077\n");
    assert_eq!(block(|cx| p.poll_wait(cx)), ExitStatus::Exited(0));
    // Without them: INT kills, the umask is the process's.
    let s = h
        .spawn(sh("kill -INT $$; echo alive", piped_out()))
        .unwrap();
    let mut p = s.proc;
    assert_eq!(read_all(&s.pipes[0].1), b"");
    assert_eq!(block(|cx| p.poll_wait(cx)), ExitStatus::Signaled(2));
    let mut spec = sh("umask", piped_out());
    spec.umask = Some(super::spawn::process_umask());
    let s = h.spawn(spec).unwrap();
    let want = format!("{:04o}\n", super::spawn::process_umask());
    assert_eq!(read_all(&s.pipes[0].1), want.as_bytes());
}

#[test]
fn rlimits_are_read_and_given_to_children() {
    use psh_engine::host::Rlimit;
    let h = host();
    let (soft, hard) = h
        .rlimit(Rlimit::NoFile)
        .expect("every Unix has RLIMIT_NOFILE");
    assert!(soft.is_some() || hard.is_none());
    // Lower than the process's own soft limit: always allowed.
    let low = soft.map_or(64, |s| s.min(64)) - 1;
    let mut spec = sh("ulimit -n", piped_out());
    spec.rlimits = vec![(Rlimit::NoFile, Some(low), hard)];
    let s = h.spawn(spec).unwrap();
    let mut p = s.proc;
    assert_eq!(read_all(&s.pipes[0].1), format!("{low}\n").as_bytes());
    assert_eq!(block(|cx| p.poll_wait(cx)), ExitStatus::Exited(0));
    // The process's own limit is untouched.
    assert_eq!(h.rlimit(Rlimit::NoFile), Some((soft, hard)));
}

#[test]
fn os_pipe_between_two_externals() {
    let h = host();
    let (r, w) = h.pipe().unwrap();
    let a = h
        .spawn(sh(
            "seq 1 50000",
            vec![
                (0, ChildFd::Null),
                (1, ChildFd::File(w)),
                (2, ChildFd::Null),
            ],
        ))
        .unwrap();
    let b = h
        .spawn(sh(
            "wc -l",
            vec![
                (0, ChildFd::File(r)),
                (1, ChildFd::Pipe { input: false }),
                (2, ChildFd::Null),
            ],
        ))
        .unwrap();
    let out = String::from_utf8(read_all(&b.pipes[0].1)).unwrap();
    assert_eq!(out.trim(), "50000");
    let (mut pa, mut pb) = (a.proc, b.proc);
    assert!(pa.pid().is_some());
    assert_eq!(block(|cx| pa.poll_wait(cx)), ExitStatus::Exited(0));
    assert_eq!(block(|cx| pb.poll_wait(cx)), ExitStatus::Exited(0));
}

#[test]
fn kill_reaches_a_pid() {
    let h = host();
    let mut p = h.spawn(sh("exec sleep 30", nulls())).unwrap().proc;
    let pid = p.pid().unwrap() as i32;
    h.kill(pid, libc::SIGTERM).unwrap();
    assert_eq!(block(|cx| p.poll_wait(cx)), ExitStatus::Signaled(15));
    assert!(h.kill(-1_000_000, 0).is_err());
}

#[test]
fn an_embedded_host_refuses_to_signal_its_embedder() {
    let h = UnixHost::new(UnixConfig {
        pid: Some(99_999_999),
        ..UnixConfig::default()
    });
    let me = std::process::id() as i32;
    // SAFETY: plain syscall.
    let group = unsafe { libc::getpgrp() };
    for target in [0, -1, me, -group] {
        let e = h.kill(target, 0).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{target}");
    }
    // A standalone host may (bash's `kill $$`).
    assert!(host().kill(me, 0).is_ok());
}

fn alive(g: i32) -> bool {
    match kill(-g, 0) {
        Ok(()) => true,
        Err(e) => e.kind() == io::ErrorKind::PermissionDenied,
    }
}

/// Waits until group `g` is gone (killed members are reaped by init a moment later).
fn wait_gone(g: i32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(g) {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

#[test]
fn dropping_the_host_kills_released_and_leftover_groups_but_not_detached_or_disowned() {
    let h = host();
    let released = h.spawn(sh("trap '' TERM; sleep 30", nulls())).unwrap().proc;
    let r = released.pid().unwrap() as i32;
    released.release();
    // A finished external that left a child in its session.
    let mut parent = h
        .spawn(sh("sleep 30 >/dev/null 2>&1 &", nulls()))
        .unwrap()
        .proc;
    let p = parent.pid().unwrap() as i32;
    assert_eq!(block(|cx| parent.poll_wait(cx)), ExitStatus::Exited(0));
    drop(parent);
    let mut spec = sh("sleep 30", nulls());
    spec.detach = true;
    let detached = h.spawn(spec).unwrap().proc;
    let d = detached.pid().unwrap() as i32;
    drop(detached); // released, not abandoned
    let disowned = h.spawn(sh("sleep 30", nulls())).unwrap().proc;
    let o = disowned.pid().unwrap() as i32;
    disowned.disown();
    std::thread::sleep(Duration::from_millis(100));
    assert!(alive(r) && alive(p) && alive(d) && alive(o));
    drop(h);
    assert!(settle(Duration::from_secs(5)));
    assert!(wait_gone(r), "released job survived");
    assert!(wait_gone(p), "leftover grandchild survived");
    assert!(alive(d) && alive(o), "detached or disowned was killed");
    kill(-d, libc::SIGKILL).unwrap();
    kill(-o, libc::SIGKILL).unwrap();
}

#[test]
fn keep_leaves_the_run_alone() {
    let h = UnixHost::new(UnixConfig {
        leftovers: Leftovers::Keep,
        ..UnixConfig::default()
    });
    let p = h.spawn(sh("sleep 30", nulls())).unwrap().proc;
    let g = p.pid().unwrap() as i32;
    p.release();
    drop(h);
    std::thread::sleep(Duration::from_millis(100));
    assert!(alive(g));
    kill(-g, libc::SIGKILL).unwrap();
}

#[test]
fn umask_is_the_process_umask_and_now_is_the_clock() {
    let h = host();
    let s = h.spawn(sh("umask", piped_out())).unwrap();
    let out = String::from_utf8(read_all(&s.pipes[0].1)).unwrap();
    assert_eq!(u32::from_str_radix(out.trim(), 8).unwrap(), h.umask());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    assert!(h.now().abs_diff(now) < Duration::from_secs(5));
}

#[test]
fn open_with_mode_sets_exact_bits_on_create_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("psh-os-mode-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let h = host();
    let open = |name: &str, opts: OpenOptions| {
        let p = dir.join(name);
        let mut f = h.open(p.as_os_str().as_bytes(), opts);
        block(|cx| f.as_mut().poll(cx))
    };
    let mode = |name: &str| {
        std::fs::metadata(dir.join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    // Looser than any usual process umask: still exact.
    open(
        "loose",
        OpenOptions {
            mode: Some(0o666),
            ..OpenOptions::write()
        },
    )
    .unwrap();
    assert_eq!(mode("loose"), 0o666);
    open(
        "tight",
        OpenOptions {
            mode: Some(0o600),
            ..OpenOptions::append()
        },
    )
    .unwrap();
    assert_eq!(mode("tight"), 0o600);
    // An existing file keeps its bits and is truncated as asked.
    std::fs::write(dir.join("tight"), b"data").unwrap();
    let f = open(
        "tight",
        OpenOptions {
            mode: Some(0o644),
            ..OpenOptions::write()
        },
    )
    .unwrap();
    drop(f);
    assert_eq!(mode("tight"), 0o600);
    assert_eq!(std::fs::read(dir.join("tight")).unwrap(), b"");
    let e = open(
        "tight",
        OpenOptions {
            mode: Some(0o644),
            create_new: true,
            ..OpenOptions::write()
        },
    )
    .err()
    .unwrap();
    assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
    // No mode: the process's umask applies as usual.
    open("plain", OpenOptions::write()).unwrap();
    assert_eq!(mode("plain"), 0o666 & !h.umask());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_umask_probe_agrees() {
    assert_eq!(
        super::spawn::umask_probe(),
        Some(super::spawn::process_umask())
    );
}

#[test]
fn info_matches_the_system() {
    let h = host();
    let i = h.info();
    let out = |cmd: &str| {
        let s = h.spawn(sh(cmd, piped_out())).unwrap();
        String::from_utf8(read_all(&s.pipes[0].1)).unwrap()
    };
    assert_eq!(out("id -u").trim(), i.uid.to_string());
    assert_eq!(out("id -u").trim(), i.euid.to_string());
    let groups: Vec<u32> = out("id -G")
        .split_whitespace()
        .map(|g| g.parse().unwrap())
        .collect();
    for g in &groups {
        assert!(i.groups.contains(g), "{g} not in {:?}", i.groups);
    }
    assert_eq!(out("hostname").trim().as_bytes(), &i.hostname[..]);
    assert_eq!(i.ppid, Some(unsafe_free_ppid()));
    assert_eq!(i.hosttype, std::env::consts::ARCH);
    if cfg!(target_os = "macos") {
        assert_eq!(i.ostype, format!("darwin{}", out("uname -r").trim()));
        assert!(
            i.machtype
                .starts_with(&format!("{}-apple-darwin", i.hosttype))
        );
    }
    if cfg!(all(target_os = "linux", target_env = "gnu")) {
        assert_eq!(i.ostype, "linux-gnu");
    }
    // Embedded (a fake `$$`): the parent is the embedding process.
    let e = UnixHost::new(UnixConfig {
        pid: Some(99_999_999),
        ..UnixConfig::default()
    });
    assert_eq!(e.info().ppid, Some(std::process::id()));
}

/// The test process's parent, from `ps` (no unsafe in the tests).
fn unsafe_free_ppid() -> u32 {
    let o = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).trim().parse().unwrap()
}

#[test]
fn a_runner_host_kills_detached_externals_too() {
    let h = UnixHost::new(UnixConfig {
        grace: Duration::from_millis(300),
        detached_survive: false,
        ..UnixConfig::default()
    });
    let mut spec = sh("sleep 30", nulls());
    spec.detach = true;
    let p = h.spawn(spec).unwrap().proc;
    let g = p.pid().unwrap() as i32;
    drop(p);
    assert!(alive(g));
    drop(h);
    assert!(
        wait_gone(g),
        "the detached process survived a runner's host"
    );
}

#[test]
fn standard_path_is_the_systems() {
    let p = host().standard_path();
    if cfg!(target_os = "linux") {
        assert_eq!(p, b"/bin:/usr/bin");
    } else if cfg!(target_os = "macos") {
        assert_eq!(p, b"/usr/bin:/bin:/usr/sbin:/sbin");
    }
}
