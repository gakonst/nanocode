//! The walker parity test (D1132): the native walk (`ignore::walk::Walk`) and the virtual
//! walk give the same entries and errors, in the same order, over fixture trees.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::ignore::WalkState;
use super::ignore::types::TypesBuilder;
use psh_engine::host::{DirEntry, FileKind, Metadata};

use super::walk::{WalkHost, WalkOptions, Walker, os_bytes, to_path};

/// `std::fs` as a [`WalkHost`]: shell paths are OS paths.
struct FsHost {
    cwd: Vec<u8>,
    env: Vec<(Vec<u8>, Vec<u8>)>,
    native: bool,
}

fn metadata(m: &std::fs::Metadata) -> Metadata {
    let t = m.file_type();
    let kind = super::walk::file_kind(t);
    #[cfg(unix)]
    let (dev, ino) = {
        use std::os::unix::fs::MetadataExt;
        (m.dev(), m.ino())
    };
    #[cfg(not(unix))]
    let (dev, ino) = (0, 0);
    Metadata {
        kind,
        size: m.len(),
        mode: 0o644,
        mtime_ns: 0,
        dev,
        ino,
        uid: 0,
        gid: 0,
    }
}

impl WalkHost for FsHost {
    fn cwd(&self) -> &[u8] {
        &self.cwd
    }

    fn var(&self, name: &[u8]) -> Option<&[u8]> {
        self.env
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_slice())
    }

    fn native_path(&self, abs: &[u8]) -> Option<io::Result<PathBuf>> {
        self.native.then(|| Ok(to_path(abs)))
    }

    fn cancelled(&self) -> bool {
        false
    }

    fn stat(&self, abs: &[u8], follow: bool) -> io::Result<Metadata> {
        let p = to_path(abs);
        let m = if follow {
            std::fs::metadata(p)?
        } else {
            std::fs::symlink_metadata(p)?
        };
        Ok(metadata(&m))
    }

    fn read_dir(&self, abs: &[u8]) -> io::Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(to_path(abs))? {
            out.push(DirEntry {
                name: os_bytes(&e?.file_name()),
            });
        }
        Ok(out)
    }

    fn read_file(&self, abs: &[u8]) -> io::Result<Vec<u8>> {
        std::fs::read(to_path(abs))
    }
}

/// A fresh directory under the system's temp directory.
fn tempdir(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let p = std::env::temp_dir().join(format!(
        "psh-walk-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    // Canonical, so that both walks see the same parents (not on Windows, where that
    // gives a `\\?\` path, which takes no `/` separators).
    if cfg!(windows) {
        p
    } else {
        p.canonicalize().unwrap()
    }
}

fn write(root: &Path, rel: &str, data: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, data).unwrap();
}

/// The fixture: a repository with nested ignore files of every kind.
fn fixture() -> PathBuf {
    let t = tempdir("fx");
    let r = t.join("repo");
    std::fs::create_dir_all(r.join(".git/info")).unwrap();
    write(&r, ".git/info/exclude", "excluded.txt\n");
    write(&r, ".git/HEAD", "ref: refs/heads/main\n");
    write(
        &r,
        ".gitignore",
        "*.log\n!keep.log\nbuild/\n/anchored.txt\nsub/deep/*.tmp\n# comment\n\\#lit\n",
    );
    write(&r, ".ignore", "by_dot_ignore\n");
    write(&r, ".rgignore", "by_custom\n");
    for f in [
        "a.txt",
        "b.log",
        "keep.log",
        "excluded.txt",
        "anchored.txt",
        "by_dot_ignore",
        "by_custom",
        "#lit",
        "main.rs",
        "lib.py",
        "UPPER.LOG",
        ".hidden",
        ".hiddendir/x.txt",
        "build/out.txt",
        "sub/anchored.txt",
        "sub/b.log",
        "sub/local.txt",
        "sub/deep/x.tmp",
        "sub/deep/y.txt",
        "sub/deep/deeper/z.rs",
        "other/build",
    ] {
        write(&r, f, "x\n");
    }
    write(&r, "sub/.gitignore", "!b.log\nlocal.txt\n");
    write(&r, "sub/deep/.ignore", "!x.tmp\n");
    // A parent with rules and a child without its own repository.
    let o = t.join("outer");
    std::fs::create_dir_all(o.join(".git")).unwrap();
    write(&o, ".gitignore", "parent_ignored.txt\nproj/sub/x\n");
    write(&o, "proj/parent_ignored.txt", "x\n");
    write(&o, "proj/kept.txt", "x\n");
    write(&o, "proj/sub/x", "x\n");
    write(&o, "proj/sub/y", "x\n");
    // No repository at all: git rules apply only without require_git.
    let n = t.join("norepo");
    write(&n, ".gitignore", "*.txt\n");
    write(&n, "a.txt", "x\n");
    write(&n, "b.md", "x\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink("sub", r.join("link")).unwrap();
        symlink("a.txt", r.join("filelink")).unwrap();
        symlink("nowhere", r.join("broken")).unwrap();
        symlink("..", r.join("sub/deep/loop")).unwrap();
    }
    t
}

fn walk(host: &FsHost, opts: &WalkOptions, roots: &[&str]) -> (bool, Vec<String>) {
    let w = Walker::new(host, opts.clone()).unwrap();
    let native = w.is_native();
    let roots: Vec<Vec<u8>> = roots.iter().map(|r| r.as_bytes().to_vec()).collect();
    let mut out = Vec::new();
    w.run(&roots, &mut |r| {
        out.push(match r {
            Ok(e) => {
                let kind = match e.kind {
                    FileKind::Dir => "d",
                    FileKind::File => "f",
                    FileKind::Symlink => "l",
                    _ => "o",
                };
                let mut s = format!(
                    "{} {} {kind}{}",
                    e.depth,
                    String::from_utf8_lossy(&e.path),
                    if e.path_is_symlink { "@" } else { "" }
                );
                if let Some(err) = &e.err {
                    s.push_str(&format!(" !{err}"));
                }
                s
            }
            Err(e) => format!("error: {e}"),
        });
        WalkState::Continue
    });
    (native, out)
}

fn process_env() -> Vec<(Vec<u8>, Vec<u8>)> {
    [
        "HOME",
        "USERPROFILE",
        "XDG_CONFIG_HOME",
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_SYSTEM",
    ]
    .iter()
    .filter_map(|k| std::env::var_os(k).map(|v| (k.as_bytes().to_vec(), os_bytes(&v))))
    .collect()
}

fn hosts(cwd: &Path) -> (FsHost, FsHost) {
    let cwd = os_bytes(cwd.as_os_str());
    let mk = |native| FsHost {
        cwd: cwd.clone(),
        env: process_env(),
        native,
    };
    (mk(true), mk(false))
}

/// Both walks agree; returns the entries.
fn parity(cwd: &Path, opts: &WalkOptions, roots: &[&str]) -> Vec<String> {
    let (native, virt) = hosts(cwd);
    let (was_native, a) = walk(&native, opts, roots);
    assert!(was_native, "the native host walks natively");
    let (was_native, b) = walk(&virt, opts, roots);
    assert!(!was_native);
    assert_eq!(
        a, b,
        "native (left) and virtual (right) differ for {roots:?} {opts:?}"
    );
    a
}

fn opts() -> WalkOptions {
    WalkOptions {
        // The machine's global gitignore is not part of the fixture.
        git_global: false,
        ..WalkOptions::default()
    }
}

#[test]
fn parity_over_fixtures() {
    let t = fixture();
    let repo = t.join("repo");
    let base = opts();
    let all = WalkOptions {
        hidden: false,
        ignore: false,
        git_ignore: false,
        git_exclude: false,
        parents: false,
        ..opts()
    };
    let mut rust = TypesBuilder::new();
    rust.add_defaults();
    rust.select("rust");
    let types = rust.build().unwrap();
    let cases: Vec<(WalkOptions, Vec<&str>)> = vec![
        (base.clone(), vec!["."]),
        (base.clone(), vec!["./"]),
        (
            base.clone(),
            vec!["sub", "./sub/", "sub/deep", "a.txt", "missing", "b.log"],
        ),
        (
            WalkOptions {
                hidden: false,
                ..base.clone()
            },
            vec!["."],
        ),
        (all.clone(), vec!["."]),
        (
            WalkOptions {
                git_ignore: false,
                ..base.clone()
            },
            vec!["."],
        ),
        (
            WalkOptions {
                git_exclude: false,
                ignore: false,
                ..base.clone()
            },
            vec!["."],
        ),
        (
            WalkOptions {
                custom_ignore_filenames: vec![".rgignore".into()],
                ..base.clone()
            },
            vec!["."],
        ),
        (
            WalkOptions {
                ignore_case_insensitive: true,
                ..base.clone()
            },
            vec!["."],
        ),
        (
            WalkOptions {
                globs: vec![("*.txt".into(), false)],
                ..base.clone()
            },
            vec!["."],
        ),
        (
            WalkOptions {
                globs: vec![("!*.txt".into(), false), ("!sub".into(), false)],
                ..base.clone()
            },
            vec!["."],
        ),
        (
            WalkOptions {
                globs: vec![("sub/**".into(), false), ("{a,keep}.*".into(), false)],
                ..base.clone()
            },
            vec!["."],
        ),
        (
            WalkOptions {
                globs: vec![("*.LOG".into(), true)],
                hidden: false,
                ..base.clone()
            },
            vec!["."],
        ),
        (
            WalkOptions {
                types: Some(types.clone()),
                ..base.clone()
            },
            vec!["."],
        ),
        (
            WalkOptions {
                follow_links: true,
                ..base.clone()
            },
            vec!["."],
        ),
        (
            WalkOptions {
                follow_links: true,
                ..all.clone()
            },
            vec!["link", "filelink"],
        ),
        (base.clone(), vec!["link", "filelink", "broken"]),
        (
            WalkOptions {
                max_depth: Some(1),
                ..base.clone()
            },
            vec!["."],
        ),
        (
            WalkOptions {
                max_depth: Some(0),
                ..base.clone()
            },
            vec!["sub", "a.txt"],
        ),
        (
            WalkOptions {
                min_depth: Some(2),
                ..base.clone()
            },
            vec!["."],
        ),
        (
            WalkOptions {
                max_filesize: Some(1),
                ..base.clone()
            },
            vec!["."],
        ),
        (
            WalkOptions {
                same_file_system: true,
                ..base.clone()
            },
            vec!["."],
        ),
    ];
    for (o, roots) in &cases {
        parity(&repo, o, roots);
    }
    // An absolute root, and one through `..`.
    let abs = repo.join("sub");
    let abs = abs.to_str().unwrap();
    parity(&repo, &base, &[abs]);
    parity(&repo.join("sub"), &base, &["../sub/deep", ".."]);
    // Parent directories' rules, with and without `parents`.
    let proj = t.join("outer/proj");
    let got = parity(&proj, &base, &["."]);
    assert!(!got.iter().any(|l| l.contains("parent_ignored")), "{got:?}");
    assert!(!got.iter().any(|l| l.contains("sub/x")), "{got:?}");
    let got = parity(
        &proj,
        &WalkOptions {
            parents: false,
            ..base.clone()
        },
        &["."],
    );
    assert!(got.iter().any(|l| l.contains("parent_ignored")), "{got:?}");
    parity(&t.join("outer"), &base, &["proj", "proj/sub"]);
    // Outside a repository.
    let norepo = t.join("norepo");
    let got = parity(&norepo, &base, &["."]);
    assert!(got.iter().any(|l| l.ends_with("a.txt f")), "{got:?}");
    let got = parity(
        &norepo,
        &WalkOptions {
            require_git: false,
            ..base.clone()
        },
        &["."],
    );
    assert!(!got.iter().any(|l| l.contains("a.txt")), "{got:?}");

    // What the default walk of the repository finds, spelled out once.
    let got = parity(&repo, &base, &["."]);
    let _ = &got;
    // (`#lit` is ignored by `\#lit`; `.git`, `.hidden` and `.hiddendir` are hidden.)
    #[cfg(unix)]
    {
        let got: Vec<&str> = got.iter().map(String::as_str).collect();
        let want = [
            "0 . d",
            "1 ./UPPER.LOG f",
            "1 ./a.txt f",
            "1 ./broken l@",
            "1 ./by_custom f",
            "1 ./filelink l@",
            "1 ./keep.log f",
            "1 ./lib.py f",
            "1 ./link l@",
            "1 ./main.rs f",
            "1 ./other d",
            "2 ./other/build f",
            "1 ./sub d",
            "2 ./sub/anchored.txt f",
            "2 ./sub/b.log f",
            "2 ./sub/deep d",
            "3 ./sub/deep/deeper d",
            "4 ./sub/deep/deeper/z.rs f",
            "3 ./sub/deep/loop l@",
            "3 ./sub/deep/x.tmp f",
            "3 ./sub/deep/y.txt f",
        ];
        assert_eq!(got, want);
    }
    let _ = std::fs::remove_dir_all(&t);
}

#[test]
fn ignore_file_errors_are_reported_with_display_paths() {
    let t = tempdir("err");
    write(&t, ".git/HEAD", "x\n");
    write(&t, ".gitignore", "ok\n[z-a]\n");
    write(&t, "sub/.ignore", "{a\n");
    write(&t, "sub/f", "x\n");
    let got = parity(&t, &opts(), &["."]);
    assert!(
        got.iter()
            .any(|l| l.starts_with("0 . d !./.gitignore: line 2: error parsing glob")),
        "{got:?}"
    );
    assert!(
        got.iter()
            .any(|l| l.starts_with("1 ./sub d !./sub/.ignore: line 1:")),
        "{got:?}"
    );
    let _ = std::fs::remove_dir_all(&t);
}

#[test]
#[cfg(unix)]
fn unreadable_directories_and_loops() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempdir("perm");
    write(&t, "open/f", "x\n");
    write(&t, "shut/f", "x\n");
    std::fs::set_permissions(t.join("shut"), std::fs::Permissions::from_mode(0o000)).unwrap();
    std::os::unix::fs::symlink(".", t.join("open/self")).unwrap();
    let readable = std::fs::read_dir(t.join("shut")).is_err();
    let follow = WalkOptions {
        follow_links: true,
        ..opts()
    };
    let got = parity(&t, &follow, &["."]);
    if readable {
        assert!(
            got.iter().any(|l| l
                .starts_with("error: ./shut: IO error for operation on ./shut: Permission denied")),
            "{got:?}"
        );
    }
    assert!(
        got.iter().any(|l| l.starts_with(
            "error: File system loop found: ./open/self points to an ancestor ./open"
        )),
        "{got:?}"
    );
    std::fs::set_permissions(t.join("shut"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let _ = std::fs::remove_dir_all(&t);
}

#[test]
fn skip_and_quit() {
    let t = fixture();
    let repo = t.join("repo");
    let (native, virt) = hosts(&repo);
    for host in [&native, &virt] {
        let w = Walker::new(host, opts()).unwrap();
        let mut seen = Vec::new();
        w.run(&[b".".to_vec()], &mut |r| {
            let e = r.unwrap();
            seen.push(String::from_utf8_lossy(&e.path).into_owned());
            if e.path == b"./sub" {
                WalkState::Skip
            } else {
                WalkState::Continue
            }
        });
        assert!(seen.contains(&"./sub".to_string()));
        assert!(!seen.iter().any(|p| p.starts_with("./sub/")), "{seen:?}");
        let mut n = 0;
        let state = w.run(&[b".".to_vec(), b"sub".to_vec()], &mut |_| {
            n += 1;
            if n == 3 {
                WalkState::Quit
            } else {
                WalkState::Continue
            }
        });
        assert_eq!((n, state), (3, WalkState::Quit));
    }
    let _ = std::fs::remove_dir_all(&t);
}

#[test]
fn virtual_global_gitignore_and_explicit_files_follow_the_shell() {
    let t = tempdir("global");
    write(
        &t,
        "home/.gitconfig",
        "[core]\n\texcludesFile = ~/my-ignore\n",
    );
    write(&t, "home/my-ignore", "*.global\n");
    write(&t, "xdg/git/ignore", "*.xdg\n");
    write(&t, "proj/.git/HEAD", "x\n");
    for f in ["a.global", "b.xdg", "c.txt", "d.explicit"] {
        write(&t, &format!("proj/{f}"), "x\n");
    }
    write(&t, "explicit-ignore", "*.explicit\n");
    let home = os_bytes(t.join("home").as_os_str());
    let xdg = os_bytes(t.join("xdg").as_os_str());
    let proj = os_bytes(t.join("proj").as_os_str());
    let names = |env: Vec<(Vec<u8>, Vec<u8>)>, files: Vec<Vec<u8>>| {
        let host = FsHost {
            cwd: proj.clone(),
            env,
            native: true,
        };
        let o = WalkOptions {
            ignore_files: files,
            ..WalkOptions::default()
        };
        let (native, got) = walk(&host, &o, &["."]);
        // The process's HOME is not the shell's: the walk is virtual.
        assert!(!native);
        got
    };
    let none = (
        b"GIT_CONFIG_SYSTEM".to_vec(),
        b"/nonexistent/gitconfig".to_vec(),
    );
    let got = names(vec![(b"HOME".to_vec(), home.clone()), none.clone()], vec![]);
    assert_eq!(
        got,
        ["0 . d", "1 ./b.xdg f", "1 ./c.txt f", "1 ./d.explicit f"]
    );
    // Without core.excludesFile: $XDG_CONFIG_HOME/git/ignore.
    let got = names(
        vec![
            (b"HOME".to_vec(), xdg.clone()),
            (b"XDG_CONFIG_HOME".to_vec(), xdg.clone()),
            none,
        ],
        vec![b"../explicit-ignore".to_vec()],
    );
    assert_eq!(got, ["0 . d", "1 ./a.global f", "1 ./c.txt f"]);
    let _ = std::fs::remove_dir_all(&t);
}
