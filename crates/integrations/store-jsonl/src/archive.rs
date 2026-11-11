use std::{
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::sync_journal;

const SUFFIX: &str = ".zst";
const FORMAT: u8 = 1;
const LEVEL: i32 = 1;
static DICTIONARY: &[u8] = include_bytes!("session.dict");

pub(super) fn archived(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(SUFFIX);
    path.with_file_name(name)
}

pub(super) fn unarchived(name: &str) -> Option<&str> {
    name.strip_suffix(SUFFIX)
}

pub(super) fn exists(path: &Path) -> bool {
    path.is_file() || archived(path).is_file()
}

pub(super) fn compress(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut compressor = zstd::bulk::Compressor::with_dictionary(LEVEL, DICTIONARY)?;
    let frame = compressor.compress(bytes)?;
    let mut archive = Vec::with_capacity(frame.len() + 1);
    archive.push(FORMAT);
    archive.extend_from_slice(&frame);
    Ok(archive)
}

pub(super) fn decompress(archive: &[u8]) -> io::Result<Vec<u8>> {
    let invalid = |message: &str| io::Error::new(io::ErrorKind::InvalidData, message.to_owned());
    let Some((&FORMAT, frame)) = archive.split_first() else {
        return Err(invalid("unknown JSONL archive format"));
    };
    let size = zstd::zstd_safe::get_frame_content_size(frame)
        .ok()
        .flatten()
        .ok_or_else(|| invalid("JSONL archive has no content size"))?;
    let capacity = usize::try_from(size).map_err(|_| invalid("JSONL archive is too large"))?;
    zstd::bulk::Decompressor::with_dictionary(DICTIONARY)?.decompress(frame, capacity)
}

pub(super) fn archive(path: &Path) -> io::Result<Option<[u8; 32]>> {
    let before = fs::metadata(path)?;
    let bytes = fs::read(path)?;
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let target = archived(path);
    let archive = compress(&bytes)?;
    write(directory, &target, &archive, true)?;
    let after = fs::metadata(path)?;
    if !unchanged(&before, &after) || after.len() != bytes.len() as u64 {
        fs::remove_file(&target)?;
        return Ok(None);
    }
    fs::remove_file(path)?;
    sync_directory(directory)?;
    Ok(Some(Sha256::digest(&archive).into()))
}

pub(super) fn restore(path: &Path) -> io::Result<()> {
    let source = archived(path);
    if path.exists() || !source.is_file() {
        return Ok(());
    }
    let archive = match fs::read(&source) {
        Ok(archive) => archive,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    write(directory, path, &decompress(&archive)?, false)?;
    match fs::remove_file(&source) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    sync_directory(directory)
}

fn write(directory: &Path, path: &Path, bytes: &[u8], replace: bool) -> io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(bytes)?;
    sync_journal(file.as_file())?;
    let persisted = if replace {
        file.persist(path).map(drop)
    } else {
        file.persist_noclobber(path).map(drop)
    };
    match persisted {
        Ok(()) => Ok(()),
        Err(error) if !replace && error.error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.error),
    }
}

#[cfg(unix)]
fn unchanged(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    before.ino() == after.ino()
        && before.dev() == after.dev()
        && before.size() == after.size()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.mtime() == after.mtime()
}

#[cfg(not(unix))]
fn unchanged(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.len() == after.len() && before.modified().ok() == after.modified().ok()
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> io::Result<()> {
    sync_journal(&fs::File::open(directory)?)
}

#[cfg(not(unix))]
fn sync_directory(_: &Path) -> io::Result<()> {
    Ok(())
}
