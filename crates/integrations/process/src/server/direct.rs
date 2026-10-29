use std::{
    collections::{HashMap, HashSet},
    ffi::{OsStr, OsString},
    io,
    os::unix::{ffi::OsStringExt, fs::MetadataExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use nanocode_sync::Signal;
use tokio::{
    net::unix::pipe,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use super::{Fingerprint, Job, Note, Output, UNDIFFED, fifo};
use crate::process;

const ATTRIBUTES: &str = "umask; ulimit -a; ulimit -H -a";

#[derive(Default)]
pub(super) struct Shape {
    pub(super) readonly: HashSet<Vec<u8>>,
    pub(super) commands: HashSet<Vec<u8>>,
    pub(super) builtins: HashSet<Vec<u8>>,
    pub(super) global: HashSet<Vec<u8>>,
    pub(super) suffixes: HashSet<Vec<u8>>,
}

pub(super) struct Origin {
    pub(super) workdir: PathBuf,
    pub(super) environment: HashMap<OsString, OsString>,
    pub(super) listed: Vec<(OsString, OsString)>,
}

pub(super) struct Direct {
    origin: Arc<Origin>,
    shape: Arc<Shape>,
    child: Vec<(OsString, OsString)>,
}

pub(crate) struct Fifos {
    pipes: [Output; 2],
    ended: Arc<Signal>,
}

impl Fifos {
    pub(crate) fn drains(
        self,
        chunks: &mpsc::UnboundedSender<Vec<u8>>,
        activity: &Arc<nanocode_process_contracts::LastActivity>,
    ) -> Vec<JoinHandle<()>> {
        self.pipes
            .into_iter()
            .map(|pipe| {
                let (ended, chunks, activity) = (
                    Arc::clone(&self.ended),
                    chunks.clone(),
                    Arc::clone(activity),
                );
                tokio::spawn(async move {
                    super::read_all(pipe, &ended, &chunks, &activity).await;
                })
            })
            .collect()
    }
}

pub(super) struct Admission(Arc<AtomicUsize>);

impl Admission {
    pub(super) fn new(running: &Arc<AtomicUsize>) -> Option<Self> {
        let limit = std::thread::available_parallelism().map_or(1, usize::from) * super::DEPTH;
        running
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < limit).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(Arc::clone(running)))
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) fn words(script: &str) -> Option<Vec<String>> {
    let plain = |word: &&str| {
        !word.starts_with('=')
            && word
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-./,:+=%".contains(&byte))
    };
    let words: Vec<&str> = script
        .split([' ', '\t'])
        .filter(|word| !word.is_empty())
        .collect();
    let command = words.first()?;
    (words.iter().all(plain) && !command.contains('=') && !command.starts_with(['-', '%']))
        .then(|| words.iter().map(|word| (*word).to_owned()).collect())
}

impl Direct {
    fn allows(&self, words: &[String]) -> bool {
        let command = words[0].as_bytes();
        !self.shape.commands.contains(command)
            && words[0]
                .rsplit_once('.')
                .is_none_or(|(_, suffix)| !self.shape.suffixes.contains(suffix.as_bytes()))
            && !words
                .iter()
                .any(|word| self.shape.global.contains(word.as_bytes()))
            && (words[0].contains('/') || !self.shape.builtins.contains(command))
    }

    fn environment(
        &self,
        environment: &[(OsString, OsString)],
        secrets: &[(OsString, OsString)],
    ) -> Option<HashMap<OsString, OsString>> {
        let undiffed = |name: &OsStr| UNDIFFED.iter().any(|undiffed| name == *undiffed);
        let settable = |name: &OsStr| {
            super::is_shell_identifier(name.as_encoded_bytes())
                && !self.shape.readonly.contains(name.as_encoded_bytes())
        };
        let mut child: HashMap<OsString, OsString> = self.child.iter().cloned().collect();
        let same = environment == self.origin.listed.as_slice();
        for (name, value) in environment {
            let secret = secrets.iter().any(|(secret, _)| secret == name);
            if undiffed(name)
                || (!secret && (same || self.origin.environment.get(name) == Some(value)))
            {
                continue;
            }
            if !settable(name) {
                return None;
            }
            child.insert(name.clone(), value.clone());
        }
        if !same {
            let present: HashSet<&OsString> = environment.iter().map(|(name, _)| name).collect();
            for name in self.origin.environment.keys() {
                if undiffed(name) || present.contains(name) {
                    continue;
                }
                if !settable(name) {
                    return None;
                }
                child.remove(name);
            }
        }
        Some(child)
    }
}

fn working_directory(environment: &[(OsString, OsString)], workdir: &Path) -> Option<PathBuf> {
    let logical = environment
        .iter()
        .find(|(name, _)| name == "PWD")
        .map(|(_, value)| PathBuf::from(value))
        .filter(|value| value.is_absolute());
    if let Some(logical) = logical
        && let (Ok(left), Ok(right)) = (std::fs::metadata(&logical), std::fs::metadata(workdir))
        && left.dev() == right.dev()
        && left.ino() == right.ino()
    {
        return Some(logical);
    }
    std::fs::canonicalize(workdir).ok()
}

fn executable(path: &Path) -> bool {
    nix::unistd::access(path, nix::unistd::AccessFlags::X_OK).is_ok()
}

fn resolve(command: &str, path: Option<&OsString>, directory: &Path) -> Option<PathBuf> {
    if command.contains('/') {
        let program = directory.join(command);
        let metadata = std::fs::metadata(&program).ok()?;
        return (!metadata.is_dir() && executable(&program)).then_some(program);
    }
    std::env::split_paths(path?).find_map(|entry| {
        let program = directory.join(entry.join(command));
        (std::fs::metadata(&program).is_ok_and(|metadata| metadata.is_file())
            && executable(&program))
        .then_some(program)
    })
}

fn fifos(root: &Path) -> io::Result<([Output; 2], [std::fs::File; 2])> {
    let name = format!("d{:016x}", crate::random());
    let open = |stream: u8| {
        let path = root.join(format!("{name}-{stream}"));
        fifo(&path)?;
        let opened = pipe::OpenOptions::new()
            .open_receiver(&path)
            .and_then(|receiver| {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .map(|sender| (receiver, sender))
            });
        let _ = std::fs::remove_file(&path);
        opened
    };
    let (stdout, out) = open(1)?;
    let (stderr, err) = open(2)?;
    Ok(([stdout, stderr], [out, err]))
}

struct Launch<'a> {
    program: PathBuf,
    arguments: &'a [String],
    arg0: &'a str,
    directory: PathBuf,
    environment: HashMap<OsString, OsString>,
}

fn launch(launch: Launch<'_>, root: &Path) -> io::Result<(tokio::process::Child, [Output; 2])> {
    let (pipes, [stdout, stderr]) = fifos(root)?;
    let mut environment = launch.environment;
    environment.insert("PWD".into(), launch.directory.clone().into_os_string());
    environment.insert("OLDPWD".into(), launch.directory.clone().into_os_string());
    environment.insert("_".into(), launch.program.clone().into_os_string());
    let mut command = tokio::process::Command::new(&launch.program);
    command
        .arg0(launch.arg0)
        .args(launch.arguments)
        .current_dir(&launch.directory)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .kill_on_drop(true)
        .process_group(0);
    let child = command.spawn()?;
    drop(command);
    Ok((child, pipes))
}

pub(super) async fn spawn(
    admission: Admission,
    direct: Arc<Direct>,
    words: Vec<String>,
    workdir: PathBuf,
    environment: Vec<(OsString, OsString)>,
    secrets: Vec<(OsString, OsString)>,
    root: PathBuf,
) -> Option<process::SpawnedProcess> {
    if !direct.allows(&words) {
        return None;
    }
    let spawned = tokio::task::spawn_blocking(move || {
        let directory = working_directory(&environment, &workdir)?;
        let child_environment = direct.environment(&environment, &secrets)?;
        let program = resolve(
            &words[0],
            child_environment.get(OsStr::new("PATH")),
            &directory,
        )?;
        let launched = launch(
            Launch {
                program,
                arguments: &words[1..],
                arg0: &words[0],
                directory,
                environment: child_environment,
            },
            &root,
        );
        launched
            .inspect_err(|error| tracing::debug!(%error, "a direct command did not start"))
            .ok()
    })
    .await
    .ok()??;
    Some(spawned_process(spawned, Some(admission)))
}

fn spawned_process(
    (mut child, pipes): (tokio::process::Child, [Output; 2]),
    admission: Option<Admission>,
) -> process::SpawnedProcess {
    let pid = child.id().unwrap_or_default();
    let ended = Arc::new(Signal::new());
    let exited = Arc::clone(&ended);
    let child = process::ProcessChild::new(
        move || {
            let _admitted = &admission;
            let status = child
                .try_wait()
                .map(|status| status.map(process::exit_code));
            if matches!(status, Ok(Some(_))) {
                exited.fire();
            }
            status
        },
        pid,
        true,
    );
    process::SpawnedProcess {
        process_group: child.process_group(),
        child,
        stdin: None,
        output: process::ProcessOutput::Fifos(Fifos { pipes, ended }),
    }
}

async fn collect(pipes: [Output; 2], ended: &Signal) -> (Vec<u8>, [Option<Output>; 2]) {
    let activity = nanocode_process_contracts::LastActivity::new();
    let [stdout, stderr] = pipes;
    let (chunks, mut received) = mpsc::unbounded_channel();
    let (ignored, _discarded) = mpsc::unbounded_channel();
    let pipes = tokio::join!(
        super::read_all(stdout, ended, &chunks, &activity),
        super::read_all(stderr, ended, &ignored, &activity)
    );
    drop(chunks);
    let mut output = Vec::new();
    while let Some(chunk) = received.recv().await {
        output.extend_from_slice(&chunk);
    }
    (output, pipes.into())
}

async fn served(
    notes: &mpsc::WeakUnboundedSender<Note>,
    script: &str,
    origin: &Origin,
) -> Option<(i32, Vec<u8>)> {
    let ended = Arc::new(Signal::new());
    let (exit, exited) = oneshot::channel();
    let (reply, accepted) = oneshot::channel();
    notes
        .upgrade()?
        .send(Note::Job(Job {
            script: script.to_owned(),
            workdir: origin.workdir.clone(),
            environment: origin.listed.clone(),
            secrets: Vec::new(),
            exit,
            ended: Arc::clone(&ended),
            reply,
            direct: None,
        }))
        .ok()?;
    let lease = accepted.await.ok()??;
    let pipes = [lease.pipes[0].take()?, lease.pipes[1].take()?];
    let ((output, pipes), status) = tokio::join!(collect(pipes, &ended), exited);
    for (slot, pipe) in lease.pipes.iter().zip(pipes) {
        if let Some(pipe) = pipe {
            slot.put(pipe);
        }
    }
    drop(lease);
    Some((status.ok()?.ok()?, output))
}

async fn discover(
    notes: &mpsc::WeakUnboundedSender<Note>,
    root: &Path,
    origin: Arc<Origin>,
    shape: Arc<Shape>,
) -> Option<Arc<Direct>> {
    let (status, environment) = served(notes, "/usr/bin/env -0", &origin).await?;
    if status != 0 {
        return None;
    }
    let child = environment
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let split = entry.iter().position(|byte| *byte == b'=')?;
            Some((
                OsString::from_vec(entry[..split].to_vec()),
                OsString::from_vec(entry[split + 1..].to_vec()),
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    let direct = Arc::new(Direct {
        origin: Arc::clone(&origin),
        shape,
        child,
    });
    let (status, expected) = served(notes, &format!("/bin/sh -c '{ATTRIBUTES}'"), &origin).await?;
    if status != 0 {
        return None;
    }
    let probe = Arc::clone(&direct);
    let root = root.to_owned();
    let launched = tokio::task::spawn_blocking(move || {
        let directory = working_directory(&probe.origin.listed, &probe.origin.workdir)?;
        let environment = probe.environment(&probe.origin.listed, &[])?;
        launch(
            Launch {
                program: PathBuf::from("/bin/sh"),
                arguments: &["-c".to_owned(), ATTRIBUTES.to_owned()],
                arg0: "/bin/sh",
                directory,
                environment,
            },
            &root,
        )
        .ok()
    })
    .await
    .ok()??;
    let mut spawned = spawned_process(launched, None);
    let process::ProcessOutput::Fifos(fifos) = spawned.output else {
        return None;
    };
    let ((actual, _), status) =
        tokio::join!(collect(fifos.pipes, &fifos.ended), spawned.child.wait());
    (status.ok()? == 0 && actual == expected).then_some(direct)
}

pub(super) async fn probe(
    notes: mpsc::WeakUnboundedSender<Note>,
    root: PathBuf,
    origin: Arc<Origin>,
    shape: Arc<Shape>,
    fingerprint: Fingerprint,
) {
    let direct = discover(&notes, &root, origin, shape).await;
    if direct.is_none() {
        tracing::debug!("shell commands keep running through the shell server");
    }
    if let Some(notes) = notes.upgrade() {
        let _ = notes.send(Note::Direct(fingerprint, direct));
    }
}
