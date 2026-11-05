//! Replacing a program with a newer release of itself: downloaded beside it
//! through the host's source, checked for size and SHA-256, run once to check
//! the version it reports, and renamed over it.

use sha2::{Digest, Sha256};
use std::{
    env,
    fmt::Display,
    fs::{self, File, OpenOptions, Permissions},
    io::{self, Write},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::Path,
    process::{self, Command, Stdio},
};

/// The most a release may weigh.
pub const MAX_BYTES: u64 = 512 << 20;

/// A release: its version and the SHA-256 (lowercase hex) and size (0 when
/// unknown) of its executable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub sha256: String,
    pub size: u64,
}

impl Release {
    /// Whether it is well formed: a three-number version, a SHA-256 and at
    /// most [`MAX_BYTES`].
    pub fn is_valid(&self) -> bool {
        parse_version(&self.version).is_some()
            && self.sha256.len() == 64
            && self.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            && self.size <= MAX_BYTES
    }
}

/// A release version: three numbers of at most 9 digits, nothing else.
pub fn parse_version(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.split('.');
    let mut numbers = [0; 3];
    for number in &mut numbers {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 9 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *number = part.parse().ok()?;
    }
    parts.next().is_none().then_some(numbers)
}

/// Whether `candidate` is a release newer than `current`; every release is
/// newer than a build whose version is not one (a development build).
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// Why [`install`] left the executable as it was.
#[derive(Debug, thiserror::Error)]
pub enum InstallError<E: Display> {
    #[error("the executable has no directory")]
    NoDirectory,
    /// The temporary beside the executable could not be created.
    #[error("cannot write next to the executable: {0}")]
    Write(io::Error),
    #[error("{0}")]
    Download(E),
    #[error("the download is larger than its release allows")]
    TooLarge,
    /// Its size or SHA-256 is not the release's.
    #[error("the download doesn't match its release's SHA-256")]
    Mismatch,
    /// It could not be started.
    #[error("the download doesn't run: {0}")]
    Run(io::Error),
    /// It failed, or reported another version.
    #[error("the download doesn't report its release's version")]
    Version,
    /// It could not be renamed over the executable.
    #[error("cannot replace the executable: {0}")]
    Replace(io::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

struct Hashing<'a> {
    digest: Sha256,
    file: &'a mut File,
    size: u64,
}

impl Write for Hashing<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.file.write(bytes)?;
        self.digest.update(&bytes[..written]);
        self.size += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Replaces `exe` with `release`. `download` writes the release's executable
/// into a new 0700 file `.<name>.update-<pid>` beside `exe`; it must have the
/// release's size (when known) and SHA-256, and running it with
/// `version_args` must succeed and print the release's version as its last
/// word. Only then is it renamed over `exe`; else it is removed.
pub fn install<E: Display>(
    exe: &Path,
    release: &Release,
    version_args: &[&str],
    download: impl FnOnce(&mut dyn Write) -> Result<(), E>,
) -> Result<(), InstallError<E>> {
    let dir = exe.parent().ok_or(InstallError::NoDirectory)?;
    let name = exe.file_name().unwrap_or_default().to_string_lossy();
    let temporary = dir.join(format!(".{name}.update-{}", process::id()));
    let _ = fs::remove_file(&temporary);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)
            .map_err(InstallError::Write)?;
        let mut hashing = Hashing {
            digest: Sha256::new(),
            file: &mut file,
            size: 0,
        };
        download(&mut hashing).map_err(InstallError::Download)?;
        let Hashing { digest, size, .. } = hashing;
        if size > MAX_BYTES {
            return Err(InstallError::TooLarge);
        }
        let digest = hex(&digest.finalize());
        if (release.size > 0 && size != release.size)
            || !digest.eq_ignore_ascii_case(&release.sha256)
        {
            return Err(InstallError::Mismatch);
        }
        file.sync_all()?;
        drop(file);
        fs::set_permissions(&temporary, Permissions::from_mode(0o755))?;
        let output = Command::new(&temporary)
            .args(version_args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(InstallError::Run)?;
        let reported = String::from_utf8_lossy(&output.stdout);
        if !output.status.success()
            || reported.split_whitespace().last() != Some(release.version.as_str())
        {
            return Err(InstallError::Version);
        }
        fs::rename(&temporary, exe).map_err(InstallError::Replace)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Starts `exe` in this process's place with this process's arguments; it
/// returns only the error that stopped it.
pub fn restart(exe: &Path) -> io::Error {
    Command::new(exe).args(env::args_os().skip(1)).exec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn release(version: &str, bytes: &[u8]) -> Release {
        Release {
            version: version.into(),
            sha256: hex(&Sha256::digest(bytes)),
            size: bytes.len() as u64,
        }
    }

    fn script(version: &str) -> Vec<u8> {
        format!("#!/bin/sh\necho \"program {version}\"\n").into_bytes()
    }

    fn installed(dir: &Path) -> PathBuf {
        let exe = dir.join("program");
        fs::write(&exe, "old").unwrap();
        exe
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn releases_are_newer_than_development_builds() {
        assert!(is_newer("1.0.1", "1.0.0"));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("0.9.0", "1.0.0"));
        assert!(is_newer("0.0.1", "0.1.0-dev"));
        assert!(!is_newer("1.0.0-rc1", "0.1.0"));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("v1.2.3"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("1.2.3"), Some([1, 2, 3]));
        assert!(release("1.2.3", b"x").is_valid());
        assert!(!release("1.2", b"x").is_valid());
    }

    #[test]
    fn a_checked_release_replaces_the_executable() {
        let dir = tempfile::tempdir().unwrap();
        let exe = installed(dir.path());
        let bytes = script("1.2.3");
        let result: Result<(), InstallError<io::Error>> =
            install(&exe, &release("1.2.3", &bytes), &["version"], |out| {
                out.write_all(&bytes)
            });
        result.unwrap();
        assert_eq!(fs::read(&exe).unwrap(), bytes);
        assert_eq!(
            fs::metadata(&exe).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(leftovers(dir.path()), ["program"]);
    }

    #[test]
    fn a_release_that_is_not_what_it_says_leaves_the_executable() {
        let dir = tempfile::tempdir().unwrap();
        let exe = installed(dir.path());
        let bytes = script("1.2.3");
        let mut wrong = release("1.2.3", &bytes);
        wrong.sha256 = hex(&Sha256::digest(b"other"));
        let mismatch = install(&exe, &wrong, &["version"], |out| out.write_all(&bytes));
        assert!(matches!(mismatch, Err(InstallError::Mismatch)));
        let other = script("1.2.4");
        let version = install(&exe, &release("1.2.3", &other), &["version"], |out| {
            out.write_all(&other)
        });
        assert!(matches!(version, Err(InstallError::Version)));
        let failed = install(&exe, &release("1.2.3", &bytes), &[], |_| Err("no route"));
        assert_eq!(failed.unwrap_err().to_string(), "no route");
        assert_eq!(fs::read_to_string(&exe).unwrap(), "old");
        assert_eq!(leftovers(dir.path()), ["program"]);
    }
}
