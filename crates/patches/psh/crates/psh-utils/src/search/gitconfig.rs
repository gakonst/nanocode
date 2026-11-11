// Adapted from the `ignore` crate's `gitignore.rs` (`gitconfig_excludes_path` and its
// helpers) in ripgrep (https://github.com/BurntSushi/ripgrep, commit 91f5d22):
// Copyright (c) 2015 Andrew Gallant. Licensed under the MIT License (see NOTICE).

//! Where git's global excludes file is, found from the shell's environment and file
//! system rather than the process's (D1132).

use std::sync::OnceLock;

use regex_automata::meta::Regex;
use regex_automata::util::syntax;

use super::walk::{WalkHost, join};

/// The path of the global gitignore (`core.excludesFile`, else
/// `$XDG_CONFIG_HOME/git/ignore` or `~/.config/git/ignore`); it may not exist.
pub fn excludes_path(host: &dyn WalkHost) -> Option<Vec<u8>> {
    let home = home_dir(host);
    let read = |p: &[u8]| host.read_file(&host.abs(p)).ok();
    let nonempty = |name: &[u8]| host.var(name).filter(|v| !v.is_empty());
    // GIT_CONFIG_GLOBAL replaces both ~/.gitconfig and $XDG_CONFIG_HOME/git/config.
    let global_env = || {
        host.var(b"GIT_CONFIG_GLOBAL")
            .filter(|v| !v.is_empty())
            .and_then(read)
            .and_then(|x| parse_excludes_file(&x, home.as_deref()))
    };
    let home_config = || {
        let home = home.as_deref()?;
        read(&join(home, b".gitconfig")).and_then(|x| parse_excludes_file(&x, Some(home)))
    };
    let config_home = || {
        nonempty(b"XDG_CONFIG_HOME")
            .map(<[u8]>::to_vec)
            .or_else(|| home.as_deref().map(|h| join(h, b".config")))
    };
    let xdg_config = || {
        read(&join(&config_home()?, b"git/config"))
            .and_then(|x| parse_excludes_file(&x, home.as_deref()))
    };
    let system = || {
        let path = nonempty(b"GIT_CONFIG_SYSTEM").unwrap_or(b"/etc/gitconfig");
        read(path).and_then(|x| parse_excludes_file(&x, home.as_deref()))
    };
    global_env()
        .or_else(home_config)
        .or_else(xdg_config)
        .or_else(system)
        .or_else(|| config_home().map(|c| join(&c, b"git/ignore")))
}

/// The shell's idea of the home directory: `HOME`, on Windows also `USERPROFILE`.
fn home_dir(host: &dyn WalkHost) -> Option<Vec<u8>> {
    let home = host.var(b"HOME").filter(|v| !v.is_empty());
    let home = if cfg!(windows) {
        home.or_else(|| host.var(b"USERPROFILE").filter(|v| !v.is_empty()))
    } else {
        home
    };
    home.map(<[u8]>::to_vec)
}

/// git's `core.excludesfile` from the raw contents of a config file. Like `ignore`, a
/// lazy line match rather than an INI parser.
fn parse_excludes_file(data: &[u8], home: Option<&[u8]>) -> Option<Vec<u8>> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::builder()
            .configure(Regex::config().utf8_empty(false))
            .syntax(syntax::Config::new().utf8(false))
            .build(r#"(?im-u)^\s*excludesfile\s*=\s*"?\s*(\S+?)\s*"?\s*$"#)
            .unwrap()
    });
    let mut caps = re.create_captures();
    re.captures(data, &mut caps);
    let span = caps.get_group(1)?;
    let candidate = std::str::from_utf8(&data[span]).ok()?;
    // `~` anywhere is replaced by the home directory, as `ignore` does.
    Some(match home {
        Some(home) => candidate
            .replace('~', &String::from_utf8_lossy(home))
            .into_bytes(),
        None => candidate.as_bytes().to_vec(),
    })
}
