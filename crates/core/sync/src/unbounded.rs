use crate::stream::Stream;
use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};
use tokio::sync::mpsc;

enum Envelope<T> {
    Message(T),
    Close,
}

pub struct Sender<T> {
    letters: mpsc::UnboundedSender<Envelope<T>>,
    closed: Arc<AtomicBool>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            letters: self.letters.clone(),
            closed: Arc::clone(&self.closed),
        }
    }
}

pub struct Receiver<T> {
    letters: mpsc::UnboundedReceiver<Envelope<T>>,
    ended: bool,
}

#[must_use]
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let (letters, received) = mpsc::unbounded_channel();
    (
        Sender {
            letters,
            closed: Arc::new(AtomicBool::new(false)),
        },
        Receiver {
            letters: received,
            ended: false,
        },
    )
}

impl<T> Sender<T> {
    pub fn send(&self, message: T) -> Result<(), T> {
        if self.closed.load(Ordering::Acquire) {
            return Err(message);
        }
        self.letters
            .send(Envelope::Message(message))
            .map_err(|mpsc::error::SendError(envelope)| match envelope {
                Envelope::Message(message) => message,
                Envelope::Close => unreachable!("a send only carries the message it was given"),
            })
    }

    pub fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            drop(self.letters.send(Envelope::Close));
        }
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) || self.letters.is_closed()
    }
}

impl<T> Stream for Receiver<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<T>> {
        if self.ended {
            return Poll::Ready(None);
        }
        match self.letters.poll_recv(context) {
            Poll::Ready(Some(Envelope::Message(message))) => Poll::Ready(Some(message)),
            Poll::Ready(Some(Envelope::Close) | None) => {
                self.ended = true;
                self.letters.close();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
