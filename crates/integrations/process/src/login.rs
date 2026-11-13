use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    os::unix::{ffi::OsStringExt, fs::MetadataExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, LazyLock},
};

use nanocode_sync::{Atom, SingleFlight};

const UNDIFFED: [&str; 4] = ["PWD", "OLDPWD", "SHLVL", "_"];

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
    shell: &Path,
    workdir: &Path,
    environment: &[(OsString, OsString)],
    secrets: &[(OsString, OsString)],
) -> Result<Vec<(OsString, OsString)>, String> {
    let current = SNAPSHOT.get().as_ref().clone().filter(|snapshot| {
        snapshot.shell == shell && fingerprint(&snapshot.files) == snapshot.fingerprint
    });
    let snapshot = match current {
        Some(snapshot) => snapshot,
        None => {
            let (shell, workdir, launched) =
                (shell.to_owned(), workdir.to_owned(), environment.to_vec());
            PROBE
                .run(|| async move {
                    nanocode_platform::spawn_task(async move {
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

#[derive(Clone, PartialEq, Eq)]
struct Fingerprint(Vec<Option<[i64; 7]>>);

fn fingerprint(files: &[PathBuf]) -> Fingerprint {
    Fingerprint(
        files
            .iter()
            .map(|file| {
                std::fs::metadata(file).ok().map(|metadata| {
                    [
                        metadata.dev() as i64,
                        metadata.ino() as i64,
                        metadata.size() as i64,
                        metadata.mtime(),
                        metadata.mtime_nsec(),
                        metadata.ctime(),
                        metadata.ctime_nsec(),
                    ]
                })
            })
            .collect(),
    )
}

fn startup_files(shell: &Path, environment: &HashMap<OsString, OsString>) -> Vec<PathBuf> {
    let variable = |name: &str| environment.get(OsStr::new(name)).map(PathBuf::from);
    let home = variable("HOME");
    let mut files = match shell.file_name().and_then(OsStr::to_str) {
        Some("zsh") => {
            let dir = variable("ZDOTDIR").or(home);
            let mut files = vec![PathBuf::from("/etc/zshenv")];
            files.extend(dir.as_ref().map(|dir| dir.join(".zshenv")));
            files.extend(["/etc/zprofile", "/etc/zlogin"].map(PathBuf::from));
            if let Some(dir) = dir {
                files.extend([".zprofile", ".zlogin"].map(|file| dir.join(file)));
            }
            files
        }
        Some("bash") => {
            let mut files: Vec<PathBuf> = variable("BASH_ENV").into_iter().collect();
            files.extend(
                [
                    "/etc/profile",
                    "/etc/profile.d",
                    "/etc/bash.bashrc",
                    "/etc/bashrc",
                ]
                .map(PathBuf::from),
            );
            if let Some(home) = home {
                files.extend(
                    [".bash_profile", ".bash_login", ".profile", ".bashrc"]
                        .map(|file| home.join(file)),
                );
            }
            files
        }
        _ => {
            return home
                .map(|home| home.join(".profile"))
                .into_iter()
                .chain([PathBuf::from("/etc/profile")])
                .collect();
        }
    };
    if cfg!(target_os = "macos") {
        files.extend(["/etc/paths", "/etc/paths.d"].map(PathBuf::from));
    }
    files
}

async fn probe(
    shell: &Path,
    workdir: &Path,
    launched: Vec<(OsString, OsString)>,
) -> Result<Snapshot, String> {
    let script = format!("printf '\\0%s\\0' {MARK}; /usr/bin/env -0");
    let failed = |why: String| format!("the login shell {} {why}", shell.display());
    let output = tokio::process::Command::new(shell)
        .args(["-lc", &script])
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
        shell: shell.to_owned(),
        origin: launched.into_iter().collect(),
        child,
        fingerprint: fingerprint(&files),
        files,
    })
}
