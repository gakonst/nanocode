use nanocode_sync::Atom;
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt as _, net::UnixStream},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};

pub const HOST_NAME: &str = "xyz.nanocode.browser";
pub const EXTENSION_ID: &str = "hifgmffkihloigeaannkmimoglgminee";
const MANIFEST: &str = include_str!("browser/manifest.json");
const BACKGROUND: &str = include_str!("browser/background.js");
const RECONNECT: Duration = Duration::from_millis(500);
const DISCONNECTED: &[u8] = br#"{"type":"disconnected"}"#;

#[cfg(target_os = "macos")]
const BROWSERS: &[(&str, &str)] = &[
    ("Chrome", "Library/Application Support/Google/Chrome"),
    (
        "Chrome Beta",
        "Library/Application Support/Google/Chrome Beta",
    ),
    (
        "Chrome Canary",
        "Library/Application Support/Google/Chrome Canary",
    ),
    (
        "Chrome for Testing",
        "Library/Application Support/Google/Chrome for Testing",
    ),
    ("Chromium", "Library/Application Support/Chromium"),
    ("Edge", "Library/Application Support/Microsoft Edge"),
    (
        "Brave",
        "Library/Application Support/BraveSoftware/Brave-Browser",
    ),
    ("Arc", "Library/Application Support/Arc/User Data"),
    ("Vivaldi", "Library/Application Support/Vivaldi"),
];

#[cfg(not(target_os = "macos"))]
const BROWSERS: &[(&str, &str)] = &[
    ("Chrome", ".config/google-chrome"),
    ("Chrome Beta", ".config/google-chrome-beta"),
    ("Chrome for Testing", ".config/google-chrome-for-testing"),
    ("Chromium", ".config/chromium"),
    ("Edge", ".config/microsoft-edge"),
    ("Brave", ".config/BraveSoftware/Brave-Browser"),
    ("Vivaldi", ".config/vivaldi"),
];

#[derive(Clone, Debug)]
pub struct Browser {
    pub name: String,
    pub root: PathBuf,
}

impl Browser {
    pub fn host_manifest(&self) -> PathBuf {
        self.root
            .join("NativeMessagingHosts")
            .join(format!("{HOST_NAME}.json"))
    }

    pub fn extension_loaded(&self) -> bool {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let profile = entry.path();
            ["Secure Preferences", "Preferences"].iter().any(|file| {
                fs::read(profile.join(file))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .is_some_and(|preferences| {
                        preferences["extensions"]["settings"][EXTENSION_ID].is_object()
                    })
            }) || extension_state_mentions(&profile)
        })
    }
}

fn extension_state_mentions(profile: &Path) -> bool {
    let Ok(files) = fs::read_dir(profile.join("Extension State")) else {
        return false;
    };
    files.flatten().any(|file| {
        fs::read(file.path()).is_ok_and(|bytes| {
            bytes
                .windows(EXTENSION_ID.len())
                .any(|window| window == EXTENSION_ID.as_bytes())
        })
    })
}

pub fn browsers(home: &Path) -> Vec<Browser> {
    BROWSERS
        .iter()
        .map(|(name, path)| Browser {
            name: (*name).to_owned(),
            root: home.join(path),
        })
        .filter(|browser| browser.root.is_dir())
        .collect()
}

pub fn browser(home: &Path, name: &str) -> io::Result<Browser> {
    if Path::new(name).is_absolute() {
        return Ok(Browser {
            name: name.to_owned(),
            root: PathBuf::from(name),
        });
    }
    BROWSERS
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(known, path)| Browser {
            name: (*known).to_owned(),
            root: home.join(path),
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "unknown browser {name:?}; use one of {} or an absolute Chromium user data directory",
                    BROWSERS
                        .iter()
                        .map(|(known, _)| *known)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        })
}

pub fn extension_dir(state: &Path) -> PathBuf {
    state.join("extension")
}

fn launcher_path(state: &Path) -> PathBuf {
    state.join("browser-host")
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn write_mode(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(mode))?;
    fs::rename(temporary, path)
}

pub fn install(
    state: &Path,
    program: &Path,
    arguments: &[&str],
    browsers: &[Browser],
) -> io::Result<PathBuf> {
    fs::create_dir_all(state)?;
    let extension = extension_dir(state);
    fs::create_dir_all(&extension)?;
    write_mode(&extension.join("manifest.json"), MANIFEST.as_bytes(), 0o644)?;
    write_mode(
        &extension.join("background.js"),
        BACKGROUND.as_bytes(),
        0o644,
    )?;
    let program = program
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "program path is not UTF-8"))?;
    let mut script = format!("#!/bin/sh\nexec {}", quote(program));
    for argument in arguments {
        script.push(' ');
        script.push_str(&quote(argument));
    }
    script.push_str(" \"$@\"\n");
    let launcher = launcher_path(state);
    write_mode(&launcher, script.as_bytes(), 0o755)?;
    let manifest = serde_json::to_vec_pretty(&json!({
        "name": HOST_NAME,
        "description": "nanocode computer use: connects the nanocode extension to cua_repl",
        "path": launcher,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    }))?;
    for browser in browsers {
        let path = browser.host_manifest();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_mode(&path, &manifest, 0o644)?;
    }
    Ok(extension)
}

pub fn uninstall(state: &Path, browsers: &[Browser]) -> io::Result<Vec<String>> {
    let launcher = launcher_path(state);
    let mut removed = Vec::new();
    for browser in browsers {
        let path = browser.host_manifest();
        let ours = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|manifest| manifest["path"] == launcher.to_string_lossy().as_ref());
        if ours {
            fs::remove_file(&path)?;
            removed.push(browser.name.clone());
        }
    }
    let _ = fs::remove_file(launcher);
    let extension = extension_dir(state);
    if extension.is_dir() {
        fs::remove_dir_all(extension)?;
    }
    Ok(removed)
}

fn read_frame(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut length = [0; 4];
    match input.read_exact(&mut length) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let mut body = vec![0; u32::from_ne_bytes(length) as usize];
    input.read_exact(&mut body)?;
    Ok(Some(body))
}

fn write_frame(output: &mut impl Write, body: &[u8]) -> io::Result<()> {
    let length = u32::try_from(body.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "message too large"))?;
    output.write_all(&length.to_ne_bytes())?;
    output.write_all(body)?;
    output.flush()
}

pub fn serve_host(socket: &Path) -> io::Result<()> {
    let provider: Arc<Atom<Option<Arc<UnixStream>>>> = Arc::default();
    let forward = Arc::clone(&provider);
    thread::spawn(move || {
        let mut input = io::stdin().lock();
        while let Ok(Some(body)) = read_frame(&mut input) {
            if let Some(stream) = Option::clone(&forward.get()) {
                let mut writer = &*stream;
                if writer.write_all(&body).is_err() || writer.write_all(b"\n").is_err() {
                    forward.try_update(|current| {
                        current
                            .as_ref()
                            .is_some_and(|current| Arc::ptr_eq(current, &stream))
                            .then(|| *current = None)
                    });
                }
            }
        }
        std::process::exit(0);
    });
    let mut output = io::stdout();
    loop {
        let Ok(stream) = UnixStream::connect(socket) else {
            thread::sleep(RECONNECT);
            continue;
        };
        provider.set(Some(Arc::new(stream.try_clone()?)));
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            write_frame(&mut output, line.as_bytes())?;
        }
        provider.set(None);
        write_frame(&mut output, DISCONNECTED)?;
        thread::sleep(RECONNECT);
    }
}
