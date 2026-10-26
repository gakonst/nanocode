use std::fmt::Display;

use futures_util::{Sink, SinkExt, Stream, StreamExt};
use nanocode_sync::Owned;
use tokio::sync::{mpsc, oneshot};

use super::{PEER_CLOSED, RemoteDelivery, RemoteFrame, RemoteLink};
use crate::embedded::{CodeModeHostError, HostFuture};

type Writes<M> = mpsc::UnboundedSender<(Vec<M>, Option<oneshot::Sender<bool>>)>;

pub struct MessageWriter<M> {
    writes: Writes<M>,
}

impl<M> Clone for MessageWriter<M> {
    fn clone(&self) -> Self {
        Self {
            writes: self.writes.clone(),
        }
    }
}

fn write_failed() -> CodeModeHostError {
    CodeModeHostError::new("socket write failed")
}

impl<M: Send + 'static> MessageWriter<M> {
    pub fn new<Si>(mut sink: Si) -> (Self, impl Future<Output = ()> + Send)
    where
        Si: Sink<M> + Unpin + Send + 'static,
    {
        let (writes, mut written) =
            mpsc::unbounded_channel::<(Vec<M>, Option<oneshot::Sender<bool>>)>();
        let writer = async move {
            while let Some((messages, done)) = written.recv().await {
                let mut wrote = true;
                for message in messages {
                    if sink.feed(message).await.is_err() {
                        wrote = false;
                        break;
                    }
                }
                wrote = wrote && sink.flush().await.is_ok();
                if let Some(done) = done {
                    let _ = done.send(wrote);
                }
                if !wrote {
                    return;
                }
            }
            let _ = sink.close().await;
        };
        (Self { writes }, writer)
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn spawn<Si>(sink: Si) -> Self
    where
        Si: Sink<M> + Unpin + Send + 'static,
    {
        let (writer, writing) = Self::new(sink);
        tokio::spawn(writing);
        writer
    }

    pub async fn send(&self, message: M) -> Result<(), CodeModeHostError> {
        self.write(vec![message]).await
    }

    async fn write(&self, messages: Vec<M>) -> Result<(), CodeModeHostError> {
        let (done, written) = oneshot::channel();
        self.writes
            .send((messages, Some(done)))
            .map_err(|_| write_failed())?;
        match written.await {
            Ok(true) => Ok(()),
            _ => Err(write_failed()),
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn post(&self, message: M) {
        let _ = self.writes.send((vec![message], None));
    }
}

pub struct MessageLink<St, M> {
    stream: Owned<St>,
    writer: MessageWriter<M>,
    max_frame_bytes: Option<usize>,
    resumes: Option<String>,
}

impl<St, M> MessageLink<St, M> {
    pub fn new(
        stream: St,
        writer: MessageWriter<M>,
        max_frame_bytes: Option<usize>,
        resumes: Option<String>,
    ) -> Self {
        Self {
            stream: Owned::new(stream),
            writer,
            max_frame_bytes,
            resumes,
        }
    }
}

impl<St, M, E> RemoteLink for MessageLink<St, M>
where
    St: Stream<Item = Result<String, E>> + Unpin + Send + 'static,
    E: Display,
    M: From<String> + Send + 'static,
{
    fn send(
        &self,
        frames: Vec<RemoteFrame>,
        _: RemoteDelivery,
    ) -> HostFuture<'_, Result<(), CodeModeHostError>> {
        Box::pin(async move {
            let limit = self.max_frame_bytes.unwrap_or(usize::MAX);
            let mut messages = Vec::with_capacity(frames.len());
            for frame in frames {
                let text = frame
                    .encode(limit)
                    .map_err(|error| CodeModeHostError::unsent(error.to_string()))?;
                messages.push(M::from(text));
            }
            self.writer.write(messages).await
        })
    }

    fn receive(&self) -> HostFuture<'_, Result<Vec<RemoteFrame>, CodeModeHostError>> {
        Box::pin(async move {
            match self.stream.acquire().await.next().await {
                Some(Ok(text)) => Ok(vec![RemoteFrame::decode(
                    &text,
                    self.max_frame_bytes.unwrap_or(usize::MAX),
                )?]),
                Some(Err(error)) => Err(CodeModeHostError::new(error.to_string())),
                None => Err(CodeModeHostError::new(PEER_CLOSED)),
            }
        })
    }

    fn close<'a>(&'a self, _: &'a str) -> HostFuture<'a, ()> {
        Box::pin(async {})
    }

    fn max_frame_bytes(&self) -> Option<usize> {
        self.max_frame_bytes
    }

    fn resumes(&self) -> Option<String> {
        self.resumes.clone()
    }
}

pub enum SocketMessage<M> {
    Text(String),
    Ping(M),
    Pong,
    Close,
    Invalid(&'static str),
}

#[cfg(not(target_family = "wasm"))]
#[expect(clippy::type_complexity, reason = "the link's boxed message stream")]
pub fn websocket<St, M, E>(
    stream: St,
    writer: MessageWriter<M>,
    max_frame_bytes: Option<usize>,
    resumes: Option<String>,
    classify: fn(M) -> SocketMessage<M>,
    ping: fn() -> M,
    (interval, timeout): (std::time::Duration, std::time::Duration),
) -> (
    MessageLink<futures_util::stream::BoxStream<'static, Result<String, &'static str>>, M>,
    impl Future<Output = &'static str> + Send,
)
where
    St: Stream<Item = Result<M, E>> + Send + 'static,
    M: Send + 'static,
{
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use tokio::time::Instant;

    let start = Instant::now();
    let answered = Arc::new(AtomicU64::new(0));
    let (replies, pong) = (writer.clone(), Arc::clone(&answered));
    let messages = stream.filter_map(move |message| {
        let message = message.map(classify).map_err(drop);
        let text = match message {
            Ok(SocketMessage::Text(text)) => Some(Ok(text)),
            Ok(SocketMessage::Ping(reply)) => {
                replies.post(reply);
                None
            }
            Ok(SocketMessage::Pong) => {
                let elapsed = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
                pong.store(elapsed, Ordering::Relaxed);
                None
            }
            Ok(SocketMessage::Invalid(error)) => Some(Err(error)),
            Ok(SocketMessage::Close) => Some(Err(PEER_CLOSED)),
            Err(()) => Some(Err("socket read failed")),
        };
        std::future::ready(text)
    });
    let link = MessageLink::new(messages.boxed(), writer.clone(), max_frame_bytes, resumes);
    let heartbeat = async move {
        let mut heartbeat = tokio::time::interval(interval);
        loop {
            heartbeat.tick().await;
            let last = start + std::time::Duration::from_millis(answered.load(Ordering::Relaxed));
            if last.elapsed() >= timeout {
                return "heartbeat lost";
            }
            if writer.send(ping()).await.is_err() {
                return "socket write failed";
            }
        }
    };
    (link, heartbeat)
}
