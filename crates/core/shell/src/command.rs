use psh_engine::HostFile;
use std::{
    future::{Future, poll_fn},
    io,
    pin::Pin,
    rc::Rc,
};

pub trait Commands {
    fn has(&self, name: &[u8]) -> bool;
    fn run(&self, call: Call) -> Pin<Box<dyn Future<Output = i32>>>;
}

pub struct NoCommands;

impl Commands for NoCommands {
    fn has(&self, _name: &[u8]) -> bool {
        false
    }

    fn run(&self, _call: Call) -> Pin<Box<dyn Future<Output = i32>>> {
        Box::pin(std::future::ready(127))
    }
}

pub struct Call {
    pub argv: Vec<Vec<u8>>,
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
    pub cwd: Vec<u8>,
    pub stdin: Stream,
    pub stdout: Stream,
    pub stderr: Stream,
}

#[derive(Clone)]
pub struct Stream(pub(crate) Rc<dyn HostFile>);

impl Stream {
    pub async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        poll_fn(|cx| self.0.poll_read(cx, buf)).await
    }

    pub async fn read_to_end(&self, out: &mut Vec<u8>) -> io::Result<()> {
        let mut chunk = vec![0; crate::file::PIPE_BYTES];
        loop {
            let read = self.read(&mut chunk).await?;
            if read == 0 {
                return Ok(());
            }
            out.extend_from_slice(&chunk[..read]);
        }
    }

    pub async fn write_all(&self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let wrote = poll_fn(|cx| self.0.poll_write(cx, data)).await?;
            if wrote == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            data = &data[wrote..];
        }
        Ok(())
    }
}
