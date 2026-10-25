use std::{io::Read, sync::Arc};

use nanocode_process_contracts::LastActivity;

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::mpsc::UnboundedSender,
};

const READ_BUFFER_LENGTH: usize = 8 * 1024;
pub(super) use nanocode_process_contracts::{effective_token_limit, redact_and_limit};

pub(super) async fn drain(
    mut pipe: Option<impl AsyncRead + Unpin>,
    chunks: UnboundedSender<Vec<u8>>,
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
                if chunks.send(buffer[..length].to_vec()).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = chunks.send(format!("failed to read command output: {error}").into_bytes());
                return;
            }
        }
    }
}

pub(super) fn drain_blocking(
    mut pipe: Box<dyn Read + Send>,
    chunks: UnboundedSender<Vec<u8>>,
    activity: Arc<LastActivity>,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let mut buffer = [0_u8; READ_BUFFER_LENGTH];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => return,
                Ok(length) => {
                    activity.touch();
                    if chunks.send(buffer[..length].to_vec()).is_err() {
                        return;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}

                Err(_) => return,
            }
        }
    })
}
