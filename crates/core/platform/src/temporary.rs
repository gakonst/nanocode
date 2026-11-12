use std::{
    error,
    ffi::{OsStr, OsString},
    fmt,
    fs::{self, File, OpenOptions, Permissions},
    io::{self, Read, Seek, SeekFrom, Write},
    mem,
    ops::Deref,
    path::{Path, PathBuf},
};

use crate::fill_random;

const DEFAULT_PREFIX: &str = ".tmp";
const NAME_RANDOM_CHARACTERS: usize = 6;
const CREATE_ATTEMPTS: u32 = 65_536;
const NAME_ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
#[cfg(unix)]
const OWNER_READ_WRITE_MODE: u32 = 0o600;

#[derive(Debug)]
struct PathError {
    path: PathBuf,
    error: io::Error,
}

impl fmt::Display for PathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at path {:?}", self.error, self.path)
    }
}

impl error::Error for PathError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        self.error.source()
    }
}

fn with_path(error: io::Error, path: &Path) -> io::Error {
    io::Error::new(
        error.kind(),
        PathError {
            path: path.to_path_buf(),
            error,
        },
    )
}

#[derive(Debug)]
pub struct TemporaryPath {
    path: PathBuf,
}

impl TemporaryPath {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(mut self) {
        self.path = PathBuf::new();
        mem::forget(self);
    }
}

impl Deref for TemporaryPath {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for TemporaryPath {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug)]
pub struct TemporaryFile {
    file: File,
    path: TemporaryPath,
}

#[derive(Debug)]
pub struct PersistError {
    pub error: io::Error,
    pub file: TemporaryFile,
}

impl fmt::Display for PersistError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "failed to persist temporary file: {}",
            self.error
        )
    }
}

impl error::Error for PersistError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        Some(&self.error)
    }
}

fn random_name(prefix: &OsStr) -> io::Result<OsString> {
    let mut random = [0_u8; NAME_RANDOM_CHARACTERS * 8];
    fill_random(&mut random)?;
    let mut name = prefix.to_os_string();
    for word in random.as_chunks::<8>().0 {
        let index = u64::from_le_bytes(*word) % NAME_ALPHABET.len() as u64;
        name.push(char::from(NAME_ALPHABET[index as usize]).to_string());
    }
    Ok(name)
}

fn create_with<T>(
    directory: &Path,
    prefix: &OsStr,
    mut create: impl FnMut(PathBuf) -> io::Result<T>,
) -> io::Result<T> {
    let absolute;
    let directory = if directory.is_absolute() {
        directory
    } else {
        absolute = std::env::current_dir()?.join(directory);
        &absolute
    };
    for _ in 0..CREATE_ATTEMPTS {
        let path = directory.join(random_name(prefix)?);
        match create(path) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {}
            result => return result,
        }
    }
    Err(with_path(
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "too many temporary files exist",
        ),
        directory,
    ))
}

fn open_new(path: &Path, permissions: Option<&Permissions>) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(permissions.map_or(OWNER_READ_WRITE_MODE, PermissionsExt::mode));
    }
    #[cfg(not(unix))]
    let _ = permissions;
    options.open(path)
}

impl TemporaryFile {
    pub fn new() -> io::Result<Self> {
        Self::new_in(&std::env::temp_dir())
    }

    pub fn new_in(directory: &Path) -> io::Result<Self> {
        Self::create_in(directory, OsStr::new(DEFAULT_PREFIX), None)
    }

    pub fn with_permissions_in(
        directory: &Path,
        permissions: Option<&Permissions>,
    ) -> io::Result<Self> {
        Self::create_in(directory, OsStr::new(DEFAULT_PREFIX), permissions)
    }

    pub fn create_in(
        directory: &Path,
        prefix: impl AsRef<OsStr>,
        permissions: Option<&Permissions>,
    ) -> io::Result<Self> {
        create_with(directory, prefix.as_ref(), |path| {
            let file = open_new(&path, permissions).map_err(|error| with_path(error, &path))?;
            Ok(Self {
                file,
                path: TemporaryPath { path },
            })
        })
    }

    pub fn make_in(
        directory: &Path,
        prefix: impl AsRef<OsStr>,
        mut open: impl FnMut(&Path) -> io::Result<File>,
    ) -> io::Result<Self> {
        create_with(directory, prefix.as_ref(), |path| {
            let file = open(&path)?;
            Ok(Self {
                file,
                path: TemporaryPath { path },
            })
        })
    }

    #[must_use]
    pub const fn as_file(&self) -> &File {
        &self.file
    }

    pub const fn as_file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn into_temporary_path(self) -> TemporaryPath {
        self.path
    }

    pub fn persist(self, new_path: impl AsRef<Path>) -> Result<File, PersistError> {
        match fs::rename(&self.path, new_path.as_ref()) {
            Ok(()) => {
                self.path.disarm();
                Ok(self.file)
            }
            Err(error) => Err(PersistError { error, file: self }),
        }
    }

    pub fn persist_noclobber(self, new_path: impl AsRef<Path>) -> Result<File, PersistError> {
        match fs::hard_link(&self.path, new_path.as_ref()) {
            Ok(()) => {
                let _ = fs::remove_file(&self.path);
                self.path.disarm();
                Ok(self.file)
            }
            Err(error) => Err(PersistError { error, file: self }),
        }
    }
}

impl Write for TemporaryFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.file
            .write(bytes)
            .map_err(|error| with_path(error, &self.path))
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file
            .write_all(bytes)
            .map_err(|error| with_path(error, &self.path))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file
            .flush()
            .map_err(|error| with_path(error, &self.path))
    }
}

impl Read for TemporaryFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.file
            .read(buffer)
            .map_err(|error| with_path(error, &self.path))
    }
}

impl Seek for TemporaryFile {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file
            .seek(position)
            .map_err(|error| with_path(error, &self.path))
    }
}
