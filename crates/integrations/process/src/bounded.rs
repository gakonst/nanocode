use nanocode_executor::process::Command;
use nanocode_io::{AsyncReadExt as _, AsyncWriteExt as _};
use std::{io, process::Stdio, time::Duration};

pub enum Input<'a> {
    Bytes(&'a [u8]),
    File(std::fs::File),
}

pub struct Bounded {
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub cut: bool,
}

pub async fn bounded(
    command: &mut Command,
    stdin: Option<Input<'_>>,
    limit: usize,
    timeout: Duration,
) -> io::Result<Option<Bounded>> {
    let (input, bytes) = match stdin {
        None => (Stdio::null(), None),
        Some(Input::Bytes(bytes)) => (Stdio::piped(), Some(bytes)),
        Some(Input::File(file)) => (Stdio::from(file), None),
    };
    let mut child = command
        .stdin(input)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()?;
    let group = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .filter(|id| *id != 0);
    let pipe = child.stdin.take();
    let mut output = child.stdout.take();
    let work = async {
        let write = async {
            if let (Some(mut pipe), Some(bytes)) = (pipe, bytes) {
                drop(pipe.write_all(bytes).await);
                drop(pipe.shutdown().await);
            }
        };
        let read = async {
            let (mut kept, mut cut) = (Vec::new(), false);
            let mut chunk = vec![0; 64 * 1024];
            if let Some(output) = output.as_mut() {
                loop {
                    let read = output.read(&mut chunk).await?;
                    if read == 0 {
                        break;
                    }
                    let room = limit.saturating_sub(kept.len());
                    cut |= read > room;
                    kept.extend_from_slice(&chunk[..read.min(room)]);
                }
            }
            io::Result::Ok((kept, cut))
        };
        let ((), read) = nanocode_sync::join!(write, read);
        let status = child.wait().await?;
        let (stdout, cut) = read?;
        io::Result::Ok(Bounded {
            status,
            stdout,
            cut,
        })
    };
    let finished = nanocode_platform::timeout(timeout, work).await;
    if finished.is_err()
        && let Some(group) = group
    {
        nanocode_platform::unix::signal_group(group, libc::SIGKILL).ok();
    }
    finished.map_or(Ok(None), |bounded| bounded.map(Some))
}
