//! jq's and jaq's own test suites against psh's jq (`tests/utils/jq/upstream/`): each
//! case is a program, an input and the expected outputs, compared as values the way
//! jq's `--run-tests` compares them. Cases psh does not pass are listed with a reason
//! in `expected-failures.txt` (a ratchet: a listed case that passes fails the test
//! too). `PSH_JQ_UPDATE=1` rewrites the list, keeping the reasons of cases still listed.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

use jaq_core::{Ctx, Vars};

use super::funs::{Data, Env, Inputs, Kind};
use super::json;
use super::map::Map;
use super::val::Val;

struct Case {
    id: String,
    program: String,
    input: String,
    expected: Vec<String>,
    /// `%%FAIL`: the program must not compile; `Some(msg)` unless `IGNORE MSG`.
    fail: Option<Option<String>>,
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/utils/jq/upstream")
}

fn parse_file(name: &str, text: &str) -> Vec<Case> {
    let lines: Vec<&str> = text.lines().collect();
    let mut cases = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if l.trim().is_empty() || l.starts_with('#') {
            i += 1;
            continue;
        }
        let mut fail = None;
        if l.starts_with("%%FAIL") {
            fail = Some(None);
            if !l.contains("IGNORE MSG") {
                fail = Some(Some(String::new()));
            }
            i += 1;
        }
        let Some(program) = lines.get(i) else { break };
        let id = format!("{name}:{}", i + 1);
        i += 1;
        let mut input = String::new();
        if fail.is_none() {
            input = lines.get(i).copied().unwrap_or("").to_string();
            i += 1;
        }
        let mut expected = Vec::new();
        while i < lines.len() && !lines[i].trim().is_empty() {
            // Comments among the expected outputs are notes (`# Runtime error: ...`).
            if !lines[i].starts_with('#') {
                expected.push(lines[i].to_string());
            }
            i += 1;
        }
        if let Some(Some(msg)) = &mut fail {
            *msg = expected.join("\n");
            expected.clear();
        }
        cases.push(Case {
            id,
            program: program.to_string(),
            input,
            expected,
            fail,
        });
    }
    cases
}

struct NoInputs;

impl Inputs for NoInputs {
    fn next_input(&mut self) -> Option<Result<Val, String>> {
        None
    }
    fn filename(&self) -> Val {
        Val::Null
    }
    fn line(&self) -> usize {
        0
    }
}

/// `None` when the case passes, else what went wrong.
fn check(c: &Case) -> Option<String> {
    let names = vec!["$ENV".to_string()];
    let compiled = super::compile(&c.program, &names);
    if let Some(msg) = &c.fail {
        return match compiled {
            Ok(_) => Some("compiled, expected a compile error".into()),
            Err(report) => match msg {
                Some(m) if !report.contains(m.trim_start_matches("jq: error: ")) => Some(format!(
                    "compile error text differs: {}",
                    report.lines().next().unwrap_or("")
                )),
                _ => None,
            },
        };
    }
    let filter = match compiled {
        Ok(f) => f,
        Err(report) => {
            return Some(format!(
                "does not compile: {}",
                report.lines().next().unwrap_or("")
            ));
        }
    };
    let input = match json::parse_all(c.input.as_bytes()) {
        Ok(v) if v.len() == 1 => v.into_iter().next().unwrap_or_default(),
        _ => return Some("bad input".into()),
    };
    // jq's test scripts set PAGER=less for the manual's `env.PAGER` examples.
    let mut env_map: Map = std::env::vars()
        .map(|(k, v)| (k.into(), Val::str(v)))
        .collect();
    env_map.insert("PAGER".into(), Val::str("less"));
    let env = Env {
        env: Val::obj(env_map),
        stderr: RefCell::new(Vec::new()),
        utc_offset: 0,
        zone: "UTC".into(),
        now: 0.0,
        builtins: super::funs::builtin_names(super::DEFS)
            .into_iter()
            .map(Val::from)
            .collect(),
    };
    let inputs = RefCell::new(NoInputs);
    let data = Data {
        lut: &filter.lut,
        env: &env,
        inputs: &inputs,
    };
    let ctx = Ctx::<Kind>::new(&data, Vars::new([env.env.clone()]));
    let mut outputs = filter.id.run((ctx, input));
    for e in &c.expected {
        let want = match json::parse_all(e.as_bytes()) {
            Ok(v) if v.len() == 1 => v.into_iter().next().unwrap_or_default(),
            _ => return Some(format!("bad expected value {e}")),
        };
        match outputs.next() {
            None => return Some(format!("insufficient results, wanted {e}")),
            Some(Err(err)) => {
                let msg = err
                    .get_err()
                    .map(|e| e.into_val().to_json())
                    .unwrap_or_else(|_| "halt".into());
                return Some(format!("error {msg}, wanted {e}"));
            }
            Some(Ok(v)) => {
                if v != want {
                    return Some(format!("got {}, wanted {e}", v.to_json()));
                }
            }
        }
    }
    if let Some(Ok(extra)) = outputs.next() {
        return Some(format!("superfluous result {}", extra.to_json()));
    }
    None
}

#[test]
fn upstream_suites() {
    let root = dir();
    let mut files: Vec<PathBuf> = Vec::new();
    for sub in std::fs::read_dir(&root).expect("tests/utils/jq/upstream") {
        let sub = sub.expect("an entry").path();
        if sub.is_dir() {
            for f in std::fs::read_dir(&sub).expect("a suite directory") {
                let f = f.expect("an entry").path();
                if f.extension().is_some_and(|e| e == "test") {
                    files.push(f);
                }
            }
        }
    }
    files.sort();
    let ratchet_path = root.join("expected-failures.txt");
    let ratchet_text = std::fs::read_to_string(&ratchet_path).unwrap_or_default();
    let mut ratchet: BTreeMap<String, String> = BTreeMap::new();
    let mut header = String::new();
    for l in ratchet_text.lines() {
        if l.starts_with('#') || l.trim().is_empty() {
            header.push_str(l);
            header.push('\n');
            continue;
        }
        let (id, reason) = l.split_once(": ").unwrap_or((l, ""));
        ratchet.insert(id.to_string(), reason.to_string());
    }
    let mut failing: BTreeMap<String, String> = BTreeMap::new();
    let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for f in &files {
        let suite = f
            .parent()
            .and_then(|p| p.file_name())
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let name = format!(
            "{suite}/{}",
            f.file_name().unwrap_or_default().to_string_lossy()
        );
        let text = std::fs::read_to_string(f).expect("a test file");
        for c in parse_file(&name, &text) {
            let r =
                std::panic::catch_unwind(|| check(&c)).unwrap_or_else(|_| Some("panicked".into()));
            let e = counts.entry(name.clone()).or_default();
            e.1 += 1;
            match r {
                None => e.0 += 1,
                Some(why) => {
                    failing.insert(c.id.clone(), why);
                }
            }
        }
    }
    for (name, (pass, total)) in &counts {
        eprintln!("{name}: {pass}/{total}");
    }
    if std::env::var_os("PSH_JQ_UPDATE").is_some() {
        let mut out = header.clone();
        for (id, why) in &failing {
            let reason = ratchet
                .get(id)
                .filter(|r| !r.is_empty())
                .cloned()
                .unwrap_or_else(|| why.clone());
            out.push_str(&format!("{id}: {reason}\n"));
        }
        std::fs::write(&ratchet_path, out).expect("write the ratchet");
        return;
    }
    let new: Vec<_> = failing
        .iter()
        .filter(|(id, _)| !ratchet.contains_key(*id))
        .collect();
    let fixed: Vec<_> = ratchet
        .keys()
        .filter(|id| !failing.contains_key(*id))
        .collect();
    let mut msg = String::new();
    for (id, why) in &new {
        msg.push_str(&format!("fails, not in the ratchet: {id}: {why}\n"));
    }
    for id in &fixed {
        msg.push_str(&format!("passes now, remove from the ratchet: {id}\n"));
    }
    assert!(
        msg.is_empty(),
        "{msg}run with PSH_JQ_UPDATE=1 to rewrite {}",
        ratchet_path.display()
    );
}
