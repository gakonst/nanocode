use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use nanocode_compression::{
    inflate::{Inflate, InflateFlush, InflateStatus},
    miniz::{self, Format, Level},
};
use nanocode_http::types::{HeaderMap, header};
use nanocode_websocket::WebSocketConfig;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

const EXTENSION: &str = "permessage-deflate";
pub(crate) const OFFER: &str = "permessage-deflate; client_no_context_takeover";
const LEVEL: Level = match Level::new(2) {
    Ok(level) => level,
    Err(_) => panic!("level 2 is a miniz level"),
};
const RAW_WINDOW_BITS: i32 = -15;
const DECOMPRESSION_ERROR: &str = "deflate decompression error";
const MESSAGE_TAIL: [u8; 4] = [0x00, 0x00, 0xff, 0xff];
const HEADER_END: &[u8] = b"\r\n\r\n";
const OPCODE_CONTINUATION: u8 = 0x0;
const OPCODE_TEXT: u8 = 0x1;
const OPCODE_BINARY: u8 = 0x2;

pub(crate) fn negotiated(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::SEC_WEBSOCKET_EXTENSIONS)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|extension| extension.split(';').next().map(str::trim) == Some(EXTENSION))
}

pub(crate) fn deflate(message: &[u8]) -> io::Result<Vec<u8>> {
    let mut compressed = miniz::compress_sync_flush(message, LEVEL, Format::Raw);
    if !compressed.ends_with(&MESSAGE_TAIL) {
        return Err(io::Error::other(
            "deflate sync flush did not end with an empty stored block",
        ));
    }
    compressed.truncate(compressed.len() - MESSAGE_TAIL.len());
    Ok(compressed)
}

#[derive(Clone, Copy)]
enum Mode {
    Handshake,
    Frames,
    Passthrough,
}

struct Header {
    fin: bool,
    rsv1: bool,
    opcode: u8,
    masked: bool,
    length: usize,
    payload: usize,
}

pub(crate) struct InflateStream<S> {
    inner: S,
    mode: Mode,
    scratch: Box<[u8]>,
    input: Vec<u8>,
    output: Vec<u8>,
    output_start: usize,
    message: Option<(u8, Vec<u8>)>,
    inflater: Inflate,
    eof: bool,
}

impl<S> InflateStream<S> {
    pub(crate) fn new(inner: S) -> Self {
        Self {
            inner,
            mode: Mode::Handshake,
            scratch: vec![0; WebSocketConfig::default().read_buffer_bytes].into_boxed_slice(),
            input: Vec::new(),
            output: Vec::new(),
            output_start: 0,
            message: None,
            inflater: raw_inflater(),
            eof: false,
        }
    }

    fn process(&mut self) -> io::Result<()> {
        let mut at = 0;
        loop {
            match self.mode {
                Mode::Handshake => {
                    let Some(end) = self.input[at..]
                        .windows(HEADER_END.len())
                        .position(|window| window == HEADER_END)
                    else {
                        break;
                    };
                    let header = &self.input[at..at + end + HEADER_END.len()];
                    let upgraded = header
                        .split(|byte| *byte == b'\n')
                        .next()
                        .and_then(|line| std::str::from_utf8(line).ok())
                        .and_then(|line| line.split_whitespace().nth(1))
                        == Some("101");
                    self.output.extend_from_slice(header);
                    at += header.len();
                    self.mode = if upgraded {
                        Mode::Frames
                    } else {
                        Mode::Passthrough
                    };
                }
                Mode::Passthrough => {
                    self.output.extend_from_slice(&self.input[at..]);
                    at = self.input.len();
                    break;
                }
                Mode::Frames => {
                    let Some(header) = parse(&self.input[at..])? else {
                        break;
                    };
                    let frame = at..at + header.length;
                    let payload = frame.end - header.payload..frame.end;
                    at = frame.end;
                    let data = matches!(header.opcode, OPCODE_TEXT | OPCODE_BINARY);
                    if header.masked {
                        self.output.extend_from_slice(&self.input[frame]);
                    } else if data && header.rsv1 {
                        let payload = self.input[payload].to_vec();
                        if header.fin {
                            self.emit(header.opcode, &payload)?;
                        } else {
                            self.message = Some((header.opcode, payload));
                        }
                    } else if header.opcode == OPCODE_CONTINUATION
                        && let Some((opcode, mut compressed)) = self.message.take()
                    {
                        compressed.extend_from_slice(&self.input[payload]);
                        if header.fin {
                            self.emit(opcode, &compressed)?;
                        } else {
                            self.message = Some((opcode, compressed));
                        }
                    } else {
                        self.output.extend_from_slice(&self.input[frame]);
                    }
                }
            }
        }
        self.input.drain(..at);
        Ok(())
    }

    fn emit(&mut self, opcode: u8, compressed: &[u8]) -> io::Result<()> {
        let message = self.inflate(compressed)?;
        self.output.push(0x80 | opcode);
        match u16::try_from(message.len()) {
            Ok(length) if length < 126 => self.output.push(length as u8),
            Ok(length) => {
                self.output.push(126);
                self.output.extend_from_slice(&length.to_be_bytes());
            }
            Err(_) => {
                self.output.push(127);
                self.output
                    .extend_from_slice(&(message.len() as u64).to_be_bytes());
            }
        }
        self.output.extend_from_slice(&message);
        Ok(())
    }

    fn inflate(&mut self, compressed: &[u8]) -> io::Result<Vec<u8>> {
        let mut input = Vec::with_capacity(compressed.len() + MESSAGE_TAIL.len());
        input.extend_from_slice(compressed);
        input.extend_from_slice(&MESSAGE_TAIL);
        let mut message = Vec::with_capacity(input.len());
        let mut offset = 0;
        loop {
            if message.len() == message.capacity() {
                message.reserve(message.capacity());
            }
            let written = message.len();
            let (read, produced) = (self.inflater.total_in(), self.inflater.total_out());
            message.resize(message.capacity(), 0);
            let status = self.inflater.inflate(
                &input[offset..],
                &mut message[written..],
                InflateFlush::SyncFlush,
            );
            let produced = usize::try_from(self.inflater.total_out() - produced).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "inflated output overflow")
            })?;
            message.truncate(written + produced);
            let status = status
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, DECOMPRESSION_ERROR))?;
            let consumed = usize::try_from(self.inflater.total_in() - read).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "inflated input overflow")
            })?;
            offset += consumed;
            if status == InflateStatus::StreamEnd {
                self.inflater = raw_inflater();
                return Ok(message);
            }
            if offset == input.len()
                && (message.len() < message.capacity() || message.len() == written)
            {
                return Ok(message);
            }
            if consumed == 0 && message.len() == written {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "compressed WebSocket message did not inflate",
                ));
            }
        }
    }
}

fn raw_inflater() -> Inflate {
    Inflate::new(RAW_WINDOW_BITS)
        .expect("raw deflate with a 32 KiB window is a valid inflate configuration")
}

fn parse(bytes: &[u8]) -> io::Result<Option<Header>> {
    let [first, second, ..] = *bytes else {
        return Ok(None);
    };
    let (payload, mut length): (u64, usize) = match second & 0x7f {
        126 => {
            let Some(extended) = bytes.get(2..4) else {
                return Ok(None);
            };
            (u64::from(u16::from_be_bytes([extended[0], extended[1]])), 4)
        }
        127 => {
            let Some(extended) = bytes.get(2..10) else {
                return Ok(None);
            };
            let mut be = [0; 8];
            be.copy_from_slice(extended);
            (u64::from_be_bytes(be), 10)
        }
        payload => (u64::from(payload), 2),
    };
    let masked = second & 0x80 != 0;
    if masked {
        length += 4;
    }
    let payload = usize::try_from(payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "WebSocket frame is too large"))?;
    let length = length.checked_add(payload).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "WebSocket frame is too large")
    })?;
    if bytes.len() < length {
        return Ok(None);
    }
    Ok(Some(Header {
        fin: first & 0x80 != 0,
        rsv1: first & 0x40 != 0,
        opcode: first & 0x0f,
        masked,
        length,
        payload,
    }))
}

impl<S: AsyncRead + Unpin> AsyncRead for InflateStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.output_start < this.output.len() {
                let available = &this.output[this.output_start..];
                let count = available.len().min(buf.remaining());
                buf.put_slice(&available[..count]);
                this.output_start += count;
                if this.output_start == this.output.len() {
                    this.output.clear();
                    this.output_start = 0;
                }
                return Poll::Ready(Ok(()));
            }
            if this.eof {
                if this.input.is_empty() {
                    return Poll::Ready(Ok(()));
                }
                this.output.append(&mut this.input);
                continue;
            }
            let mut read = ReadBuf::new(&mut this.scratch);
            match Pin::new(&mut this.inner).poll_read(context, &mut read) {
                Poll::Ready(Ok(())) if read.filled().is_empty() => this.eof = true,
                Poll::Ready(Ok(())) => {
                    this.input.extend_from_slice(read.filled());
                    this.process()?;
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for InflateStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(context, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(context, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(context)
    }
}
