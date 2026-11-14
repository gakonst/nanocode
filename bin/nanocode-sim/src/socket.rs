use std::{
    collections::VecDeque,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker},
};

use nanocode_io::{AsyncRead, AsyncWrite, ReadBuf};
use nanocode_sync::Slot;

use crate::rng::Rng;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    Open,
    Closed,
    Reset,
}

struct Direction {
    bytes: VecDeque<u8>,
    end: End,
    reader: Option<Waker>,
}

impl Direction {
    const fn new() -> Self {
        Self {
            bytes: VecDeque::new(),
            end: End::Open,
            reader: None,
        }
    }

    fn read(
        &mut self,
        into: &mut [u8],
        piece_max: usize,
        waker: &Waker,
    ) -> Poll<io::Result<usize>> {
        if !self.bytes.is_empty() {
            let count = into.len().min(self.bytes.len()).min(piece_max);
            for (slot, byte) in into.iter_mut().zip(self.bytes.drain(..count)) {
                *slot = byte;
            }
            return Poll::Ready(Ok(count));
        }
        match self.end {
            End::Open => {
                self.reader = Some(waker.clone());
                Poll::Pending
            }
            End::Closed => Poll::Ready(Ok(0)),
            End::Reset => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "sim: connection reset by peer",
            ))),
        }
    }

    fn end(&mut self, end: End) -> Option<Waker> {
        if self.end == End::Open {
            self.end = end;
        }
        self.reader.take()
    }
}

struct Line {
    to_client: Direction,
    to_server: Direction,
    client_gone: bool,
    server_gone: bool,
    rng: Rng,
    read_piece_max: usize,
    write_piece_max: usize,
}

pub struct Wire {
    line: Slot<Line>,
}

impl Wire {
    fn with<R>(&self, change: impl FnOnce(&mut Line) -> (R, Option<Waker>)) -> R {
        let mut line = self
            .line
            .take()
            .expect("the simulator polls one task at a time, so a socket is never taken twice");
        let (result, wake) = change(&mut line);
        assert!(self.line.put(line).is_none(), "a socket has one home");
        if let Some(waker) = wake {
            waker.wake();
        }
        result
    }
}

pub fn pair(rng: Rng, read_piece_max: usize, write_piece_max: usize) -> (ClientSide, ServerSide) {
    assert!(read_piece_max > 0, "a read delivers at least one byte");
    assert!(write_piece_max > 0, "a write accepts at least one byte");
    let wire = Arc::new(Wire {
        line: Slot::new(Some(Line {
            to_client: Direction::new(),
            to_server: Direction::new(),
            client_gone: false,
            server_gone: false,
            rng,
            read_piece_max,
            write_piece_max,
        })),
    });
    (ClientSide(Arc::clone(&wire)), ServerSide(wire))
}

pub struct ClientSide(Arc<Wire>);

impl AsyncRead for ClientSide {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let polled = self.0.with(|line| {
            let piece_max = line.rng.range(1, line.read_piece_max as u64);
            let piece_max = usize::try_from(piece_max).expect("a piece bound fits in usize");
            let into = buffer.initialize_unfilled();
            (line.to_client.read(into, piece_max, context.waker()), None)
        });
        match polled {
            Poll::Ready(Ok(count)) => {
                buffer.advance(count);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for ClientSide {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(self.0.with(|line| {
            if line.to_client.end == End::Reset {
                return (
                    Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "sim: write to a reset connection",
                    )),
                    None,
                );
            }
            if line.to_server.end != End::Open {
                return (
                    Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "sim: write after shutdown",
                    )),
                    None,
                );
            }
            let piece_max = line.rng.range(1, line.write_piece_max as u64);
            let piece_max = usize::try_from(piece_max).expect("a piece bound fits in usize");
            let count = bytes.len().min(piece_max);
            if !line.server_gone {
                line.to_server.bytes.extend(&bytes[..count]);
            }
            (Ok(count), line.to_server.reader.take())
        }))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0.with(|line| ((), line.to_server.end(End::Closed)));
        Poll::Ready(Ok(()))
    }
}

impl Drop for ClientSide {
    fn drop(&mut self) {
        self.0.with(|line| {
            line.client_gone = true;
            ((), line.to_server.end(End::Closed))
        });
    }
}

pub struct ServerSide(Arc<Wire>);

impl ServerSide {
    pub async fn read(&self, into: &mut [u8]) -> io::Result<usize> {
        std::future::poll_fn(|context| {
            self.0
                .with(|line| (line.to_server.read(into, usize::MAX, context.waker()), None))
        })
        .await
    }

    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        self.0.with(|line| {
            if line.client_gone {
                return (
                    Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "sim: the client closed the connection",
                    )),
                    None,
                );
            }
            assert_eq!(
                line.to_client.end,
                End::Open,
                "the server writes only to an open connection"
            );
            line.to_client.bytes.extend(bytes);
            (Ok(()), line.to_client.reader.take())
        })
    }

    pub fn close(&self) {
        self.0.with(|line| ((), line.to_client.end(End::Closed)));
    }

    pub fn client_gone(&self) -> bool {
        self.0.with(|line| (line.client_gone, None))
    }

    pub fn reset(&self) {
        self.0.with(|line| {
            line.to_client.bytes.clear();
            ((), line.to_client.end(End::Reset))
        });
    }
}

impl Drop for ServerSide {
    fn drop(&mut self) {
        self.0.with(|line| {
            line.server_gone = true;
            ((), line.to_client.end(End::Closed))
        });
    }
}
