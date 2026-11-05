use crate::App;
use std::{
    collections::HashSet,
    env, fs,
    io::Read as _,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::SystemTime,
};

const MAX_ENTRY_BYTES: u64 = 1024 * 1024;
const MAX_DIRS: usize = 32;
const MAX_ENTRIES: usize = 4096;

pub struct Entry {
    pub name: String,
    pub exec: Vec<String>,
    pub wm_class: Option<String>,
}

pub fn parse(text: &str) -> Option<Entry> {
    let mut group = "";
    let (mut name, mut exec, mut wm_class, mut application, mut hidden) =
        (None, None, None, false, false);
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            group = line;
            continue;
        }
        if group != "[Desktop Entry]" {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Name" => name = Some(value.trim().to_owned()),
            "Exec" => exec = Some(arguments(value.trim())),
            "StartupWMClass" => wm_class = Some(value.trim().to_owned()),
            "Type" => application = value.trim() == "Application",
            "Hidden" => hidden = value.trim() == "true",
            _ => {}
        }
    }
    let exec = exec.filter(|exec| !exec.is_empty())?;
    (application && !hidden).then_some(Entry {
        name: name.filter(|name| !name.is_empty())?,
        exec,
        wm_class: wm_class.filter(|class| !class.is_empty()),
    })
}

pub fn arguments(exec: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let (mut quoted, mut started) = (false, false);
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => word.extend(chars.next()),
            c if c.is_whitespace() && !quoted => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                started = false;
            }
            c => word.push(c),
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    words
        .into_iter()
        .filter_map(|word| {
            if word.len() == 2 && word.starts_with('%') && word != "%%" {
                return None;
            }
            let mut out = String::new();
            let mut chars = word.chars();
            while let Some(c) = chars.next() {
                if c != '%' {
                    out.push(c);
                    continue;
                }
                if chars.next() == Some('%') {
                    out.push('%');
                }
            }
            Some(out)
        })
        .collect()
}

pub fn base(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn which(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let path = PathBuf::from(program);
        return executable(&path).then_some(path);
    }
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|path| executable(path))
}

fn dirs() -> Vec<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let data = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| home.map(|home| home.join(".local/share")));
    let shared = env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    data.into_iter()
        .chain(env::split_paths(&shared))
        .take(MAX_DIRS)
        .map(|dir| dir.join("applications"))
        .collect()
}

fn read(path: &Path) -> Option<String> {
    let meta = path.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_ENTRY_BYTES {
        return None;
    }
    let mut text = String::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_ENTRY_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

pub fn interpreter(program: &str) -> bool {
    let name = program.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    [
        "python", "perl", "ruby", "node", "nodejs", "bash", "sh", "dash", "zsh", "fish", "ksh",
        "java", "gjs", "lua", "luajit", "php", "tclsh", "wish", "mono", "electron",
    ]
    .contains(&name)
}

pub fn script(arguments: &[String]) -> Option<(usize, &str)> {
    arguments
        .iter()
        .enumerate()
        .find(|(_, word)| !word.starts_with('-'))
        .map(|(at, word)| (at, word.as_str()))
}

pub fn names(candidates: impl IntoIterator<Item = Option<String>>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in candidates.into_iter().flatten() {
        if !name.is_empty() && !names.iter().any(|known| known.eq_ignore_ascii_case(&name)) {
            names.push(name);
        }
    }
    names
}

pub struct Listed {
    pub id: String,
    pub path: String,
    pub entry: Entry,
    pub program: Option<PathBuf>,
}

impl Listed {
    fn new(id: &str, path: &Path, entry: Entry) -> Self {
        Self {
            id: id.to_owned(),
            path: path.to_string_lossy().into_owned(),
            program: which(&entry.exec[0]).and_then(|path| fs::canonicalize(path).ok()),
            entry,
        }
    }

    pub fn app(&self) -> App {
        let program = &self.entry.exec[0];
        let resolved = self
            .program
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned());
        let script = (interpreter(base(program)) || resolved.as_deref().is_some_and(interpreter))
            .then(|| script(&self.entry.exec[1..]))
            .flatten()
            .map(|(_, script)| base(script).to_owned());
        let aliases = names([script, Some(base(program).to_owned()), resolved])
            .into_iter()
            .filter(|name| !name.eq_ignore_ascii_case(&self.id))
            .collect();
        App {
            pid: 0,
            name: self.entry.name.clone(),
            id: self.id.clone(),
            aliases,
            path: self.path.clone(),
            background: false,
            windows: false,
        }
    }
}

#[derive(Default)]
pub struct Index {
    scanned: Vec<(PathBuf, Option<SystemTime>)>,
    entries: Vec<Listed>,
}

impl Index {
    pub fn entries(&mut self) -> &[Listed] {
        let dirs: Vec<(PathBuf, Option<SystemTime>)> = dirs()
            .into_iter()
            .map(|dir| {
                let modified = fs::metadata(&dir).and_then(|meta| meta.modified()).ok();
                (dir, modified)
            })
            .collect();
        if dirs != self.scanned {
            self.entries = scan(&dirs);
            self.scanned = dirs;
        }
        &self.entries
    }
}

fn scan(dirs: &[(PathBuf, Option<SystemTime>)]) -> Vec<Listed> {
    let mut seen = HashSet::new();
    let mut listed = Vec::new();
    for (dir, _) in dirs {
        let Ok(files) = fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = files
            .flatten()
            .take(MAX_ENTRIES)
            .map(|file| file.path())
            .collect();
        files.sort();
        for path in files {
            let Some(id) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".desktop"))
            else {
                continue;
            };
            if !seen.insert(id.to_owned()) {
                continue;
            }
            if let Some(entry) = read(&path).and_then(|text| parse(&text)) {
                listed.push(Listed::new(id, &path, entry));
            }
        }
    }
    listed
}

pub fn matches(entry: &Entry, stem: &str, wanted: &str) -> bool {
    entry.name.eq_ignore_ascii_case(wanted)
        || stem.eq_ignore_ascii_case(wanted)
        || entry
            .exec
            .first()
            .is_some_and(|program| base(program).eq_ignore_ascii_case(wanted))
}

pub fn installed(entries: &[Listed], wanted: &str) -> Option<App> {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return None;
    }
    if wanted.ends_with(".desktop") && wanted.starts_with('/') {
        let path = Path::new(wanted);
        let id = base(wanted).trim_end_matches(".desktop");
        return Some(Listed::new(id, path, parse(&read(path)?)?).app());
    }
    let stem = wanted.trim_end_matches(".desktop");
    let named = |listed: &&Listed| {
        listed.entry.name.eq_ignore_ascii_case(stem) || listed.id.eq_ignore_ascii_case(stem)
    };
    if let Some(listed) = entries.iter().find(named).or_else(|| {
        entries
            .iter()
            .find(|listed| matches(&listed.entry, &listed.id, stem))
    }) {
        return Some(listed.app());
    }
    let path = which(wanted)?;
    let resolved = fs::canonicalize(&path)
        .ok()
        .and_then(|path| Some(base(path.to_str()?).to_owned()));
    let path = path.to_string_lossy().into_owned();
    let id = base(&path).to_owned();
    Some(App {
        name: id.clone(),
        aliases: resolved.filter(|name| *name != id).into_iter().collect(),
        id,
        path,
        ..App::default()
    })
}

pub fn launch(app: &App) -> Result<(), String> {
    let entry = app.path.ends_with(".desktop");
    let words = if entry {
        read(Path::new(&app.path))
            .and_then(|text| parse(&text))
            .map(|entry| entry.exec)
            .ok_or_else(|| {
                format!(
                    "{}'s desktop entry could not be read, so nothing ran.",
                    app.name
                )
            })?
    } else {
        vec![app.path.clone()]
    };
    let (program, rest) = words
        .split_first()
        .ok_or_else(|| format!("{} has no program to start, so nothing ran.", app.name))?;
    let program = which(program).ok_or_else(|| {
        format!("{program} is not an executable on this computer, so nothing ran.")
    })?;
    let mut command = if entry {
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "export GIO_LAUNCHED_DESKTOP_FILE_PID=$$; exec \"$0\" \"$@\"",
            ])
            .arg(program)
            .env("GIO_LAUNCHED_DESKTOP_FILE", &app.path);
        command
    } else {
        Command::new(program)
    };
    command
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    if let Some(home) = env::var_os("HOME") {
        command.current_dir(home);
    }
    let mut child = command.spawn().map_err(|error| {
        format!(
            "{} could not be started ({error}), so nothing ran.",
            app.name
        )
    })?;
    thread::spawn(move || child.wait());
    Ok(())
}

pub fn exe(pid: u32) -> Option<String> {
    let path = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let path = path.to_string_lossy();
    Some(path.strip_suffix(" (deleted)").unwrap_or(&path).to_owned())
}
