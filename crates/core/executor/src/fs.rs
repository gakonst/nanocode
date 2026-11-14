use std::{
    fs::{Metadata, Permissions},
    future::Future,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll, ready},
};

use nanocode_io::{AsyncRead, AsyncSeek, AsyncWrite, ReadBuf};

use crate::{blocking, task::JoinHandle};

const MAX_BUFFER: usize = 2 * 1024 * 1024;

fn background_failed() -> io::Error {
    io::Error::other("background task failed")
}

pub(crate) async fn asyncify<F, T>(work: F) -> io::Result<T>
where
    F: FnOnce() -> io::Result<T> + Send + 'static,
    T: Send + 'static,
{
    match blocking::spawn_anywhere(work).await {
        Ok(result) => result,
        Err(_) => Err(background_failed()),
    }
}

pub async fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref().to_owned();
    asyncify(move || std::fs::read(path)).await
}

pub async fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    let path = path.as_ref().to_owned();
    asyncify(move || std::fs::read_to_string(path)).await
}

pub async fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    let contents = contents.as_ref().to_owned();
    asyncify(move || std::fs::write(path, contents)).await
}

pub async fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    asyncify(move || std::fs::remove_file(path)).await
}

pub async fn remove_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    asyncify(move || std::fs::remove_dir(path)).await
}

pub async fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    asyncify(move || std::fs::remove_dir_all(path)).await
}

pub async fn create_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    asyncify(move || std::fs::create_dir(path)).await
}

pub async fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    asyncify(move || std::fs::create_dir_all(path)).await
}

pub async fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref().to_owned();
    asyncify(move || std::fs::canonicalize(path)).await
}

pub async fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    let path = path.as_ref().to_owned();
    asyncify(move || std::fs::metadata(path)).await
}

pub async fn symlink_metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    let path = path.as_ref().to_owned();
    asyncify(move || std::fs::symlink_metadata(path)).await
}

pub async fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    let from = from.as_ref().to_owned();
    let to = to.as_ref().to_owned();
    asyncify(move || std::fs::rename(from, to)).await
}

pub async fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
    let from = from.as_ref().to_owned();
    let to = to.as_ref().to_owned();
    asyncify(move || std::fs::copy(from, to)).await
}

pub async fn try_exists(path: impl AsRef<Path>) -> io::Result<bool> {
    let path = path.as_ref().to_owned();
    asyncify(move || path.try_exists()).await
}

pub async fn set_permissions(path: impl AsRef<Path>, permissions: Permissions) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    asyncify(move || std::fs::set_permissions(path, permissions)).await
}

pub async fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDir> {
    let path = path.as_ref().to_owned();
    let entries =
        asyncify(move || std::fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()).await?;
    Ok(ReadDir {
        entries: entries.into_iter(),
    })
}

pub struct ReadDir {
    entries: std::vec::IntoIter<std::fs::DirEntry>,
}

impl ReadDir {
    #[allow(clippy::unused_async)]
    pub async fn next_entry(&mut self) -> io::Result<Option<DirEntry>> {
        Ok(self.entries.next().map(DirEntry))
    }
}

pub struct DirEntry(std::fs::DirEntry);

impl DirEntry {
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.0.path()
    }

    #[must_use]
    pub fn file_name(&self) -> std::ffi::OsString {
        self.0.file_name()
    }

    pub async fn metadata(&self) -> io::Result<Metadata> {
        let path = self.0.path();
        asyncify(move || std::fs::symlink_metadata(path)).await
    }

    pub async fn file_type(&self) -> io::Result<std::fs::FileType> {
        let path = self.0.path();
        asyncify(move || std::fs::symlink_metadata(path).map(|metadata| metadata.file_type())).await
    }
}

#[derive(Debug, Default)]
pub struct DirBuilder {
    recursive: bool,
    #[cfg(unix)]
    mode: Option<u32>,
}

impl DirBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn recursive(&mut self, recursive: bool) -> &mut Self {
        self.recursive = recursive;
        self
    }

    #[cfg(unix)]
    pub const fn mode(&mut self, mode: u32) -> &mut Self {
        self.mode = Some(mode);
        self
    }

    pub async fn create(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref().to_owned();
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(self.recursive);
        #[cfg(unix)]
        if let Some(mode) = self.mode {
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, mode);
        }
        asyncify(move || builder.create(path)).await
    }
}

#[derive(Clone, Debug)]
pub struct OpenOptions(std::fs::OpenOptions);

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenOptions {
    #[must_use]
    pub fn new() -> Self {
        Self(std::fs::OpenOptions::new())
    }

    pub fn read(&mut self, read: bool) -> &mut Self {
        self.0.read(read);
        self
    }

    pub fn write(&mut self, write: bool) -> &mut Self {
        self.0.write(write);
        self
    }

    pub fn append(&mut self, append: bool) -> &mut Self {
        self.0.append(append);
        self
    }

    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.0.truncate(truncate);
        self
    }

    pub fn create(&mut self, create: bool) -> &mut Self {
        self.0.create(create);
        self
    }

    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.0.create_new(create_new);
        self
    }

    #[cfg(unix)]
    pub fn mode(&mut self, mode: u32) -> &mut Self {
        std::os::unix::fs::OpenOptionsExt::mode(&mut self.0, mode);
        self
    }

    #[cfg(unix)]
    pub fn custom_flags(&mut self, flags: i32) -> &mut Self {
        std::os::unix::fs::OpenOptionsExt::custom_flags(&mut self.0, flags);
        self
    }

    pub async fn open(&self, path: impl AsRef<Path>) -> io::Result<File> {
        let path = path.as_ref().to_owned();
        let options = self.0.clone();
        let file = asyncify(move || options.open(path)).await?;
        Ok(File::from_std(file))
    }
}

type Done<T> = (T, io::Result<Outcome>);

enum Outcome {
    Read(Vec<u8>),
    Wrote(usize),
    Sought(u64),
    Flushed,
}

enum State<T> {
    Idle(Option<T>),
    Busy(JoinHandle<Done<T>>),
}

pub struct Blocking<T: Send + 'static> {
    state: State<T>,
    buffered: Vec<u8>,
    position: usize,
    seek: Option<SeekFrom>,
    sought: Option<io::Result<u64>>,
}

impl<T: Send + 'static> Unpin for Blocking<T> {}

impl<T: Send + 'static> Blocking<T> {
    const fn new(inner: T) -> Self {
        Self {
            state: State::Idle(Some(inner)),
            buffered: Vec::new(),
            position: 0,
            seek: None,
            sought: None,
        }
    }

    fn poll_settled(&mut self, context: &mut Context<'_>) -> Poll<io::Result<Option<Outcome>>> {
        match &mut self.state {
            State::Idle(_) => Poll::Ready(Ok(None)),
            State::Busy(handle) => {
                let joined = ready!(Pin::new(handle).poll(context));
                let (inner, outcome) = joined.map_err(|_| background_failed())?;
                self.state = State::Idle(Some(inner));
                Poll::Ready(outcome.map(Some))
            }
        }
    }

    fn start(&mut self, work: impl FnOnce(&mut T) -> io::Result<Outcome> + Send + 'static) {
        let State::Idle(inner) = &mut self.state else {
            return;
        };
        let Some(mut inner) = inner.take() else {
            return;
        };
        self.state = State::Busy(blocking::spawn_anywhere(move || {
            let outcome = work(&mut inner);
            (inner, outcome)
        }));
    }

    async fn with<R: Send + 'static>(
        &mut self,
        work: impl FnOnce(&mut T) -> io::Result<R> + Send + 'static,
    ) -> io::Result<R> {
        std::future::poll_fn(|context| self.poll_settled(context)).await?;
        let State::Idle(inner) = &mut self.state else {
            return Err(background_failed());
        };
        let mut owned = inner.take().ok_or_else(background_failed)?;
        let (owned, result) = match blocking::spawn_anywhere(move || {
            let result = work(&mut owned);
            (owned, result)
        })
        .await
        {
            Ok(done) => done,
            Err(_) => return Err(background_failed()),
        };
        self.state = State::Idle(Some(owned));
        result
    }
}

impl<T: Read + Send + 'static> AsyncRead for Blocking<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if self.position < self.buffered.len() {
                let available = &self.buffered[self.position..];
                let count = available.len().min(buffer.remaining());
                buffer.put_slice(&available[..count]);
                self.position += count;
                return Poll::Ready(Ok(()));
            }
            if let Some(outcome) = ready!(self.poll_settled(context))? {
                if let Outcome::Read(bytes) = outcome {
                    self.buffered = bytes;
                    self.position = 0;
                    if self.buffered.is_empty() {
                        return Poll::Ready(Ok(()));
                    }
                }
                continue;
            }
            let wanted = buffer.remaining().min(MAX_BUFFER);
            self.start(move |inner| {
                let mut bytes = vec![0; wanted];
                let read = loop {
                    match inner.read(&mut bytes) {
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        other => break other?,
                    }
                };
                bytes.truncate(read);
                Ok(Outcome::Read(bytes))
            });
        }
    }
}

impl<T: Write + Send + 'static> AsyncWrite for Blocking<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            if let Some(outcome) = ready!(self.poll_settled(context))? {
                if let Outcome::Wrote(written) = outcome {
                    return Poll::Ready(Ok(written));
                }
                continue;
            }
            let bytes = buffer[..buffer.len().min(MAX_BUFFER)].to_vec();
            self.start(move |inner| {
                inner.write_all(&bytes)?;
                Ok(Outcome::Wrote(bytes.len()))
            });
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            if let Some(outcome) = ready!(self.poll_settled(context))? {
                if matches!(outcome, Outcome::Flushed) {
                    return Poll::Ready(Ok(()));
                }
                continue;
            }
            self.start(|inner| inner.flush().map(|()| Outcome::Flushed));
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(context)
    }
}

impl<T: Seek + Send + 'static> AsyncSeek for Blocking<T> {
    fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        if self.seek.is_some() {
            return Err(io::Error::other(
                "other file operation is pending, call poll_complete before start_seek",
            ));
        }
        let position = match position {
            SeekFrom::Current(offset) => {
                let unread = i64::try_from(self.buffered.len() - self.position)
                    .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
                SeekFrom::Current(offset - unread)
            }
            other => other,
        };
        self.buffered.clear();
        self.position = 0;
        self.seek = Some(position);
        Ok(())
    }

    fn poll_complete(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<u64>> {
        loop {
            if let Some(outcome) = ready!(self.poll_settled(context))? {
                if let Outcome::Sought(position) = outcome {
                    self.sought = Some(Ok(position));
                }
                continue;
            }
            if let Some(result) = self.sought.take() {
                return Poll::Ready(result);
            }
            let Some(position) = self.seek.take() else {
                return Poll::Ready(Err(io::Error::other("no seek is pending")));
            };
            self.start(move |inner| inner.seek(position).map(Outcome::Sought));
        }
    }
}

pub struct File {
    blocking: Blocking<std::fs::File>,
}

impl std::fmt::Debug for File {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("File")
    }
}

impl File {
    pub async fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_owned();
        asyncify(move || std::fs::File::open(path))
            .await
            .map(Self::from_std)
    }

    pub async fn create(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_owned();
        asyncify(move || std::fs::File::create(path))
            .await
            .map(Self::from_std)
    }

    #[must_use]
    pub const fn from_std(file: std::fs::File) -> Self {
        Self {
            blocking: Blocking::new(file),
        }
    }

    #[must_use]
    pub fn options() -> OpenOptions {
        OpenOptions::new()
    }

    pub async fn metadata(&mut self) -> io::Result<Metadata> {
        self.blocking.with(|file| file.metadata()).await
    }

    pub async fn sync_all(&mut self) -> io::Result<()> {
        std::future::poll_fn(|context| Pin::new(&mut self.blocking).poll_flush(context)).await?;
        self.blocking.with(|file| file.sync_all()).await
    }

    pub async fn sync_data(&mut self) -> io::Result<()> {
        std::future::poll_fn(|context| Pin::new(&mut self.blocking).poll_flush(context)).await?;
        self.blocking.with(|file| file.sync_data()).await
    }

    pub async fn set_len(&mut self, size: u64) -> io::Result<()> {
        self.blocking.with(move |file| file.set_len(size)).await
    }

    pub async fn into_std(mut self) -> std::fs::File {
        let _ = std::future::poll_fn(|context| self.blocking.poll_settled(context)).await;
        match std::mem::replace(&mut self.blocking.state, State::Idle(None)) {
            State::Idle(Some(file)) => file,
            _ => unreachable!("a settled file is idle and holds its descriptor"),
        }
    }
}

impl AsyncRead for File {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.blocking).poll_read(context, buffer)
    }
}

impl AsyncWrite for File {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.blocking).poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.blocking).poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.blocking).poll_shutdown(context)
    }
}

impl AsyncSeek for File {
    fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        Pin::new(&mut self.blocking).start_seek(position)
    }

    fn poll_complete(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Pin::new(&mut self.blocking).poll_complete(context)
    }
}

pub type Stdin = Blocking<std::io::Stdin>;
pub type Stdout = Blocking<std::io::Stdout>;
pub type Stderr = Blocking<std::io::Stderr>;

#[must_use]
pub fn stdin() -> Stdin {
    Blocking::new(std::io::stdin())
}

#[must_use]
pub fn stdout() -> Stdout {
    Blocking::new(std::io::stdout())
}

#[must_use]
pub fn stderr() -> Stderr {
    Blocking::new(std::io::stderr())
}
