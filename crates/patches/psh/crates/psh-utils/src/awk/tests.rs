//! awk on the engine's test host: unit cases, and the vendored suites
//! (tests/vendor-awk: onetrueawk's t.* and p.* programs, GoAWK's interpreter cases)
//! against gawk's recorded results, held by a ratchet.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use psh_engine::testhost::TestHost;
use psh_engine::{Script, Shell, Step};

struct Out {
    status: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Runs `src` with `files` in /tmp/case (the cwd) and `stdin`; None status: it did not
/// end within the step budget.
fn run(src: &str, stdin: &[u8], files: &[(String, Vec<u8>)]) -> Out {
    let host = Rc::new(TestHost::new());
    host.set_stdin(stdin);
    host.fs.mkdir_all("/tmp/case");
    for (name, data) in files {
        host.fs.write_file(&format!("/tmp/case/{name}"), data);
    }
    let mut cfg = host.config();
    cfg.cwd = b"/tmp/case".to_vec();
    cfg.env.push((b"LC_ALL".to_vec(), b"C.UTF-8".to_vec()));
    cfg.env.push((b"TZ".to_vec(), b"UTC".to_vec()));
    cfg.utilities = crate::policy(false);
    let mut shell = Shell::new(cfg);
    shell.start(Script::command(src));
    let mut status = None;
    for _ in 0..200_000 {
        match shell.step(1000) {
            Step::Done(s) => {
                status = Some(s);
                break;
            }
            Step::Yield => {}
            other => panic!("{other:?} running {src:?}"),
        }
    }
    Out {
        status,
        stdout: host.stdout(),
        stderr: host.stderr(),
    }
}

#[track_caller]
fn check(src: &str, stdin: &str, stdout: &str) {
    let o = run(src, stdin.as_bytes(), &[]);
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        stdout,
        "{src:?} (stderr: {})",
        String::from_utf8_lossy(&o.stderr)
    );
}

#[test]
fn basics() {
    check("awk 'BEGIN { print 1+2, \"x\" }'", "", "3 x\n");
    check("awk '{ print $2; print NF }'", "a b c\n", "b\n3\n");
    check("awk -F: -v x=1 '{ print $3 $1 x }'", "a:b:c\n", "ca1\n");
    check("awk -F '[][]' '{ print $2 }'", "a[b]c\n", "b\n");
    check(
        "awk '{ n[$1]++ } END { print length(n), toupper(\"ab\") }'",
        "x\ny\nx\n",
        "2 AB\n",
    );
    check(
        "awk 'BEGIN { printf \"%*d|%-3s|\\n\", 4, 7, \"a\" }'",
        "",
        "   7|a  |\n",
    );
    check(
        "awk 'BEGIN { \"echo hi\" | getline x; print x; system(\"echo sys\") }'",
        "",
        "hi\nsys\n",
    );
    check(
        "awk 'BEGIN { print \"e\" > \"/dev/stderr\"; print \"o\" }'",
        "",
        "o\n",
    );
    check(
        "awk 'BEGIN { print strftime(\"%Y-%m-%d %H:%M:%S\", 86400 * 365) }'",
        "",
        "1971-01-01 00:00:00\n",
    );
    check("awk '{ print | \"sort\" }'", "b\na\n", "a\nb\n");
    check(
        "printf 'l1\\nl2\\n' > f; awk 'BEGIN { while ((getline l < \"f\") > 0) n++; print n }'",
        "",
        "2\n",
    );
    check(
        "awk '{ $2 = \"X\"; print; print NF }' OFS=-",
        "a b c\n",
        "a-X-c\n3\n",
    );
}

#[test]
fn regex_and_substitution_details() {
    // Global substitution sees the text before each match (\B), gawk's backslashes.
    check(
        r#"awk 'BEGIN { x = "abcd"; gsub(/\B/, ":", x); print x }'"#,
        "",
        "a:b:c:d\n",
    );
    check(
        r#"awk '{ s = $0; sub(/B/, "\\\\&", s); print s }'"#,
        "B\n",
        "\\B\n",
    );
    check(
        r#"awk '{ s = $0; sub(/B/, "\\\\\\\\&", s); print s }'"#,
        "B\n",
        "\\\\B\n",
    );
    check(
        r#"awk '{ s = $0; sub(/B/, "x\\&y", s); print s }'"#,
        "B\n",
        "x&y\n",
    );
    // split() does not split at newlines in paragraph mode; octal escapes in a regex.
    check(
        r#"awk 'BEGIN { RS = ""; FS = ":"; print split("a:b\nc:d", p) }'"#,
        "",
        "3\n",
    );
    check(r#"awk '/a\52b/ { print "m" }'"#, "ab\n", "m\n");
    check(r#"awk '{ print FILENAME }'"#, "x\n", "-\n");
    check(r#"awk '{ print ("\\" ~ /[\[-\]]/) }'"#, "x\n", "1\n");
}

#[test]
fn unsupported_options_and_errors() {
    // No awk program on the test host to fall through to: a clear error.
    let o = run("awk --posix 'BEGIN { }'", b"", &[]);
    assert_eq!(o.status, Some(2));
    assert_eq!(
        String::from_utf8_lossy(&o.stderr),
        "awk: unsupported option '--posix' (psh built-in awk)\n"
    );
    let o = run("awk -f missing.awk", b"", &[]);
    assert_eq!(o.status, Some(2));
    let o = run("awk 'BEGIN {\n  x = 1 / y\n}'", b"", &[]);
    assert_eq!(
        (o.status, String::from_utf8_lossy(&o.stderr).as_ref()),
        (
            Some(2),
            "awk: cmd. line:2: fatal: division by zero attempted\n"
        )
    );
}

// ---- The vendored suites.

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/vendor-awk")
}

fn unesc(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            match b[i + 1] {
                b'\\' => out.push(b'\\'),
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'x' if i + 3 < b.len() => {
                    out.push(u8::from_str_radix(&s[i + 2..i + 4], 16).unwrap());
                    i += 2;
                }
                c => out.push(c),
            }
            i += 2;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

struct Expect {
    status: String,
    stdout: Vec<u8>,
}

fn expectations(file: &str) -> BTreeMap<String, Expect> {
    let text = std::fs::read_to_string(root().join(file)).unwrap();
    text.lines()
        .map(|l| {
            let mut f = l.splitn(3, '\t');
            let id = f.next().unwrap().to_string();
            let status = f.next().unwrap().to_string();
            let stdout = unesc(f.next().unwrap_or(""));
            (id, Expect { status, stdout })
        })
        .collect()
}

/// modes.txt: `id: MODE (reason)` lines. `bsd`: held to the BSD recording (D016 per
/// case); `sorted`: lines compared sorted (`for (k in a)` order); `status`: only the
/// status (rand()).
fn choices() -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(root().join("modes.txt")).unwrap();
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let (id, rest) = l.split_once(':')?;
            Some((id.trim().to_string(), rest.trim().to_string()))
        })
        .collect()
}

type Files = Vec<(String, Vec<u8>)>;

/// The command, stdin and files of a case.
fn case(
    id: &str,
    ota_files: &[(String, Vec<u8>)],
    goawk: &[(Vec<u8>, Vec<u8>)],
) -> (String, Vec<u8>, Files) {
    if let Some(p) = id.strip_prefix("onetrueawk/") {
        let input = if p.starts_with("t.") {
            "test.data"
        } else {
            "test.countries"
        };
        (
            format!("awk -f {p} {input}"),
            Vec::new(),
            ota_files.to_vec(),
        )
    } else {
        let n: usize = id.strip_prefix("goawk/").unwrap().parse().unwrap();
        let (src, stdin) = &goawk[n];
        let src = String::from_utf8_lossy(src).replace('\'', "'\\''");
        (format!("awk -- '{src}' -"), stdin.clone(), Vec::new())
    }
}

#[test]
fn vendored_suites() {
    let gnu = expectations("expected.gnu.txt");
    let bsd = expectations("expected.bsd.txt");
    let choices = choices();
    let dir = root().join("onetrueawk");
    let mut ota_files = Vec::new();
    for e in std::fs::read_dir(&dir).unwrap() {
        let e = e.unwrap();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("t.") || name.starts_with("p.") || name.starts_with("test.") {
            ota_files.push((name, std::fs::read(e.path()).unwrap()));
        }
    }
    let goawk: Vec<(Vec<u8>, Vec<u8>)> = std::fs::read_to_string(root().join("goawk/cases.txt"))
        .unwrap()
        .lines()
        .map(|l| {
            let (s, i) = l.split_once('\t').unwrap_or((l, ""));
            (unesc(s), unesc(i))
        })
        .collect();

    let ids: Vec<String> = gnu.keys().cloned().collect();
    let threads = 4;
    let failing: BTreeMap<String, String> = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let ids = &ids;
                let (gnu, bsd, choices, ota_files, goawk) =
                    (&gnu, &bsd, &choices, &ota_files, &goawk);
                sc.spawn(move || {
                    let mut failing = BTreeMap::new();
                    for id in ids.iter().skip(t).step_by(threads) {
                        let want = match choices.get(id) {
                            Some(c) if c.starts_with("bsd") => &bsd[id],
                            _ => &gnu[id],
                        };
                        let (cmd, stdin, files) = case(id, ota_files, goawk);
                        let o = run(&cmd, &stdin, &files);
                        let status = o.status.map_or("timeout".to_string(), |s| s.to_string());
                        let mode = choices.get(id).map_or("", |m| m.split(' ').next().unwrap());
                        let same = match mode {
                            "sorted" => sorted(&o.stdout) == sorted(&want.stdout),
                            "status" => true,
                            _ => o.stdout == want.stdout,
                        };
                        if status != want.status || !same {
                            let also_bsd = bsd
                                .get(id)
                                .is_some_and(|b| b.status == status && b.stdout == o.stdout);
                            let why = if also_bsd {
                                "matches bsd".to_string()
                            } else {
                                format!("status {status} (want {})", want.status)
                            };
                            if std::env::var_os("PSH_AWK_SHOW").is_some() {
                                eprintln!(
                                    "== {id}: {cmd}\n-- want {}:\n{}\n-- got {status}:\n{}\n-- stderr:\n{}",
                                    want.status,
                                    String::from_utf8_lossy(&want.stdout),
                                    String::from_utf8_lossy(&o.stdout),
                                    String::from_utf8_lossy(&o.stderr)
                                );
                            }
                            failing.insert(id.clone(), why);
                        }
                    }
                    failing
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });

    let ratchet_path = root().join("expected-failures.txt");
    let ratchet_text = std::fs::read_to_string(&ratchet_path).unwrap_or_default();
    let ratchet: BTreeSet<String> = ratchet_text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| l.split(':').next().unwrap().trim().to_string())
        .collect();
    let regressions: Vec<_> = failing.keys().filter(|id| !ratchet.contains(*id)).collect();
    let stale: Vec<_> = ratchet
        .iter()
        .filter(|id| !failing.contains_key(*id))
        .collect();
    let total = ids.len();
    eprintln!(
        "vendor-awk: {}/{total} pass ({} expected failures)",
        total - failing.len(),
        ratchet.len()
    );
    if std::env::var_os("PSH_UPDATE_RATCHET").is_some() {
        let mut out = String::from(
            "# The vendored awk cases psh fails (tests/vendor-awk/README.md); update with\n\
             # PSH_UPDATE_RATCHET=1 cargo test -p psh-utils awk::tests::vendored_suites\n",
        );
        // Sorted by suite, then naturally.
        let mut ids: Vec<_> = failing.iter().collect();
        ids.sort_by_key(|(id, _)| sort_key(id));
        let reasons: BTreeMap<&str, &str> = ratchet_text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.split_once(':'))
            .map(|(id, why)| (id.trim(), why.trim()))
            .collect();
        for (id, why) in ids {
            let why = reasons.get(id.as_str()).copied().unwrap_or(why);
            out.push_str(&format!("{id}: {why}\n"));
        }
        std::fs::write(&ratchet_path, out).unwrap();
        return;
    }
    assert!(
        regressions.is_empty() && stale.is_empty(),
        "vendor-awk: regressions {regressions:?}; now passing (remove from the ratchet with \
         PSH_UPDATE_RATCHET=1): {stale:?}"
    );
}

fn sorted(b: &[u8]) -> Vec<&[u8]> {
    let mut lines: Vec<&[u8]> = b.split(|&c| c == b'\n').collect();
    lines.sort();
    lines
}

fn sort_key(id: &str) -> (String, u64, String) {
    match id.rsplit_once('/') {
        Some((suite, n)) => match n.parse::<u64>() {
            Ok(k) => (suite.to_string(), k, String::new()),
            Err(_) => (suite.to_string(), 0, n.to_string()),
        },
        None => (id.to_string(), 0, String::new()),
    }
}

#[test]
fn a_program_only_gawk_runs_is_the_real_programs() {
    // With gawk on PATH, what this awk cannot compile or does not act on goes to it.
    let run_with_gawk = |src: &str| {
        let host = Rc::new(TestHost::new());
        host.add_command("gawk", |p| {
            Box::pin(async move {
                let _ = p.write(b"the real gawk\n").await;
                7
            })
        });
        let mut cfg = host.config();
        cfg.utilities = crate::policy(false);
        let mut shell = Shell::new(cfg);
        shell.start(Script::command(src));
        let status = loop {
            match shell.step(1000) {
                Step::Done(s) => break s,
                Step::Yield => {}
                other => panic!("{other:?} running {src:?}"),
            }
        };
        (status, String::from_utf8_lossy(&host.stdout()).into_owned())
    };
    for program in [
        "BEGIN { n = asorti(a, b) }",
        "BEGIN { PROCINFO[\"sorted_in\"] = \"@ind_str_asc\" }",
        "BEGINFILE { print FILENAME }",
        "BEGIN { IGNORECASE = 1 }",
        "BEGIN { a[1][2] = 3 }",
    ] {
        assert_eq!(
            run_with_gawk(&format!("gawk '{program}'")),
            (7, "the real gawk\n".into()),
            "{program}"
        );
    }
    assert_eq!(run_with_gawk("gawk -i inplace 1 f"), (7, "the real gawk\n".into()));
    // What it runs itself stays its own, and without the program its own error stands.
    assert_eq!(run_with_gawk("gawk 'BEGIN { print 1 + 1 }'"), (0, "2\n".into()));
    let o = run("awk 'BEGIN { n = asorti(a, b) }'", b"", &[]);
    assert_eq!(o.status, Some(2));
    assert_eq!(
        String::from_utf8_lossy(&o.stderr),
        "awk: cmd. line:1: fatal: function `asorti' not defined\n"
    );
}
