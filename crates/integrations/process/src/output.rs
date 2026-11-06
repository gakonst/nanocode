use std::{io::Read, sync::Arc};

use nanocode_process_contracts::LastActivity;
use nanocode_sync::{Actor, Stopped};

use tokio::io::{AsyncRead, AsyncReadExt};

pub(super) const READ_BUFFER_LENGTH: usize = 8 * 1024;
pub(super) use nanocode_process_contracts::{effective_token_limit, redact_and_limit};

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
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    Unbounded(tokio::sync::mpsc::UnboundedSender<Vec<u8>>),
}

impl Sender {
    pub(super) async fn send(&self, bytes: Vec<u8>) -> Result<(), ()> {
        match self {
            Self::Captured(capture) => capture.push(bytes).await.map_err(|_| ()),
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            Self::Unbounded(sender) => sender.send(bytes).map_err(|_| ()),
        }
    }
}

pub(super) async fn drain(
    mut pipe: Option<impl AsyncRead + Unpin>,
    chunks: Sender,
    activity: Arc<LastActivity>,
) {
    let Some(mut pipe) = pipe.take() else {
        return;
    };
    let mut buffer = [0_u8; READ_BUFFER_LENGTH];
    loop {
        match pipe.read(&mut buffer).await {
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
}

pub(super) fn drain_blocking(
    mut pipe: Box<dyn Read + Send>,
    chunks: Sender,
    activity: Arc<LastActivity>,
) -> tokio::task::JoinHandle<()> {
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
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
