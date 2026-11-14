use std::{
    io,
    os::fd::AsFd,
    task::{Context, Poll},
};

use nanocode_executor::signal::unix::{Signal, SignalKind, signal};
use nanocode_executor::{Direction, Source};

use crate::terminal;

mod event;
mod parse;

pub use event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MediaKeyCode,
    ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};

const TTY_BUFFER_BYTES: usize = 1_024;

pub struct EventStream {
    input: Source<terminal::Tty>,
    resize: Signal,
    parser: parse::Parser,
    bytes: Box<[u8; TTY_BUFFER_BYTES]>,
}

impl EventStream {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            input: Source::new(terminal::tty()?)?,
            resize: signal(SignalKind::window_change())?,
            parser: parse::Parser::default(),
            bytes: Box::new([0; TTY_BUFFER_BYTES]),
        })
    }

    pub async fn next(&mut self) -> Option<io::Result<Event>> {
        std::future::poll_fn(|context| self.poll_next(context)).await
    }

    pub fn poll_next(&mut self, context: &mut Context<'_>) -> Poll<Option<io::Result<Event>>> {
        loop {
            if let Some(event) = self.parser.next_event() {
                return Poll::Ready(Some(Ok(event)));
            }
            match self.input.poll_ready(context, Direction::Read) {
                Poll::Ready(ready) => {
                    let fd = self.input.get_ref().as_fd();
                    match terminal::readable_now(fd) {
                        Ok(true) => {}
                        Ok(false) => {
                            self.input.clear(ready);
                            continue;
                        }
                        Err(error) => return Poll::Ready(Some(Err(error))),
                    }
                    match nanocode_sys::unix::read(fd, &mut self.bytes[..]) {
                        Ok(0) => return Poll::Ready(None),
                        Ok(count) => {
                            let more = count == TTY_BUFFER_BYTES;
                            self.parser.advance(&self.bytes[..count], more);
                            if !more {
                                self.input.clear(ready);
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            self.input.clear(ready);
                        }
                        Err(error) => return Poll::Ready(Some(Err(error))),
                    }
                    continue;
                }
                Poll::Pending => {}
            }
            if self.resize.poll_recv(context).is_ready() {
                return Poll::Ready(Some(
                    terminal::size().map(|(columns, rows)| Event::Resize(columns, rows)),
                ));
            }
            return Poll::Pending;
        }
    }
}
