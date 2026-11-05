use crate::App;
use std::{
    env, fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
};

pub struct Entry {
    pub name: String,
    pub exec: Vec<String>,
}

pub fn parse(text: &str) -> Option<Entry> {
    let mut group = "";
    let (mut name, mut exec, mut application, mut hidden) = (None, None, false, false);
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
            "Type" => application = value.trim() == "Application",
            "Hidden" => hidden = value.trim() == "true",
            _ => {}
        }
    }
    let exec = exec.filter(|exec| !exec.is_empty())?;
    (application && !hidden).then_some(Entry {
        name: name.filter(|name| !name.is_empty())?,
        exec,
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

fn base(path: &str) -> &str {
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
        .map(|dir| dir.join("applications"))
        .collect()
}

fn app(name: String, program: &str, path: String) -> App {
    App {
        pid: 0,
        name,
        id: base(program).to_owned(),
        path,
        background: false,
        windows: false,
    }
}

pub fn matches(entry: &Entry, stem: &str, wanted: &str) -> bool {
    entry.name.eq_ignore_ascii_case(wanted)
        || stem.eq_ignore_ascii_case(wanted)
        || entry
            .exec
            .first()
            .is_some_and(|program| base(program).eq_ignore_ascii_case(wanted))
}

pub fn installed(wanted: &str) -> Option<App> {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return None;
    }
    if wanted.ends_with(".desktop") && wanted.starts_with('/') {
        let entry = parse(&fs::read_to_string(wanted).ok()?)?;
        return Some(app(entry.name, &entry.exec[0], wanted.to_owned()));
    }
    let stem = wanted.trim_end_matches(".desktop");
    for dir in dirs() {
        let Ok(files) = fs::read_dir(&dir) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(file_stem) = name.strip_suffix(".desktop") else {
                continue;
            };
            let Some(entry) = fs::read_to_string(&path).ok().and_then(|text| parse(&text)) else {
                continue;
            };
            if matches(&entry, file_stem, stem) {
                return Some(app(
                    entry.name,
                    &entry.exec[0],
                    path.to_string_lossy().into_owned(),
                ));
            }
        }
    }
    let path = which(wanted)?;
    let path = path.to_string_lossy().into_owned();
    Some(app(base(&path).to_owned(), &path, path.clone()))
}

pub fn launch(app: &App) -> Result<(), String> {
    let words = if app.path.ends_with(".desktop") {
        fs::read_to_string(&app.path)
            .ok()
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
    let mut command = Command::new(program);
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
    fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}
