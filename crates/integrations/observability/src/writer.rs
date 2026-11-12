use std::io::{self, Write};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::time::{Duration, Instant};


const CLOSE_SEND_WAIT: Duration = Duration::from_millis(100);
const CLOSE_SEND_POLL: Duration = Duration::from_millis(1);
const CLOSE_FLUSH_WAIT: Duration = Duration::from_millis(1_000);

enum Message {
    Line(Vec<u8>),
    Close(SyncSender<()>),
}

#[derive(Clone)]
pub(crate) struct LineWriter(SyncSender<Message>);

pub(crate) struct WriterGuard(SyncSender<Message>);

pub(crate) fn spawn(
    output: Box<dyn Write + Send>,
    lines_max: usize,
) -> io::Result<(LineWriter, WriterGuard)> {
    let (lines, received) = sync_channel(lines_max);
    std::thread::Builder::new()
        .name("nanocode-log-writer".to_owned())
        .spawn(move || run(output, &received))?;
    Ok((LineWriter(lines.clone()), WriterGuard(lines)))
}

fn run(mut output: Box<dyn Write + Send>, received: &Receiver<Message>) {
    let closed = loop {
        let Ok(mut message) = received.recv() else {
            break None;
        };
        let closing = loop {
            match message {
                Message::Line(line) => drop(output.write_all(&line)),
                Message::Close(acknowledge) => break Some(acknowledge),
            }
            message = match received.try_recv() {
                Ok(next) => next,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break None,
            };
        };
        drop(output.flush());
        if closing.is_some() {
            break closing;
        }
    };
    if let Err(error) = output.flush() {
        eprintln!("Failed to flush. Error: {error}");
    }
    if let Some(acknowledge) = closed {
        let _ = acknowledge.send(());
    }
}

impl Write for LineWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0
            .send(Message::Line(buffer.to_vec()))
            .map_err(|_| io::Error::from(io::ErrorKind::Other))?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl crate::format::Output for LineWriter {
    fn write_line(&self, line: &[u8]) -> io::Result<()> {
        self.0
            .send(Message::Line(line.to_vec()))
            .map_err(|_| io::Error::from(io::ErrorKind::Other))
    }
}

impl Drop for WriterGuard {
    fn drop(&mut self) {
        let (acknowledge, acknowledged) = sync_channel(1);
        let deadline = Instant::now() + CLOSE_SEND_WAIT;
        let mut message = Message::Close(acknowledge);
        loop {
            match self.0.try_send(message) {
                Ok(()) => {
                    let _ = acknowledged.recv_timeout(CLOSE_FLUSH_WAIT);
                    return;
                }
                Err(TrySendError::Disconnected(_)) => return,
                Err(TrySendError::Full(unsent)) if Instant::now() < deadline => {
                    message = unsent;
                    std::thread::sleep(CLOSE_SEND_POLL);
                }
                Err(TrySendError::Full(_)) => {
                    println!("Failed to send shutdown signal to logging worker. Error: Shutdown");
                    return;
                }
            }
        }
    }
}
