use super::desktop::{Listed, base, interpreter, names, script};
use std::{
    cmp::Reverse,
    fs,
    io::Read as _,
    path::{Path, PathBuf},
};

const MAX_PROC_BYTES: u64 = 2 * 1024 * 1024;

pub struct Process {
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub environ: Vec<u8>,
}

fn read_proc(pid: u32, name: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let read = fs::File::open(format!("/proc/{pid}/{name}"))
        .and_then(|file| file.take(MAX_PROC_BYTES).read_to_end(&mut bytes));
    if read.is_ok() { bytes } else { Vec::new() }
}

fn strings(bytes: &[u8]) -> impl Iterator<Item = String> + '_ {
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    (!bytes.is_empty())
        .then_some(bytes)
        .into_iter()
        .flat_map(|bytes| bytes.split(|&byte| byte == 0))
        .map(|string| String::from_utf8_lossy(string).into_owned())
}

impl Process {
    pub fn read(pid: u32, exe: &str) -> Self {
        Self {
            exe: PathBuf::from(exe),
            args: strings(&read_proc(pid, "cmdline")).collect(),
            cwd: fs::read_link(format!("/proc/{pid}/cwd")).ok(),
            environ: read_proc(pid, "environ"),
        }
    }

    pub fn script(&self) -> Option<(usize, &str)> {
        let program = base(self.exe.to_str()?);
        let (at, script) = script(self.args.get(1..)?).filter(|_| interpreter(program))?;
        Some((at + 1, script))
    }

    fn path(&self, word: &str) -> Option<PathBuf> {
        if !word.contains('/') {
            return None;
        }
        let path = match &self.cwd {
            Some(cwd) => cwd.join(word),
            None => PathBuf::from(word),
        };
        fs::canonicalize(path).ok()
    }

    fn same(&self, wanted: &str, word: &str) -> bool {
        wanted == word
            || Path::new(wanted).is_absolute()
                && fs::canonicalize(wanted).is_ok_and(|wanted| self.path(word) == Some(wanted))
    }

    pub fn launched_from(&self, pid: u32) -> Option<String> {
        let (mut file, mut owner, mut hint) = (None, None, None);
        for var in strings(&self.environ) {
            let Some((key, value)) = var.split_once('=') else {
                continue;
            };
            match key {
                "GIO_LAUNCHED_DESKTOP_FILE" => file = Some(value.to_owned()),
                "GIO_LAUNCHED_DESKTOP_FILE_PID" => owner = Some(value.to_owned()),
                "BAMF_DESKTOP_FILE_HINT" => hint = Some(value.to_owned()),
                _ => {}
            }
        }
        let own = pid.to_string();
        let file = file
            .filter(|_| owner.is_none_or(|owner| owner.trim() == own))
            .or(hint)?;
        base(&file)
            .strip_suffix(".desktop")
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    }
}

pub fn by_class(entries: &[Listed], classes: &[String]) -> Option<String> {
    classes.iter().find_map(|class| {
        entries
            .iter()
            .find(|listed| {
                listed
                    .entry
                    .wm_class
                    .as_deref()
                    .is_some_and(|wm_class| wm_class.eq_ignore_ascii_case(class))
                    || listed.id.eq_ignore_ascii_case(class)
            })
            .map(|listed| listed.id.clone())
    })
}

pub fn by_exec(entries: &[Listed], process: &Process) -> Option<String> {
    let script = process.script();
    let script_path = script.and_then(|(at, word)| Some((at, process.path(word)?)));
    let interpreted = process
        .exe
        .to_str()
        .is_some_and(|exe| interpreter(base(exe)));
    entries
        .iter()
        .filter_map(|listed| {
            let program = listed.program.as_ref()?;
            let words = &listed.entry.exec[1..];
            let rest = if *program == process.exe {
                if interpreted && !words.iter().any(|word| !word.starts_with('-')) {
                    return None;
                }
                process.args.get(1..)?
            } else if let Some((at, path)) = &script_path
                && path == program
            {
                process.args.get(at + 1..)?
            } else {
                return None;
            };
            (rest.len() >= words.len()
                && words
                    .iter()
                    .zip(rest)
                    .all(|(wanted, word)| process.same(wanted, word)))
            .then_some((words.len(), &listed.id))
        })
        .min_by_key(|(words, _)| Reverse(*words))
        .map(|(_, id)| id.clone())
}

pub fn of(pid: u32, exe: &str, classes: &[String], entries: &[Listed]) -> Vec<String> {
    let process = (!exe.is_empty()).then(|| Process::read(pid, exe));
    let desktop = process
        .as_ref()
        .and_then(|process| process.launched_from(pid))
        .or_else(|| by_class(entries, classes))
        .or_else(|| by_exec(entries, process.as_ref()?));
    let script = process
        .as_ref()
        .and_then(Process::script)
        .map(|(_, script)| base(script).to_owned());
    names([
        desktop,
        script,
        (!exe.is_empty()).then(|| base(exe).to_owned()),
    ])
}
