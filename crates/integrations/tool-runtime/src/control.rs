//! A local control socket: a Unix socket only its owner may use, on which a
//! client writes one request per line and reads one answer per line, as a
//! desktop app asks a hand what it is doing. Bounded in conversations, line
//! length, idle time and failed accepts.

use std::{
    fs,
    future::Future,
    io::{self, BufRead, Read, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::mpsc,
    time::{sleep, timeout},
};

/// How much a control socket serves at once.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Conversations at once; the next connection is accepted once one ends.
    pub conversations: usize,
    /// The longest request line, in bytes; a longer one ends its conversation.
    pub line: u64,
    /// How long a conversation may wait for its next line.
    pub idle: Duration,
    /// Failed accepts in a row after which [`serve`] returns.
    pub failures: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            conversations: 16,
            line: 64 * 1024,
            idle: Duration::from_secs(60),
            failures: 1000,
        }
    }
}

/// Serves `path`, replacing what was there, as a 0600 socket: each request
/// line gets `answer`'s line back. Fails only when it cannot bind; `failed`
/// hears each failed accept, and it returns after `limits.failures` in a row.
pub async fn serve<A, F>(
    path: &Path,
    limits: Limits,
    answer: A,
    mut failed: impl FnMut(&io::Error),
) -> io::Result<()>
where
    A: Fn(String) -> F + Clone + Send + 'static,
    F: Future<Output = String> + Send,
{
    let _ = fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    let (ended, mut ends) = mpsc::unbounded_channel();
    let (mut active, mut failures) = (0, 0);
    while failures < limits.failures {
        if active >= limits.conversations {
            ends.recv().await;
            active -= 1;
            continue;
        }
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            Some(()) = ends.recv() => {
                active -= 1;
                continue;
            }
        };
        match accepted {
            Ok((stream, _)) => {
                failures = 0;
                active += 1;
                let (answer, ended) = (answer.clone(), Ended(ended.clone()));
                tokio::spawn(async move {
                    converse(stream, limits, answer).await;
                    drop(ended);
                });
            }
            Err(error) => {
                failures += 1;
                failed(&error);
                sleep(Duration::from_millis(100)).await;
            }
        }
    }
    Ok(())
}

/// Tells the accepting task a conversation ended, however it ended.
struct Ended(mpsc::UnboundedSender<()>);

impl Drop for Ended {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

async fn converse<A, F>(stream: UnixStream, limits: Limits, answer: A)
where
    A: Fn(String) -> F,
    F: Future<Output = String>,
{
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    while let Ok(Some(line)) = timeout(limits.idle, read_line(&mut reader, limits.line)).await {
        let reply = format!("{}\n", answer(line).await);
        if writer.write_all(reply.as_bytes()).await.is_err() {
            return;
        }
    }
}

/// One line of at most `max` bytes, without its line ending; `None` at the
/// end of the stream, for a longer line, or one that is not UTF-8.
pub async fn read_line(reader: &mut (impl AsyncBufRead + Unpin), max: u64) -> Option<String> {
    let mut line = Vec::new();
    let read = (&mut *reader)
        .take(max)
        .read_until(b'\n', &mut line)
        .await
        .ok()?;
    if read == 0 || (line.last() != Some(&b'\n') && read as u64 >= max) {
        return None;
    }
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    String::from_utf8(line).ok()
}

/// Asks the socket at `path` one request and returns its answer line, or
/// `None` when the answer did not end its line within `max` bytes.
pub fn ask(
    path: &Path,
    request: &str,
    write: Duration,
    read: Duration,
    max: u64,
) -> io::Result<Option<String>> {
    let mut socket = std::os::unix::net::UnixStream::connect(path)?;
    socket.set_read_timeout(Some(read))?;
    socket.set_write_timeout(Some(write))?;
    writeln!(socket, "{request}")?;
    let mut line = String::new();
    std::io::BufReader::new(socket)
        .take(max)
        .read_line(&mut line)?;
    Ok(line.ends_with('\n').then_some(line))
}
