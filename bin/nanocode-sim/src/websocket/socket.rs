use std::{
    cell::RefCell,
    collections::VecDeque,
    future::poll_fn,
    io::{self, IoSlice},
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, Waker},
};

use nanocode_io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    Open,
    Eof,
    Reset,
}

struct Pipe {
    queue: VecDeque<u8>,
    capacity_bytes: Option<usize>,
    end: End,
    reader_gone: bool,
    reader: Option<Waker>,
    writer: Option<Waker>,
}

impl Pipe {
    const fn new(capacity_bytes: Option<usize>) -> Self {
        Self {
            queue: VecDeque::new(),
            capacity_bytes,
            end: End::Open,
            reader_gone: false,
            reader: None,
            writer: None,
        }
    }

    fn wake(&mut self) {
        if let Some(reader) = self.reader.take() {
            reader.wake();
        }
        if let Some(writer) = self.writer.take() {
            writer.wake();
        }
    }

    fn poll_read(
        &mut self,
        context: &Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.queue.is_empty() {
            let count = self.queue.len().min(buffer.remaining());
            let (front, back) = self.queue.as_slices();
            let from_front = front.len().min(count);
            buffer.put_slice(&front[..from_front]);
            buffer.put_slice(&back[..count - from_front]);
            self.queue.drain(..count);
            self.wake();
            return Poll::Ready(Ok(()));
        }
        match self.end {
            End::Eof => Poll::Ready(Ok(())),
            End::Reset => Poll::Ready(Err(io::ErrorKind::ConnectionReset.into())),
            End::Open => {
                self.reader = Some(context.waker().clone());
                Poll::Pending
            }
        }
    }

    fn poll_write(&mut self, context: &Context<'_>, buffers: &[&[u8]]) -> Poll<io::Result<usize>> {
        match self.end {
            End::Open => {}
            End::Reset => return Poll::Ready(Err(io::ErrorKind::ConnectionReset.into())),
            End::Eof => panic!("the simulated socket writes after its own end"),
        }
        let total: usize = buffers.iter().map(|buffer| buffer.len()).sum();
        if self.reader_gone {
            return Poll::Ready(Ok(total));
        }
        let room = self
            .capacity_bytes
            .map_or(total, |capacity| capacity - self.queue.len());
        if room == 0 && total > 0 {
            self.writer = Some(context.waker().clone());
            return Poll::Pending;
        }
        let mut written = 0;
        for buffer in buffers {
            let take = buffer.len().min(room - written);
            self.queue.extend(&buffer[..take]);
            written += take;
        }
        self.wake();
        Poll::Ready(Ok(written))
    }
}

pub struct Wire {
    to_client: RefCell<Pipe>,
    to_server: RefCell<Pipe>,
}

impl Wire {
    pub const fn new(client_capacity_bytes: Option<usize>) -> Self {
        Self {
            to_client: RefCell::new(Pipe::new(None)),
            to_server: RefCell::new(Pipe::new(client_capacity_bytes)),
        }
    }

    pub fn server_write(&self, bytes: &[u8]) {
        let mut pipe = self.to_client.borrow_mut();
        if pipe.end != End::Open {
            return;
        }
        if !pipe.reader_gone {
            pipe.queue.extend(bytes);
        }
        pipe.wake();
    }

    pub fn server_eof(&self) {
        let mut pipe = self.to_client.borrow_mut();
        if pipe.end == End::Open {
            pipe.end = End::Eof;
        }
        pipe.wake();
    }

    pub fn reset(&self) {
        for pipe in [&self.to_client, &self.to_server] {
            let mut pipe = pipe.borrow_mut();
            pipe.end = End::Reset;
            pipe.wake();
        }
    }

    pub async fn server_read(&self, bytes_max: usize) -> io::Result<Vec<u8>> {
        assert!(bytes_max > 0, "a read asks for at least one byte");
        let mut chunk = vec![0; bytes_max];
        let filled = poll_fn(|context| {
            let mut buffer = ReadBuf::new(&mut chunk);
            match self.to_server.borrow_mut().poll_read(context, &mut buffer) {
                Poll::Ready(Ok(())) => Poll::Ready(Ok(buffer.filled().len())),
                Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
                Poll::Pending => Poll::Pending,
            }
        })
        .await?;
        chunk.truncate(filled);
        Ok(chunk)
    }
}

pub struct ClientSide {
    wire: Rc<Wire>,
    vectored: bool,
}

impl ClientSide {
    pub const fn new(wire: Rc<Wire>, vectored: bool) -> Self {
        Self { wire, vectored }
    }
}

impl Drop for ClientSide {
    fn drop(&mut self) {
        let mut to_server = self.wire.to_server.borrow_mut();
        if to_server.end == End::Open {
            to_server.end = End::Eof;
        }
        to_server.wake();
        drop(to_server);
        let mut to_client = self.wire.to_client.borrow_mut();
        to_client.reader_gone = true;
        to_client.queue.clear();
        to_client.wake();
    }
}

impl AsyncRead for ClientSide {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.wire.to_client.borrow_mut().poll_read(context, buffer)
    }
}

impl AsyncWrite for ClientSide {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.wire
            .to_server
            .borrow_mut()
            .poll_write(context, &[buffer])
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffers: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let slices: Vec<&[u8]> = buffers.iter().map(|buffer| &**buffer).collect();
        self.wire
            .to_server
            .borrow_mut()
            .poll_write(context, &slices)
    }

    fn is_write_vectored(&self) -> bool {
        self.vectored
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut to_server = self.wire.to_server.borrow_mut();
        if to_server.end == End::Open {
            to_server.end = End::Eof;
        }
        to_server.wake();
        Poll::Ready(Ok(()))
    }
}
