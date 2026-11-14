use std::{io::Read, sync::Arc};

use nanocode_process_contracts::LastActivity;
use nanocode_sync::{Actor, Stopped};

use tokio::io::{AsyncRead, AsyncReadExt};

use super::process::{Exit, Ready};

pub(super) const READ_BUFFER_LENGTH: usize = 8 * 1024;
pub(super) use nanocode_process_contracts::{effective_token_limit, redact_and_limit_in};

#[derive(Clone)]
pub(super) struct Capture {
    owner: Actor<super::CapturedOutput>,
    limit: usize,
}

impl Capture {
    pub(super) fn new(limit: usize) -> std::io::Result<Self> {
        Ok(Self {
            owner: Actor::spawn(super::CapturedOutput::default())?,
            limit,
        })
    }

    pub(super) async fn push(&self, bytes: Vec<u8>) -> Result<(), Stopped> {
        let limit = self.limit;
        self.owner
            .run(move |captured| Box::pin(async move { captured.push(&bytes, limit) }))
            .await
    }

    pub(super) async fn take(&self) -> Result<super::CapturedChunk, Stopped> {
        self.owner
            .run(|captured| Box::pin(async move { captured.take() }))
            .await
    }
}

#[derive(Clone)]
pub(super) enum Sender {
    Captured(Capture),
}

impl Sender {
    pub(super) async fn send(&self, bytes: Vec<u8>) -> Result<(), ()> {
        match self {
            Self::Captured(capture) => capture.push(bytes).await.map_err(|_| ()),
        }
    }
}

#[cfg(unix)]
static OPEN: std::sync::LazyLock<nanocode_sync::Atom<Vec<Arc<std::os::fd::OwnedFd>>>> =
    std::sync::LazyLock::new(nanocode_sync::Atom::default);

pub(super) struct Open(#[cfg(unix)] Option<Arc<std::os::fd::OwnedFd>>);

impl Open {
    #[cfg(unix)]
    pub(super) fn new(pipe: &impl std::os::fd::AsFd) -> Self {
        let output = pipe.as_fd().try_clone_to_owned().ok().map(Arc::new);
        if let Some(output) = &output {
            OPEN.update(|open| open.push(Arc::clone(output)));
        }
        Self(output)
    }

    #[cfg(not(unix))]
    pub(super) const fn new<P>(_: &P) -> Self {
        Self()
    }

    pub(super) const fn none() -> Self {
        Self(
            #[cfg(unix)]
            None,
        )
    }
}

#[cfg(unix)]
impl Drop for Open {
    fn drop(&mut self) {
        if let Some(output) = self.0.take() {
            OPEN.update(|open| open.retain(|other| !Arc::ptr_eq(other, &output)));
        }
    }
}

#[cfg(unix)]
pub(super) fn open() -> Vec<Arc<std::os::fd::OwnedFd>> {
    OPEN.read(Vec::clone)
}

pub(super) async fn discard(mut pipe: impl AsyncRead + Unpin, _open: Open) {
    let _ = tokio::io::copy(&mut pipe, &mut tokio::io::sink()).await;
}

pub(super) async fn drain<P: AsyncRead + Ready + Unpin + Send + 'static>(
    mut pipe: Option<P>,
    chunks: Sender,
    activity: Arc<LastActivity>,
    exited: Arc<Exit>,
) {
    let Some(mut pipe) = pipe.take() else {
        return;
    };
    let open = pipe.open();
    let mut buffer = [0_u8; READ_BUFFER_LENGTH];
    loop {
        let read = nanocode_sync::select! {
            () = exited.wait() => break,
            read = pipe.read(&mut buffer) => read,
        };
        match read {
            Ok(0) => return,
            Ok(length) => {
                activity.touch();
                if chunks.send(buffer[..length].to_vec()).await.is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = chunks
                    .send(format!("failed to read command output: {error}").into_bytes())
                    .await;
                return;
            }
        }
    }
    loop {
        match pipe.ready(&mut buffer) {
            Ok(Some(0)) | Err(_) => return,
            Ok(Some(length)) => {
                activity.touch();
                if chunks.send(buffer[..length].to_vec()).await.is_err() {
                    return;
                }
            }
            Ok(None) => {
                drop(nanocode_platform::spawn_task(discard(pipe, open)));
                return;
            }
        }
    }
}

pub(super) fn drain_blocking(
    mut pipe: std::fs::File,
    chunks: Sender,
    activity: Arc<LastActivity>,
) -> tokio::task::JoinHandle<()> {
    let runtime = tokio::runtime::Handle::current();
    nanocode_platform::spawn_blocking(move || {
        let _open = Open::new(&pipe);
        let mut buffer = [0_u8; READ_BUFFER_LENGTH];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => return,
                Ok(length) => {
                    activity.touch();
                    if runtime
                        .block_on(chunks.send(buffer[..length].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}

                Err(_) => return,
            }
        }
    })
}
