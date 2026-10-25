use std::{
    path::{Path, PathBuf},
    process::Command,
};

use nanocode::cua::{ComputerConfig, browser, browser_socket, fork_driver};

use crate::{
    cli::{CuaArgs, CuaCommand},
    host::{self, Result},
};

const FORK_RELEASE: &str = "https://github.com/gakonst/cua/releases/download/nanocode-cua-driver-v0.38.0-perf.1/cua-driver-nanocode-0.38.0-perf.1-darwin-universal.tar.gz";
const FORK_SHA256: &str = "2567a9586421509affab3753f38024a497ab495a142dfd04d7038d410bd576fd";
const CUA_DRIVER_INSTALLER: &str =
    "https://github.com/trycua/cua/releases/download/cua-driver-rs-v0.38.0/install.sh";

fn home() -> Result<PathBuf> {
    Ok(std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?)
}

fn state() -> Result<PathBuf> {
    Ok(host::home_dir(None)?.join("browser"))
}

fn selected(home: &Path, names: &[String]) -> Result<Vec<browser::Browser>> {
    if names.is_empty() {
        return Ok(browser::browsers(home));
    }
    Ok(names
        .iter()
        .map(|name| browser::browser(home, name))
        .collect::<std::io::Result<_>>()?)
}

fn driver() -> Option<PathBuf> {
    ComputerConfig::cua_driver().ok().and_then(|config| {
        config
            .environment
            .get(std::ffi::OsStr::new("CUA_DRIVER_PATH"))
            .map(PathBuf::from)
    })
}

fn run(command: &mut Command) -> Result<()> {
    let status = command.status()?;
    if !status.success() {
        return Err(format!("{command:?} failed ({status})").into());
    }
    Ok(())
}

pub async fn cua(args: CuaArgs) -> Result<()> {
    let home = home()?;
    let state = state()?;
    match args.command {
        CuaCommand::Install {
            browser: names,
            skip_driver,
        } => {
            if !skip_driver && let Some(fork) = fork_driver(&home) {
                let directory = fork.parent().ok_or("invalid cua-driver path")?;
                std::fs::create_dir_all(directory)?;
                println!("Installing nanocode's cua-driver build from {FORK_RELEASE}");
                run(Command::new("/bin/bash").arg("-c").arg(format!(
                    "set -euo pipefail; archive=$(mktemp); trap 'rm -f \"$archive\"' EXIT; curl -fsSL '{FORK_RELEASE}' -o \"$archive\"; echo '{FORK_SHA256}  '\"$archive\" | shasum -a 256 -c - >/dev/null; tar xzf \"$archive\" -C '{}' cua-driver",
                    directory.display()
                )))?;
                println!(
                    "cua-driver: {} (runs inside nanocode, so the app running nanocode needs Accessibility and Screen & System Audio Recording)",
                    fork.display()
                );
            } else if !skip_driver {
                match driver() {
                    Some(path) => println!("cua-driver: {}", path.display()),
                    None => {
                        println!("Installing cua-driver from {CUA_DRIVER_INSTALLER}");
                        run(Command::new("/bin/bash").arg("-c").arg(format!(
                            "set -o pipefail; curl -fsSL '{CUA_DRIVER_INSTALLER}' | bash"
                        )))?;
                    }
                }
                let path = driver().ok_or(
                    "cua-driver is still missing (or Node.js 22+ is not installed); set CUA_DRIVER_PATH and CUA_REPL_NODE",
                )?;
                if cfg!(target_os = "macos") {
                    println!(
                        "Allow CuaDriver under Accessibility and Screen & System Audio Recording when macOS asks"
                    );
                    run(Command::new(&path).args(["permissions", "grant"]))?;
                }
            }
            let browsers = selected(&home, &names)?;
            if browsers.is_empty() {
                println!("No Chromium browser found; pass --browser NAME or a user data directory");
                return Ok(());
            }
            let extension = browser::install(
                &state,
                &std::env::current_exe()?,
                &["cua", "browser-host"],
                &browsers,
            )?;
            for browser in &browsers {
                println!(
                    "{}: native host {} — extension {}",
                    browser.name,
                    browser.host_manifest().display(),
                    if browser.extension_loaded() {
                        "loaded"
                    } else {
                        "not loaded yet"
                    }
                );
            }
            if browsers.iter().any(|browser| !browser.extension_loaded()) {
                println!(
                    "\nLoad the nanocode extension once in each browser: open chrome://extensions, turn on Developer mode, choose Load unpacked and pick\n  {}\nIts ID is {}.",
                    extension.display(),
                    browser::EXTENSION_ID
                );
            }
        }
        CuaCommand::Uninstall { browser: names } => {
            let browsers = selected(&home, &names)?;
            let removed = browser::uninstall(&state, &browsers)?;
            println!(
                "Removed the nanocode native host from {}; remove the nanocode extension from chrome://extensions",
                if removed.is_empty() {
                    "no browser".to_owned()
                } else {
                    removed.join(", ")
                }
            );
        }
        CuaCommand::Status => {
            match ComputerConfig::cua_driver() {
                Ok(config) => println!(
                    "node: {}\ncua-driver: {}",
                    config.executable.display(),
                    driver().map_or_else(String::new, |path| path.display().to_string())
                ),
                Err(error) => println!("cua-driver backend unavailable: {error}"),
            }
            for browser in browser::browsers(&home) {
                println!(
                    "{}: native host {}, extension {}",
                    browser.name,
                    if browser.host_manifest().is_file() {
                        "installed"
                    } else {
                        "missing"
                    },
                    if browser.extension_loaded() {
                        "loaded"
                    } else {
                        "not loaded"
                    }
                );
            }
            println!("browser socket: {}", browser_socket(&home).display());
        }
        CuaCommand::BrowserHost { .. } => {
            let socket = browser_socket(&home);
            tokio::task::spawn_blocking(move || browser::serve_host(&socket)).await??;
        }
    }
    Ok(())
}
