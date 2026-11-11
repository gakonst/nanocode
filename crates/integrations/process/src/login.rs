use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, LazyLock},
};

use nanocode_sync::{Atom, SingleFlight};

use super::{
    selection::Shell,
    server::{Fingerprint, UNDIFFED, fingerprint},
};

const MARK: &str = "nanocode-login-environment";

struct Snapshot {
    shell: PathBuf,
    origin: HashMap<OsString, OsString>,
    child: Vec<(OsString, OsString)>,
    files: Vec<PathBuf>,
    fingerprint: Fingerprint,
}

static SNAPSHOT: LazyLock<Atom<Option<Arc<Snapshot>>>> = LazyLock::new(Atom::default);
static PROBE: LazyLock<SingleFlight<Result<Arc<Snapshot>, String>>> =
    LazyLock::new(SingleFlight::default);

pub(crate) async fn environment(
    shell: &Shell,
    workdir: &Path,
    environment: &[(OsString, OsString)],
    secrets: &[(OsString, OsString)],
) -> Result<Vec<(OsString, OsString)>, String> {
    let current = SNAPSHOT.get().as_ref().clone().filter(|snapshot| {
        snapshot.shell == shell.path() && fingerprint(&snapshot.files) == snapshot.fingerprint
    });
    let snapshot = match current {
        Some(snapshot) => snapshot,
        None => {
            let (shell, workdir, launched) =
                (shell.clone(), workdir.to_owned(), environment.to_vec());
            PROBE
                .run(|| async move {
                    tokio::spawn(async move {
                        let snapshot = Arc::new(probe(&shell, &workdir, launched).await?);
                        SNAPSHOT.set(Some(Arc::clone(&snapshot)));
                        Ok(snapshot)
                    })
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()))
                })
                .await?
        }
    };
    Ok(snapshot.apply(environment, secrets))
}

fn undiffed(name: &OsStr) -> bool {
    UNDIFFED.iter().any(|undiffed| name == *undiffed)
}

impl Snapshot {
    fn apply(
        &self,
        environment: &[(OsString, OsString)],
        secrets: &[(OsString, OsString)],
    ) -> Vec<(OsString, OsString)> {
        let mut applied: Vec<(OsString, OsString)> = self
            .child
            .iter()
            .filter(|(name, _)| !undiffed(name) && !self.dropped(name, environment))
            .cloned()
            .collect();
        for (name, value) in environment {
            let secret = secrets.iter().any(|(secret, _)| secret == name);
            if !secret && !undiffed(name) && self.origin.get(name) == Some(value) {
                continue;
            }
            applied.retain(|(candidate, _)| candidate != name);
            applied.push((name.clone(), value.clone()));
        }
        applied
    }

    fn dropped(&self, name: &OsStr, environment: &[(OsString, OsString)]) -> bool {
        self.origin.contains_key(name) && !environment.iter().any(|(listed, _)| listed == name)
    }
}

fn startup_files(shell: &Shell, environment: &HashMap<OsString, OsString>) -> Vec<PathBuf> {
    match shell.name() {
        "zsh" => super::server::startup_files(true, environment, None),
        "bash" => {
            let listed: Vec<_> = environment
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
            super::warm::startup_files(true, &listed)
        }
        _ => environment
            .get(OsStr::new("HOME"))
            .map(|home| Path::new(home).join(".profile"))
            .into_iter()
            .chain([PathBuf::from("/etc/profile")])
            .collect(),
    }
}

async fn probe(
    shell: &Shell,
    workdir: &Path,
    launched: Vec<(OsString, OsString)>,
) -> Result<Snapshot, String> {
    let script = format!("printf '\\0%s\\0' {MARK}; /usr/bin/env -0");
    let failed = |why: String| format!("the login shell {} {why}", shell.path().display());
    let output = tokio::process::Command::new(shell.path())
        .args(shell.args(&script, true))
        .current_dir(workdir)
        .env_clear()
        .envs(launched.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .process_group(0)
        .output()
        .await
        .map_err(|error| failed(format!("did not start: {error}")))?;
    if !output.status.success() {
        return Err(failed(format!(
            "ended with {} instead of reporting its environment",
            output.status
        )));
    }
    let mark = [b"\0", MARK.as_bytes(), b"\0"].concat();
    let reported = output
        .stdout
        .windows(mark.len())
        .rposition(|window| window == mark)
        .map(|at| &output.stdout[at + mark.len()..])
        .ok_or_else(|| failed("did not report its environment".to_owned()))?;
    let child: Vec<(OsString, OsString)> = reported
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let equals = entry.iter().position(|byte| *byte == b'=')?;
            Some((
                OsString::from_vec(entry[..equals].to_vec()),
                OsString::from_vec(entry[equals + 1..].to_vec()),
            ))
        })
        .collect();
    let reported: HashMap<OsString, OsString> = child.iter().cloned().collect();
    let files = startup_files(shell, &reported);
    Ok(Snapshot {
        shell: shell.path().to_owned(),
        origin: launched.into_iter().collect(),
        child,
        fingerprint: fingerprint(&files),
        files,
    })
}
