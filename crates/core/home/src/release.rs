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

pub const MAX_BYTES: u64 = 512 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub sha256: String,
    pub size: u64,
}

impl Release {
    pub fn is_valid(&self) -> bool {
        parse_version(&self.version).is_some()
            && self.sha256.len() == 64
            && self.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            && self.size <= MAX_BYTES
    }
}

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

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

#[derive(Debug)]
pub enum InstallError<E: Display> {
    NoDirectory,
    Write(io::Error),
    Download(E),
    TooLarge,
    Mismatch,
    Run(io::Error),
    Version,
    Replace(io::Error),
    Io(io::Error),
}

impl<E: Display> std::fmt::Display for InstallError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDirectory => formatter.write_str("the executable has no directory"),
            Self::Write(value) => write!(formatter, "cannot write next to the executable: {value}"),
            Self::Download(value) => write!(formatter, "{value}"),
            Self::TooLarge => formatter.write_str("the download is larger than its release allows"),
            Self::Mismatch => {
                formatter.write_str("the download doesn't match its release's SHA-256")
            }
            Self::Run(value) => write!(formatter, "the download doesn't run: {value}"),
            Self::Version => {
                formatter.write_str("the download doesn't report its release's version")
            }
            Self::Replace(value) => write!(formatter, "cannot replace the executable: {value}"),
            Self::Io(value) => std::fmt::Display::fmt(value, formatter),
        }
    }
}

impl<E: Display + std::fmt::Debug> std::error::Error for InstallError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(value) => std::error::Error::source(value),
            _ => None,
        }
    }
}

impl<E: Display> From<io::Error> for InstallError<E> {
    fn from(source: io::Error) -> Self {
        Self::Io(source)
    }
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

pub fn restart(exe: &Path) -> io::Error {
    Command::new(exe).args(env::args_os().skip(1)).exec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
