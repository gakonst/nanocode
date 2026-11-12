// Adapted from the `ignore` crate's `gitignore.rs` (`gitconfig_excludes_path` and its
// helpers) in ripgrep (https://github.com/BurntSushi/ripgrep, commit 91f5d22):
// Copyright (c) 2015 Andrew Gallant. Licensed under the MIT License (see NOTICE).

//! Where git's global excludes file is, found from the shell's environment and file
//! system rather than the process's (D1132).

use std::path::PathBuf;

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
    let candidate = excludes_file_value(data)?;
    // `~` anywhere is replaced by the home directory, as `ignore` does.
    Some(match home {
        Some(home) => candidate
            .replace('~', &String::from_utf8_lossy(home))
            .into_bytes(),
        None => candidate.as_bytes().to_vec(),
    })
}

pub(super) fn native_excludes_path() -> Option<PathBuf> {
    let read = |path: PathBuf| std::fs::read(path).ok();
    let nonempty = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let parse =
        |data: Vec<u8>| excludes_file_value(&data).map(|value| PathBuf::from(expand_tilde(value)));
    let config_home = || {
        nonempty("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::home_dir().map(|home| home.join(".config")))
    };
    nonempty("GIT_CONFIG_GLOBAL")
        .and_then(|path| read(PathBuf::from(path)))
        .and_then(parse)
        .or_else(|| {
            std::env::home_dir()
                .and_then(|home| read(home.join(".gitconfig")))
                .and_then(parse)
        })
        .or_else(|| {
            config_home()
                .and_then(|base| read(base.join("git/config")))
                .and_then(parse)
        })
        .or_else(|| {
            let system = nonempty("GIT_CONFIG_SYSTEM").unwrap_or_else(|| "/etc/gitconfig".into());
            read(PathBuf::from(system)).and_then(parse)
        })
        .or_else(|| config_home().map(|base| base.join("git/ignore")))
}

fn expand_tilde(path: &str) -> String {
    match std::env::home_dir() {
        Some(home) => path.replace('~', &home.to_string_lossy()),
        None => path.to_string(),
    }
}

#[derive(Clone, Copy)]
enum Piece {
    Spaces,
    Keyword,
    Equals,
    OptionalQuote,
    Value,
    LineEnd,
}

const KEYWORD: &[u8] = b"excludesfile";

const PIECES: [Piece; 12] = [
    Piece::Spaces,
    Piece::Keyword,
    Piece::Spaces,
    Piece::Equals,
    Piece::Spaces,
    Piece::OptionalQuote,
    Piece::Spaces,
    Piece::Value,
    Piece::Spaces,
    Piece::OptionalQuote,
    Piece::Spaces,
    Piece::LineEnd,
];

fn excludes_file_value(data: &[u8]) -> Option<&str> {
    let mut search = Search {
        data,
        failed: vec![false; PIECES.len() * (data.len() + 1)],
        value: None,
    };
    let starts = (0..=data.len()).filter(|&at| at == 0 || data[at - 1] == b'\n');
    for start in starts {
        if search.matches(0, start) {
            let (from, to) = search.value.expect("a match captures the value");
            return std::str::from_utf8(&data[from..to]).ok();
        }
    }
    None
}

struct Search<'d> {
    data: &'d [u8],
    failed: Vec<bool>,
    value: Option<(usize, usize)>,
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b'\t' | b'\n' | 0x0b | 0x0c | b'\r' | b' ')
}

impl Search<'_> {
    fn matches(&mut self, piece: usize, at: usize) -> bool {
        let Some(&kind) = PIECES.get(piece) else {
            return true;
        };
        let key = piece * (self.data.len() + 1) + at;
        if self.failed[key] {
            return false;
        }
        let data = self.data;
        let found = match kind {
            Piece::Spaces => {
                let run = data[at..].iter().take_while(|&&b| is_space(b)).count();
                (at..=at + run)
                    .rev()
                    .any(|end| self.matches(piece + 1, end))
            }
            Piece::Keyword => {
                data.get(at..at + KEYWORD.len())
                    .is_some_and(|word| word.eq_ignore_ascii_case(KEYWORD))
                    && self.matches(piece + 1, at + KEYWORD.len())
            }
            Piece::Equals => data.get(at) == Some(&b'=') && self.matches(piece + 1, at + 1),
            Piece::OptionalQuote => {
                (data.get(at) == Some(&b'"') && self.matches(piece + 1, at + 1))
                    || self.matches(piece + 1, at)
            }
            Piece::Value => {
                let run = data[at..].iter().take_while(|&&b| !is_space(b)).count();
                let end = (at + 1..=at + run).find(|&end| self.matches(piece + 1, end));
                if let Some(end) = end {
                    self.value = Some((at, end));
                }
                end.is_some()
            }
            Piece::LineEnd => at == data.len() || data[at] == b'\n',
        };
        if !found {
            self.failed[key] = true;
        }
        found
    }
}
