//! The Windows host without the engine: spawn, wait, pipes, batch files, `#!` scripts,
//! lookup, abandon, release, leftovers. The programs are the ones every Windows has
//! (`cmd.exe`, `ping.exe` as a sleep, `findstr.exe` as a cat).

use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use psh_engine::host::{ChildFd, ExitStatus, Host, SIGKILL, SIGTERM, SpawnSpec};

use super::*;

struct Unpark(std::thread::Thread);
impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `f` until ready, parking in between; panics after 30 s.
fn block<T>(mut f: impl FnMut(&mut Context<'_>) -> Poll<T>) -> T {
    let w = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&w);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Poll::Ready(v) = f(&mut cx) {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out");
        std::thread::park_timeout(Duration::from_millis(50));
    }
}

fn host() -> WindowsHost {
    WindowsHost::new(WindowsConfig {
        grace: Duration::from_millis(300),
        ..WindowsConfig::default()
    })
}

fn system32(exe: &str) -> Vec<u8> {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    path::engine_path(&std::path::Path::new(&root).join("System32").join(exe))
}

fn env() -> Vec<(Vec<u8>, Vec<u8>)> {
    std::env::vars()
        .map(|(k, v)| (k.into_bytes(), v.into_bytes()))
        .collect()
}

fn cwd() -> Vec<u8> {
    path::engine_path(&std::env::temp_dir())
}

fn cmd(script: &str, fds: Vec<(u32, ChildFd)>) -> SpawnSpec {
    SpawnSpec {
        program: system32("cmd.exe"),
        argv: vec![
            b"cmd".to_vec(),
            b"/d".to_vec(),
            b"/c".to_vec(),
            script.as_bytes().to_vec(),
        ],
        env: env(),
        cwd: cwd(),
        fds,
        ..SpawnSpec::default()
    }
}

fn nulls() -> Vec<(u32, ChildFd)> {
    vec![(0, ChildFd::Null), (1, ChildFd::Null), (2, ChildFd::Null)]
}

fn out_pipe() -> Vec<(u32, ChildFd)> {
    vec![
        (0, ChildFd::Null),
        (1, ChildFd::Pipe { input: false }),
        (2, ChildFd::Dup(1)),
    ]
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

fn output(h: &WindowsHost, spec: SpawnSpec) -> (String, ExitStatus) {
    let mut s = h.spawn(spec).unwrap();
    let text = String::from_utf8_lossy(&read_all(&s.pipes[0].1)).into_owned();
    let status = block(|cx| s.proc.poll_wait(cx));
    (text.replace("\r\n", "\n"), status)
}

/// Waits until the host has no live jobs left (bounded).
fn no_leftovers(h: &WindowsHost) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !h.leftovers().is_empty() {
        assert!(Instant::now() < deadline, "leftovers: {:?}", h.leftovers());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A scratch directory under the temp dir.
fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("psh-win-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn exit_codes_and_output() {
    let h = host();
    let mut p = h.spawn(cmd("exit 4", nulls())).unwrap().proc;
    assert_eq!(block(|cx| p.poll_wait(cx)), ExitStatus::Exited(4));
    assert!(p.pid().is_some());
    drop(p);
    let (out, st) = output(&h, cmd("echo hi& echo err>&2", out_pipe()));
    assert_eq!((out.as_str(), st), ("hi\nerr\n", ExitStatus::Exited(0)));
    let mut spec = cmd("", nulls());
    spec.program = b"C:/nonexistent/program.exe".to_vec();
    assert_eq!(h.spawn(spec).err().unwrap().kind(), io::ErrorKind::NotFound);
    no_leftovers(&h);
}

#[test]
fn input_pipe_feeds_the_child_and_ends() {
    // Repeated: a lost tail of the input showed up only now and then.
    for _ in 0..10 {
        input_pipe_round();
    }
}

fn input_pipe_round() {
    let h = host();
    let mut spec = cmd("", vec![]);
    spec.program = system32("findstr.exe");
    spec.argv = vec![b"findstr".to_vec(), b"^".to_vec()];
    spec.fds = vec![
        (0, ChildFd::Pipe { input: true }),
        (1, ChildFd::Pipe { input: false }),
        (2, ChildFd::Null),
    ];
    let mut s = h.spawn(spec).unwrap();
    let input = s.pipes.iter().find(|(n, _)| *n == 0).unwrap().1.clone();
    let output = s.pipes.iter().find(|(n, _)| *n == 1).unwrap().1.clone();
    let data: Vec<u8> = (0..20_000)
        .flat_map(|i| format!("line {i}\n").into_bytes())
        .collect();
    let mut off = 0;
    let mut got = Vec::new();
    let mut buf = [0u8; 8192];
    // Feed and drain together: findstr writes while it reads.
    while off < data.len() {
        let w = block(|cx| match input.poll_write(cx, &data[off..]) {
            Poll::Pending => match output.poll_read(cx, &mut buf) {
                Poll::Ready(Ok(n)) => {
                    got.extend_from_slice(&buf[..n]);
                    Poll::Ready(0)
                }
                _ => Poll::Pending,
            },
            Poll::Ready(r) => Poll::Ready(r.unwrap()),
        });
        off += w;
    }
    drop(input);
    s.pipes.clear();
    got.extend(read_all(&output));
    assert_eq!(block(|cx| s.proc.poll_wait(cx)), ExitStatus::Exited(0));
    let text = String::from_utf8(got).unwrap().replace("\r\n", "\n");
    assert_eq!(text.len(), data.len());
    no_leftovers(&h);
}

#[test]
fn abandon_kills_the_whole_tree() {
    let h = host();
    // A child with a grandchild that would outlive it.
    let p = h
        .spawn(cmd(
            "start /b ping -n 60 127.0.0.1 >NUL & ping -n 60 127.0.0.1 >NUL",
            nulls(),
        ))
        .unwrap()
        .proc;
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(h.leftovers().len(), 1);
    let started = Instant::now();
    drop(p);
    no_leftovers(&h);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(settle(Duration::from_secs(5)));
}

#[test]
fn kill_and_term_end_the_job() {
    let h = host();
    let mut p = h
        .spawn(cmd("ping -n 60 127.0.0.1 >NUL", nulls()))
        .unwrap()
        .proc;
    p.signal(SIGKILL);
    assert_eq!(block(|cx| p.poll_wait(cx)), ExitStatus::Signaled(SIGKILL));
    drop(p);
    let mut p = h
        .spawn(cmd("ping -n 60 127.0.0.1 >NUL", nulls()))
        .unwrap()
        .proc;
    p.signal(SIGTERM);
    let st = block(|cx| p.poll_wait(cx));
    assert!(
        matches!(st, ExitStatus::Signaled(SIGTERM | SIGKILL)),
        "{st:?}"
    );
    drop(p);
    // Host::kill reaches a run's external by its pid.
    let mut p = h
        .spawn(cmd("ping -n 60 127.0.0.1 >NUL", nulls()))
        .unwrap()
        .proc;
    h.kill(p.pid().unwrap() as i32, SIGKILL).unwrap();
    assert_eq!(block(|cx| p.poll_wait(cx)), ExitStatus::Signaled(SIGKILL));
    drop(p);
    no_leftovers(&h);
}

#[test]
fn released_externals_are_leftovers_killed_with_the_host() {
    let h = host();
    let p = h
        .spawn(cmd("ping -n 60 127.0.0.1 >NUL", nulls()))
        .unwrap()
        .proc;
    p.release();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(h.leftovers().len(), 1);
    let killer = h.killer();
    killer.kill_all();
    no_leftovers(&h);
    // Disowned and detached ones leave the run.
    let p = h
        .spawn(cmd("ping -n 3 127.0.0.1 >NUL", nulls()))
        .unwrap()
        .proc;
    p.disown();
    let mut spec = cmd("ping -n 3 127.0.0.1 >NUL", nulls());
    spec.detach = true;
    drop(h.spawn(spec).unwrap().proc);
    assert!(h.leftovers().is_empty());
}

#[test]
fn batch_files_get_their_arguments() {
    let h = host();
    let d = scratch("bat");
    let bat = d.join("show args.cmd");
    std::fs::write(&bat, "@echo off\r\necho [%1] [%2] [%3]\r\n").unwrap();
    let mut spec = cmd("", out_pipe());
    spec.program = path::engine_path(&bat);
    spec.argv = vec![
        b"show".to_vec(),
        b"a b".to_vec(),
        b"x&y".to_vec(),
        b"100%".to_vec(),
    ];
    let (out, st) = output(&h, spec);
    assert_eq!(st, ExitStatus::Exited(0));
    assert_eq!(out, "[\"a b\"] [\"x&y\"] [\"100%\"]\n");
    let mut spec = cmd("", out_pipe());
    spec.program = path::engine_path(&bat);
    spec.argv = vec![b"show".to_vec(), b"line\nbreak".to_vec()];
    assert_eq!(
        h.spawn(spec).err().unwrap().kind(),
        io::ErrorKind::InvalidInput
    );
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn lookup_uses_pathext_and_shebangs() {
    let d = scratch("lookup");
    std::fs::write(d.join("tool.cmd"), "@echo tool %*\r\n").unwrap();
    std::fs::write(d.join("tool"), "#!/bin/sh\necho sh\n").unwrap();
    std::fs::write(d.join("script"), "#!/usr/bin/env tool -x\nbody\n").unwrap();
    std::fs::write(d.join("plain"), "no shebang").unwrap();
    let dir = path::engine_path(&d);
    let mut path_var = dir.clone();
    path_var.extend_from_slice(b";C:/nonexistent");
    let found = resolve_in(b"tool", &path_var, b"C:/").unwrap();
    assert!(
        found.to_ascii_lowercase().ends_with(b"/tool.cmd"),
        "{found:?}"
    );
    let found = resolve_in(b"TOOL.CMD", &path_var, b"C:/").unwrap();
    assert!(found.ends_with(b"/TOOL.CMD") || found.ends_with(b"/tool.cmd"));
    assert!(resolve_in(b"script", &path_var, b"C:/").is_some());
    assert!(resolve_in(b"plain", &path_var, b"C:/").is_none());
    assert!(resolve_in(b"tool", b"C:/nonexistent", &dir).is_none());
    // A colon-separated PATH as bash scripts write it.
    let mut colon = b"/nonexistent:".to_vec();
    colon.extend_from_slice(&dir);
    assert!(resolve_in(b"tool", &colon, b"C:/").is_some());

    // `#!/usr/bin/env tool -x` runs tool.cmd with the script and its arguments.
    let h = host();
    let mut spec = cmd("", out_pipe());
    let mut script = dir.clone();
    script.extend_from_slice(b"/script");
    spec.program = script;
    spec.argv = vec![b"script".to_vec(), b"arg".to_vec()];
    spec.env.push((b"PATH".to_vec(), dir.clone()));
    let (out, st) = output(&h, spec);
    assert_eq!(st, ExitStatus::Exited(0));
    assert!(
        out.starts_with("tool -x ") && out.trim_end().replace('"', "").ends_with("script arg"),
        "{out}"
    );
    // `#!/bin/sh` needs psh itself.
    let mut spec = cmd("", nulls());
    let mut tool = dir.clone();
    tool.extend_from_slice(b"/tool");
    spec.program = tool;
    assert!(psh_engine::host::RunInShell::is(
        &h.spawn(spec).err().unwrap()
    ));
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn files_and_paths() {
    let h = host();
    let d = scratch("files");
    let f = d.join("a.txt");
    let fp = path::engine_path(&f);
    let w = block(|cx| h.open(&fp, OpenOptions::write()).as_mut().poll(cx)).unwrap();
    assert!(matches!(
        block(|cx| w.poll_write(cx, b"abc").map(Some)),
        Some(Ok(3))
    ));
    drop(w);
    let m = block(|cx| h.stat(&fp, true).as_mut().poll(cx)).unwrap();
    assert_eq!((m.kind, m.size, m.mode & 0o111), (FileKind::File, 3, 0));
    assert_ne!(m.ino, 0);
    let mut git_bash = b"/".to_vec();
    git_bash.push(fp[0].to_ascii_lowercase());
    git_bash.extend_from_slice(&fp[2..]);
    let m2 = block(|cx| h.stat(&git_bash, true).as_mut().poll(cx)).unwrap();
    assert_eq!((m2.dev, m2.ino), (m.dev, m.ino));
    let null = block(|cx| h.open(b"/dev/null", OpenOptions::write()).as_mut().poll(cx)).unwrap();
    assert!(matches!(
        block(|cx| null.poll_write(cx, b"x").map(Some)),
        Some(Ok(1))
    ));
    let canon = block(|cx| h.canonicalize(&fp).as_mut().poll(cx)).unwrap();
    assert!(
        !canon.starts_with(b"//?/") && canon.ends_with(b"/a.txt"),
        "{canon:?}"
    );
    let entries = block(|cx| h.read_dir(&path::engine_path(&d)).as_mut().poll(cx)).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(
        block(|cx| h
            .access(&path::engine_path(&d), Access::Execute)
            .as_mut()
            .poll(cx))
        .unwrap()
    );
    assert!(!block(|cx| h.access(&fp, Access::Execute).as_mut().poll(cx)).unwrap());
    let (r, w) = h.pipe().unwrap();
    assert!(matches!(
        block(|cx| w.poll_write(cx, b"pipe").map(Some)),
        Some(Ok(4))
    ));
    drop(w);
    assert_eq!(read_all(&r), b"pipe");
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn shell_env_names_path_and_fills_home_and_tmpdir() {
    let e = shell_env(vec![(b"Path".to_vec(), b"C:/x".to_vec())]);
    assert!(e.iter().any(|(k, v)| k == b"PATH" && v == b"C:/x"));
    assert!(e.iter().any(|(k, v)| k == b"TMPDIR" && !v.contains(&b'\\')));
    let e = shell_env(vec![(b"HOME".to_vec(), b"x".to_vec())]);
    assert!(e.iter().filter(|(k, _)| k == b"HOME").count() == 1);
}

#[test]
fn umask_now_and_creation_modes() {
    let h = host();
    assert_eq!(h.umask(), 0o022);
    let now = h.now().as_secs();
    assert!(now > 1_700_000_000, "{now}");
    let d = scratch("mode");
    let ro = path::engine_path(&d.join("ro"));
    let opts = OpenOptions {
        mode: Some(0o444),
        ..OpenOptions::write()
    };
    let f = block(|cx| h.open(&ro, opts).as_mut().poll(cx)).unwrap();
    // The creating handle still writes.
    assert!(matches!(
        block(|cx| f.poll_write(cx, b"x").map(Some)),
        Some(Ok(1))
    ));
    drop(f);
    let m = block(|cx| h.stat(&ro, true).as_mut().poll(cx)).unwrap();
    assert_eq!(m.mode & 0o222, 0);
    assert!(!block(|cx| h.access(&ro, Access::Write).as_mut().poll(cx)).unwrap());
    // An existing file keeps its attributes; a writable mode creates a normal file.
    let rw = path::engine_path(&d.join("rw"));
    let opts = OpenOptions {
        mode: Some(0o600),
        ..OpenOptions::write()
    };
    drop(block(|cx| h.open(&rw, opts).as_mut().poll(cx)).unwrap());
    let m = block(|cx| h.stat(&rw, true).as_mut().poll(cx)).unwrap();
    assert_ne!(m.mode & 0o200, 0);
    let mut perm = std::fs::metadata(d.join("ro")).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    std::fs::set_permissions(d.join("ro"), perm).unwrap();
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn host_info_in_bash_s_windows_forms() {
    let i = host().info();
    assert_eq!(i.ostype, "msys");
    assert!(i.machtype.ends_with("-pc-msys"), "{}", i.machtype);
    assert!(!i.hostname.is_empty());
    assert!(i.ppid.is_some_and(|p| p != std::process::id()));
    let embedded = WindowsHost::new(WindowsConfig {
        pid: Some(4242),
        ..WindowsConfig::default()
    });
    assert_eq!(embedded.info().ppid, Some(std::process::id()));
}

#[test]
fn file_system_changes() {
    let h = host();
    let d = scratch("fsops");
    let e = |p: &str| path::engine_path(&d.join(p));
    let run = |f: HostFuture<io::Result<()>>| {
        let mut f = f;
        block(|cx| f.as_mut().poll(cx))
    };
    run(h.mkdir(&e("a"), 0o755)).unwrap();
    assert_eq!(
        run(h.mkdir(&e("a"), 0o755)).unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    // A read-only file is removed like any other.
    std::fs::write(d.join("a/ro"), b"x").unwrap();
    run(h.set_mode(&e("a/ro"), 0o444)).unwrap();
    assert!(
        std::fs::metadata(d.join("a/ro"))
            .unwrap()
            .permissions()
            .readonly()
    );
    run(h.remove_file(&e("a/ro"))).unwrap();
    // A directory replaces an empty one, not a full one.
    run(h.mkdir(&e("b"), 0o755)).unwrap();
    run(h.mkdir(&e("c"), 0o755)).unwrap();
    std::fs::write(d.join("b/f"), b"1").unwrap();
    run(h.rename(&e("b"), &e("c"))).unwrap();
    assert!(d.join("c/f").exists() && !d.join("b").exists());
    run(h.mkdir(&e("b"), 0o755)).unwrap();
    assert!(run(h.rename(&e("b"), &e("c"))).is_err());
    // Hard links count.
    run(h.hard_link(&e("c/f"), &e("c/g"))).unwrap();
    let mut st = h.stat_extra(&e("c/f"), true);
    let (_, x) = block(|cx| st.as_mut().poll(cx)).unwrap();
    assert_eq!(x.nlink, 2);
    // A link to a directory: a symlink, or a junction without the privilege; removed as
    // the link, the target stays.
    run(h.symlink(&e("c"), &e("l"))).unwrap();
    assert!(d.join("l/f").exists());
    let mut rl = h.read_link(&e("l"));
    let target = block(|cx| rl.as_mut().poll(cx)).unwrap();
    assert!(target.ends_with(b"/c"), "{target:?}");
    run(h.remove_file(&e("l"))).unwrap();
    assert!(!d.join("l").exists() && d.join("c/f").exists());
    let mut rl = h.read_link(&e("c/f"));
    assert_eq!(
        block(|cx| rl.as_mut().poll(cx)).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    // Times.
    let t = 1_600_000_000_000_000_000i64;
    run(h.set_times(&e("c/f"), Some(t), Some(t), true)).unwrap();
    let mut st = h.stat_extra(&e("c/f"), true);
    let (m, x) = block(|cx| st.as_mut().poll(cx)).unwrap();
    assert_eq!((m.mtime_ns, x.atime_ns), (t, t));
    // Names and the time zone.
    assert!(h.user_name(1000).is_some());
    let me = h.user_name(1000).unwrap();
    assert!(h.home_dir(&me).is_some_and(|p| !p.contains(&b'\\')));
    assert!(h.utc_offset(1_700_000_000).abs() <= 14 * 3600);
    run(h.remove_dir(&e("b"))).unwrap();
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn standard_path_is_windows_own() {
    let p = host().standard_path();
    let entries = host().path_entries(&p);
    assert_eq!(entries.len(), 4, "{:?}", String::from_utf8_lossy(&p));
    assert!(!p.contains(&b'\\'));
    assert!(resolve_in(b"cmd", &p, b"C:/").is_some());
    assert!(resolve_in(b"powershell", &p, b"C:/").is_some());
}

#[test]
fn detached_externals_stay_in_a_test_runner_s_run() {
    let h = WindowsHost::new(WindowsConfig {
        grace: Duration::from_millis(300),
        detached_survive: false,
        ..WindowsConfig::default()
    });
    let mut spec = cmd("ping -n 60 127.0.0.1 >NUL", nulls());
    spec.detach = true;
    drop(h.spawn(spec).unwrap().proc);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(h.leftovers().len(), 1);
    h.kill_all();
    no_leftovers(&h);
}
