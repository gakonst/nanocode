use std::{
    io::{self, IoSlice, Read, Write},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

use futures_util::{
    sink::{Sink, SinkExt as _},
    stream::{FusedStream, Stream},
    task::AtomicWaker,
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _, ReadBuf};

use crate::{
    error::{Error, ProtocolError, Result},
    frame::CloseFrame,
    handshake::{
        IntoClientRequest, client, create_response, read_handshake,
        server::{self, Callback},
        start_client, write_response,
    },
    message::Message,
    protocol::{Protocol, Role, WebSocketConfig},
};

#[derive(Default)]
struct WakerProxy {
    read: AtomicWaker,
    write: AtomicWaker,
}

impl Wake for WakerProxy {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.read.wake();
        self.write.wake();
    }
}

struct Io<'stream, 'context, 'waker, S> {
    stream: &'stream mut S,
    context: &'context mut Context<'waker>,
}

impl<S: AsyncRead + Unpin> Read for Io<'_, '_, '_, S> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut buffer = ReadBuf::new(buffer);
        match Pin::new(&mut *self.stream).poll_read(self.context, &mut buffer) {
            Poll::Ready(Ok(())) => Ok(buffer.filled().len()),
            Poll::Ready(Err(error)) => Err(error),
            Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
        }
    }
}

impl<S: AsyncWrite + Unpin> Write for Io<'_, '_, '_, S> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match Pin::new(&mut *self.stream).poll_write(self.context, buffer) {
            Poll::Ready(result) => result,
            Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
        }
    }

    fn write_vectored(&mut self, buffers: &[IoSlice<'_>]) -> io::Result<usize> {
        match Pin::new(&mut *self.stream).poll_write_vectored(self.context, buffers) {
            Poll::Ready(result) => result,
            Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match Pin::new(&mut *self.stream).poll_flush(self.context) {
            Poll::Ready(result) => result,
            Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
        }
    }
}

fn would_block(error: &Error) -> bool {
    matches!(error, Error::Io(error) if error.kind() == io::ErrorKind::WouldBlock)
}

fn ready<T>(result: Result<T>) -> Poll<Result<T>> {
    match result {
        Err(error) if would_block(&error) => Poll::Pending,
        other => Poll::Ready(other),
    }
}

pub struct WebSocketStream<S> {
    stream: S,
    protocol: Protocol,
    proxy: Arc<WakerProxy>,
    waker: Waker,
    closing: bool,
    ended: bool,
    ready: bool,
}

enum Registration {
    Read,
    Write,
    Keep,
}

impl<S: AsyncRead + AsyncWrite + Unpin> WebSocketStream<S> {
    fn new(stream: S, role: Role, read_ahead: Vec<u8>, config: Option<WebSocketConfig>) -> Self {
        let proxy = Arc::new(WakerProxy::default());
        let vectored = stream.is_write_vectored();
        Self {
            stream,
            protocol: Protocol::new(role, read_ahead, config.unwrap_or_default(), vectored),
            waker: Waker::from(Arc::clone(&proxy)),
            proxy,
            closing: false,
            ended: false,
            ready: true,
        }
    }

    fn with_io<T>(
        &mut self,
        registration: &Registration,
        context: Option<&Context<'_>>,
        operation: impl FnOnce(&mut Protocol, &mut Io<'_, '_, '_, S>) -> T,
    ) -> T {
        if let Some(context) = context {
            match registration {
                Registration::Read => self.proxy.read.register(context.waker()),
                Registration::Write => self.proxy.write.register(context.waker()),
                Registration::Keep => {}
            }
        }
        let mut context = Context::from_waker(&self.waker);
        let mut io = Io {
            stream: &mut self.stream,
            context: &mut context,
        };
        operation(&mut self.protocol, &mut io)
    }

    pub const fn get_ref(&self) -> &S {
        &self.stream
    }

    pub const fn get_mut(&mut self) -> &mut S {
        &mut self.stream
    }

    pub async fn close(&mut self, frame: Option<CloseFrame>) -> Result<()> {
        self.send(Message::Close(frame)).await
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Stream for WebSocketStream<S> {
    type Item = Result<Message>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.ended {
            return Poll::Ready(None);
        }
        let read = this.with_io(&Registration::Read, Some(context), |protocol, io| {
            protocol.read(io)
        });
        match ready(read) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(message)) => Poll::Ready(Some(Ok(message))),
            Poll::Ready(Err(error)) => {
                this.ended = true;
                if matches!(error, Error::AlreadyClosed | Error::ConnectionClosed) {
                    Poll::Ready(None)
                } else {
                    Poll::Ready(Some(Err(error)))
                }
            }
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> FusedStream for WebSocketStream<S> {
    fn is_terminated(&self) -> bool {
        self.ended
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Sink<Message> for WebSocketStream<S> {
    type Error = Error;

    fn poll_ready(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<()>> {
        let this = self.get_mut();
        if this.ready {
            return Poll::Ready(Ok(()));
        }
        let flushed = this.with_io(&Registration::Write, Some(context), |protocol, io| {
            protocol.flush(io)
        });
        ready(flushed).map(|result| {
            this.ready = true;
            result
        })
    }

    fn start_send(self: Pin<&mut Self>, message: Message) -> Result<()> {
        let this = self.get_mut();
        match this.with_io(&Registration::Keep, None, |protocol, io| {
            protocol.write(io, message)
        }) {
            Ok(()) => {
                this.ready = true;
                Ok(())
            }
            Err(error) if would_block(&error) => {
                this.ready = false;
                Ok(())
            }
            Err(error) => {
                this.ready = true;
                Err(error)
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<()>> {
        let this = self.get_mut();
        let flushed = this.with_io(&Registration::Write, Some(context), |protocol, io| {
            protocol.flush(io)
        });
        ready(flushed).map(|result| {
            this.ready = true;
            match result {
                Err(Error::ConnectionClosed) => Ok(()),
                other => other,
            }
        })
    }

    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<()>> {
        let this = self.get_mut();
        this.ready = true;
        let closing = this.closing;
        let closed = this.with_io(&Registration::Write, Some(context), |protocol, io| {
            if closing {
                protocol.flush(io)
            } else {
                protocol.close(io, None)
            }
        });
        match closed {
            Ok(()) | Err(Error::ConnectionClosed) => Poll::Ready(Ok(())),
            Err(error) if would_block(&error) => {
                this.closing = true;
                Poll::Pending
            }
            Err(error) => Poll::Ready(Err(error)),
        }
    }
}

pub async fn client_async_with_config<R, S>(
    request: R,
    mut stream: S,
    config: Option<WebSocketConfig>,
) -> Result<(WebSocketStream<S>, client::Response)>
where
    R: IntoClientRequest,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let start = start_client(request.into_client_request()?)?;
    stream.write_all(&start.request).await?;
    stream.flush().await?;
    let (response, tail) = read_handshake::<client::Response, _>(&mut stream).await?;
    let response = match start.verify.verify(response) {
        Ok(response) => response,
        Err(Error::Http(mut response)) => {
            *response.body_mut() = Some(tail);
            return Err(Error::Http(response));
        }
        Err(error) => return Err(error),
    };
    Ok((
        WebSocketStream::new(stream, Role::Client, tail, config),
        response,
    ))
}

pub async fn accept_hdr_async<S, C>(stream: S, callback: C) -> Result<WebSocketStream<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Callback,
{
    accept_hdr_async_with_config(stream, callback, None).await
}

pub async fn accept_hdr_async_with_config<S, C>(
    mut stream: S,
    callback: C,
    config: Option<WebSocketConfig>,
) -> Result<WebSocketStream<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Callback,
{
    let (request, tail) = read_handshake::<server::Request, _>(&mut stream).await?;
    if !tail.is_empty() {
        return Err(Error::Protocol(ProtocolError::JunkAfterRequest));
    }
    let response = create_response(&request)?;
    let mut output = Vec::new();
    let rejection = match callback.on_request(&request, response) {
        Ok(response) => {
            write_response(&mut output, &response)?;
            None
        }
        Err(rejection) => {
            if rejection.status().is_success() {
                return Err(Error::Protocol(ProtocolError::CustomResponseSuccessful));
            }
            write_response(&mut output, &rejection)?;
            if let Some(body) = rejection.body() {
                output.extend_from_slice(body.as_bytes());
            }
            Some(rejection)
        }
    };
    stream.write_all(&output).await?;
    stream.flush().await?;
    match rejection {
        Some(rejection) => {
            let (parts, body) = rejection.into_parts();
            let body = body.map(String::into_bytes);
            Err(Error::Http(Box::new(http::Response::from_parts(
                parts, body,
            ))))
        }
        None => Ok(WebSocketStream::new(
            stream,
            Role::Server,
            Vec::new(),
            config,
        )),
    }
}
