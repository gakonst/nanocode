use crate::{EDGE_EXTENSION_ID, EXTENSION_ID};
use nanocode_sync::Atom;
use serde_json::Value;
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::LazyLock,
    time::SystemTime,
};

const LAUNCH_SERVICES: &str =
    "Library/Preferences/com.apple.LaunchServices/com.apple.launchservices.secure.plist";

struct DefaultBrowser {
    plist: PathBuf,
    len: u64,
    modified: SystemTime,
    browser: Option<&'static str>,
}

static DEFAULT_BROWSER: LazyLock<Atom<Option<DefaultBrowser>>> = LazyLock::new(Atom::default);

pub fn config_base(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".config"))
    }
}

pub fn browser_dirs(base: &Path) -> Vec<(&'static str, PathBuf)> {
    let names: &[(&str, &str, &str)] = &[
        ("Chrome", "Google/Chrome", "google-chrome"),
        ("Chrome Beta", "Google/Chrome Beta", "google-chrome-beta"),
        ("Chrome Dev", "Google/Chrome Dev", "google-chrome-unstable"),
        (
            "Chrome Canary",
            "Google/Chrome Canary",
            "google-chrome-canary",
        ),
        (
            "Chrome for Testing",
            "Google/ChromeForTesting",
            "google-chrome-for-testing",
        ),
        (
            "Chrome for Testing (spaced)",
            "Google/Chrome for Testing",
            "google-chrome-for-testing",
        ),
        ("Chromium", "Chromium", "chromium"),
        ("Helium", "net.imput.helium", "net.imput.helium"),
        (
            "Brave",
            "BraveSoftware/Brave-Browser",
            "BraveSoftware/Brave-Browser",
        ),
        (
            "Brave Beta",
            "BraveSoftware/Brave-Browser-Beta",
            "BraveSoftware/Brave-Browser-Beta",
        ),
        (
            "Brave Nightly",
            "BraveSoftware/Brave-Browser-Nightly",
            "BraveSoftware/Brave-Browser-Nightly",
        ),
        ("Edge", "Microsoft Edge", "microsoft-edge"),
        ("Edge Beta", "Microsoft Edge Beta", "microsoft-edge-beta"),
        ("Edge Dev", "Microsoft Edge Dev", "microsoft-edge-dev"),
        (
            "Edge Canary",
            "Microsoft Edge Canary",
            "microsoft-edge-canary",
        ),
        ("Arc", "Arc/User Data", "arc"),
        ("Opera", "com.operasoftware.Opera", "opera"),
        ("Vivaldi", "Vivaldi", "vivaldi"),
        ("Vivaldi Snapshot", "Vivaldi Snapshot", "vivaldi-snapshot"),
    ];
    names
        .iter()
        .map(|(name, mac, linux)| {
            (
                *name,
                base.join(if cfg!(target_os = "macos") {
                    mac
                } else {
                    linux
                }),
            )
        })
        .collect()
}

pub fn profiles(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    if root.join("Extensions").is_dir() || root.join("Preferences").is_file() {
        result.push(root.to_path_buf());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "Default" || name.starts_with("Profile ") {
            result.push(entry.path());
        }
    }
    Ok(result)
}

pub fn chatgpt_installed(root: &Path) -> io::Result<bool> {
    Ok(profiles(root)?.iter().any(|profile| {
        [EXTENSION_ID, EDGE_EXTENSION_ID]
            .iter()
            .any(|id| profile.join("Extensions").join(id).is_dir())
    }))
}

pub fn extension_on(entry: &Value) -> bool {
    let disabled = match &entry["disable_reasons"] {
        Value::Array(reasons) => !reasons.is_empty(),
        Value::Number(reasons) => reasons.as_u64() != Some(0),
        _ => false,
    };
    !disabled && entry["state"].as_u64().unwrap_or(1) != 0
}

pub fn extension_enabled(root: &Path, extension: &str) -> io::Result<bool> {
    Ok(profiles(root)?.iter().any(|profile| {
        let mut recorded = false;
        for name in ["Secure Preferences", "Preferences"] {
            let entry = fs::read(profile.join(name))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .map(|value| value["extensions"]["settings"][extension].clone())
                .filter(|entry| !entry.is_null());
            if let Some(entry) = entry {
                if extension_on(&entry) {
                    return true;
                }
                recorded = true;
            }
        }
        !recorded && profile.join("Extensions").join(extension).is_dir()
    }))
}

pub fn disable_reasons(root: &Path, extension: &str) -> io::Result<Vec<u64>> {
    let mut reasons = Vec::new();
    for profile in profiles(root)? {
        let entry = ["Secure Preferences", "Preferences"]
            .iter()
            .find_map(|name| {
                fs::read(profile.join(name))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .map(|value| value["extensions"]["settings"][extension].clone())
                    .filter(|entry| !entry.is_null())
            });
        let Some(entry) = entry.filter(|entry| !extension_on(entry)) else {
            continue;
        };
        let found: Vec<u64> = match &entry["disable_reasons"] {
            Value::Array(listed) => listed.iter().filter_map(Value::as_u64).collect(),
            Value::Number(mask) => mask.as_u64().map_or_else(Vec::new, |mask| {
                (0..u64::BITS)
                    .map(|bit| 1 << bit)
                    .filter(|reason| mask & reason != 0)
                    .collect()
            }),
            _ => Vec::new(),
        };
        for reason in found {
            if !reasons.contains(&reason) {
                reasons.push(reason);
            }
        }
    }
    Ok(reasons)
}

pub fn browser_for_bundle(bundle: &str) -> Option<&'static str> {
    let browsers: &[(&str, &str)] = &[
        ("com.google.chrome", "Chrome"),
        ("com.google.chrome.beta", "Chrome Beta"),
        ("com.google.chrome.dev", "Chrome Dev"),
        ("com.google.chrome.canary", "Chrome Canary"),
        ("com.google.chrome.for.testing", "Chrome for Testing"),
        ("org.chromium.chromium", "Chromium"),
        ("net.imput.helium", "Helium"),
        ("com.brave.browser", "Brave"),
        ("com.brave.browser.beta", "Brave Beta"),
        ("com.brave.browser.nightly", "Brave Nightly"),
        ("com.microsoft.edgemac", "Edge"),
        ("com.microsoft.edgemac.beta", "Edge Beta"),
        ("com.microsoft.edgemac.dev", "Edge Dev"),
        ("com.microsoft.edgemac.canary", "Edge Canary"),
        ("company.thebrowser.browser", "Arc"),
        ("com.operasoftware.opera", "Opera"),
        ("com.vivaldi.vivaldi", "Vivaldi"),
        ("com.vivaldi.vivaldi.snapshot", "Vivaldi Snapshot"),
    ];
    browsers
        .iter()
        .find(|(id, _)| id.eq_ignore_ascii_case(bundle))
        .map(|(_, name)| *name)
}

pub fn default_from_handlers(value: &Value) -> Option<&'static str> {
    let handlers = value["LSHandlers"].as_array()?;
    ["https", "http"].iter().find_map(|scheme| {
        handlers
            .iter()
            .find(|handler| {
                handler["LSHandlerURLScheme"]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case(scheme))
            })
            .and_then(|handler| handler["LSHandlerRoleAll"].as_str())
            .and_then(browser_for_bundle)
    })
}

pub fn default_browser(home: &Path) -> Option<&'static str> {
    let plist = home.join(LAUNCH_SERVICES);
    let output = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(plist)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    default_from_handlers(&serde_json::from_slice(&output.stdout).ok()?)
}

pub fn cached_default_browser(home: &Path) -> Option<&'static str> {
    let plist = home.join(LAUNCH_SERVICES);
    let meta = fs::metadata(&plist).ok()?;
    let (len, modified) = (meta.len(), meta.modified().ok()?);
    if let Some(cached) = DEFAULT_BROWSER.get().as_ref()
        && cached.plist == plist
        && cached.len == len
        && cached.modified == modified
    {
        return cached.browser;
    }
    let browser = default_browser(home);
    DEFAULT_BROWSER.set(Some(DefaultBrowser {
        plist,
        len,
        modified,
        browser,
    }));
    browser
}
