use std::{
    fs::{self, File, Metadata, ReadDir},
    io,
    path::{Path, PathBuf},
};

mod append_log;
pub use append_log::AppendLog;

pub fn open(path: impl AsRef<Path>) -> io::Result<File> {
    File::open(path)
}

pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    fs::read(path)
}

pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    fs::read_to_string(path)
}

pub fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDir> {
    fs::read_dir(path)
}

pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    fs::create_dir_all(path)
}

pub fn remove_dir(path: impl AsRef<Path>) -> io::Result<()> {
    fs::remove_dir(path)
}

pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    fs::remove_file(path)
}

pub fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    fs::metadata(path)
}

pub fn exists(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok()
}

pub fn is_file(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok_and(|metadata| metadata.is_file())
}

pub fn is_dir(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

pub fn symlink_metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    fs::symlink_metadata(path)
}

pub fn open_write(path: impl AsRef<Path>) -> io::Result<File> {
    fs::OpenOptions::new().write(true).open(path)
}

pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    fs::canonicalize(path)
}

/// Resolve `binary_name` to an absolute path by searching `PATH` (and `PATHEXT` on Windows).
#[cfg(not(target_arch = "wasm32"))]
pub fn which<T: AsRef<std::ffi::OsStr>>(binary_name: T) -> Option<PathBuf> {
    let binary = PathBuf::from(binary_name.as_ref());
    let candidates: Vec<PathBuf> = match std::env::current_dir().ok() {
        _ if binary.is_absolute() => vec![binary],
        Some(current) if binary.components().count() > 1 => {
            let mut absolute = current;
            absolute.extend(
                binary
                    .components()
                    .skip_while(|component| matches!(component, std::path::Component::CurDir)),
            );
            vec![absolute]
        }
        _ => {
            let paths = std::env::var_os("PATH")?;
            let directories = std::env::split_paths(&paths);
            #[cfg(windows)]
            let directories = directories.filter(|directory| !directory.as_os_str().is_empty());
            directories
                .map(|directory| home_expanded(&directory).join(&binary))
                .collect()
        }
    };
    let extensions = executable_extensions();
    candidates
        .into_iter()
        .flat_map(|candidate| with_extensions(candidate, &extensions))
        .find(|candidate| is_executable_file(candidate))
        .map(corrected_casing)
}

#[cfg(not(target_arch = "wasm32"))]
fn home_expanded(directory: &Path) -> PathBuf {
    let mut components = directory.components();
    if let Some(std::path::Component::Normal(first)) = components.next()
        && first == "~"
        && let Some(mut home) = std::env::home_dir()
    {
        home.extend(components);
        return home;
    }
    directory.to_path_buf()
}

#[cfg(windows)]
fn executable_extensions() -> Vec<String> {
    std::env::var_os("PATHEXT")
        .and_then(|extensions| extensions.into_string().ok())
        .map(|extensions| {
            extensions
                .split(';')
                .filter(|extension| extension.starts_with('.'))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(all(not(windows), not(target_arch = "wasm32")))]
const fn executable_extensions() -> Vec<String> {
    Vec::new()
}

#[cfg(not(target_arch = "wasm32"))]
fn with_extensions(candidate: PathBuf, extensions: &[String]) -> Vec<PathBuf> {
    let has_executable_extension = candidate
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(&known[1..]))
        });
    if extensions.is_empty() || has_executable_extension {
        return vec![candidate];
    }
    let mut expanded = Vec::with_capacity(extensions.len() + 1);
    for extension in extensions {
        let mut extended = candidate.clone().into_os_string();
        extended.push(extension);
        expanded.push(PathBuf::from(extended));
    }
    expanded.insert(0, candidate);
    expanded
}

#[cfg(unix)]
fn is_executable_file(candidate: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let is_file = fs::metadata(candidate).is_ok_and(|metadata| metadata.is_file());
    let Ok(path) = std::ffi::CString::new(candidate.as_os_str().as_bytes()) else {
        return false;
    };
    is_file && unsafe { libc::access(path.as_ptr(), libc::X_OK) } == 0
}

#[cfg(windows)]
fn is_executable_file(candidate: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    let exists = fs::symlink_metadata(candidate)
        .is_ok_and(|metadata| metadata.is_file() || metadata.is_symlink());
    if !exists {
        return false;
    }
    if candidate.extension().is_some() {
        return true;
    }
    let wide: Vec<u16> = candidate
        .as_os_str()
        .encode_wide()
        .chain(core::iter::once(0))
        .collect();
    let mut binary_type = 0u32;
    unsafe {
        windows_sys::Win32::Storage::FileSystem::GetBinaryTypeW(wide.as_ptr(), &mut binary_type)
            != 0
    }
}

#[cfg(not(any(unix, windows, target_arch = "wasm32")))]
const fn is_executable_file(_candidate: &Path) -> bool {
    false
}

#[cfg(windows)]
fn corrected_casing(mut candidate: PathBuf) -> PathBuf {
    let (Some(parent), Some(name)) = (candidate.parent(), candidate.file_name()) else {
        return candidate;
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return candidate;
    };
    let name = name.to_owned();
    if let Some(entry) = entries
        .flatten()
        .find(|entry| entry.file_name().eq_ignore_ascii_case(&name))
    {
        candidate.pop();
        candidate.push(entry.file_name());
    }
    candidate
}

#[cfg(all(not(windows), not(target_arch = "wasm32")))]
const fn corrected_casing(candidate: PathBuf) -> PathBuf {
    candidate
}

#[cfg(windows)]
pub fn open_write_with_custom_flags(path: impl AsRef<Path>, flags: u32) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .write(true)
        .custom_flags(flags)
        .open(path)
}
