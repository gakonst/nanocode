use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, OnceLock},
    task::{Context, Poll},
};

use tokio::{
    io::{AsyncRead, ReadBuf},
    sync::{mpsc, oneshot},
};

use super::process;

pub type Sink = Box<dyn FnMut(&[u8]) -> io::Result<()> + Send>;

pub struct Contained {
    pub script: String,
    pub workdir: PathBuf,
    pub environment: Vec<(OsString, OsString)>,
    pub stdout: Sink,
    pub stderr: Sink,
    pub ended: Box<dyn FnOnce(io::Result<i32>) + Send>,
}

pub trait Running: Send + Sync {
    fn interrupt(&self);
    fn stop(&self);
}

pub trait Embedded: Send + Sync {
    fn contains(&self, script: &str, environment: &[(OsString, OsString)]) -> bool;
    fn start(&self, run: Contained) -> io::Result<Arc<dyn Running>>;
}

static EMBEDDED: OnceLock<Arc<dyn Embedded>> = OnceLock::new();

pub fn embed(shell: Arc<dyn Embedded>) -> Result<(), Arc<dyn Embedded>> {
    EMBEDDED.set(shell)
}

pub(crate) struct Reader {
    chunks: mpsc::Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    read: usize,
}

impl AsyncRead for Reader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        while self.read == self.chunk.len() {
            match self.chunks.poll_recv(cx) {
                Poll::Ready(Some(chunk)) => (self.chunk, self.read) = (chunk, 0),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
        let end = self.chunk.len().min(self.read + buffer.remaining());
        buffer.put_slice(&self.chunk[self.read..end]);
        self.read = end;
        Poll::Ready(Ok(()))
    }
}

impl process::Ready for Reader {
    fn ready(&mut self, buffer: &mut [u8]) -> io::Result<Option<usize>> {
        while self.read == self.chunk.len() {
            match self.chunks.try_recv() {
                Ok(chunk) => (self.chunk, self.read) = (chunk, 0),
                Err(mpsc::error::TryRecvError::Empty) => return Ok(None),
                Err(mpsc::error::TryRecvError::Disconnected) => return Ok(Some(0)),
            }
        }
        let end = self.chunk.len().min(self.read + buffer.len());
        let read = end - self.read;
        buffer[..read].copy_from_slice(&self.chunk[self.read..end]);
        self.read = end;
        Ok(Some(read))
    }

    fn open(&self) -> super::output::Open {
        super::output::Open::none()
    }
}

fn sink(written: mpsc::Sender<Vec<u8>>) -> Sink {
    Box::new(move |bytes: &[u8]| {
        written
            .blocking_send(bytes.to_vec())
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    })
}

fn stream() -> (mpsc::Sender<Vec<u8>>, Reader) {
    let (written, chunks) = mpsc::channel::<Vec<u8>>(1);
    let reader = Reader {
        chunks,
        chunk: Vec::new(),
        read: 0,
    };
    (written, reader)
}

pub(crate) enum Streams {
    Apart,
    InOrder,
}

pub(crate) fn spawn(
    script: &str,
    workdir: &Path,
    environment: &[(OsString, OsString)],
    streams: Streams,
) -> Option<process::SpawnedProcess> {
    let embedded = EMBEDDED.get()?;
    if !embedded.contains(script, environment) {
        return None;
    }
    let (out, err, output) = match streams {
        Streams::Apart => {
            let (out, stdout) = stream();
            let (err, stderr) = stream();
            let output = process::ProcessOutput::Streams { stdout, stderr };
            (sink(out), sink(err), output)
        }
        Streams::InOrder => {
            let (written, both) = stream();
            let output = process::ProcessOutput::Stream(both);
            (sink(written.clone()), sink(written), output)
        }
    };
    let (exit, exited) = oneshot::channel();
    let started = embedded.start(Contained {
        script: script.to_owned(),
        workdir: workdir.to_owned(),
        environment: environment.to_vec(),
        stdout: out,
        stderr: err,
        ended: Box::new(move |status| drop(exit.send(status))),
    });
    let running = started
        .inspect_err(|error| nanocode_trace::warn!(%error, "the shell did not start in this process"))
        .ok()?;
    let child =
        process::ProcessChild::served(process::ProcessGroupGuard::embedded(running), exited);
    Some(process::SpawnedProcess {
        process_group: child.process_group(),
        child,
        stdin: None,
        output,
    })
}
