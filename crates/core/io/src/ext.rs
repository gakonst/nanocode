use crate::{AsyncBufRead, AsyncRead, AsyncSeek, AsyncWrite, ReadBuf};
use std::{
    future::Future,
    io::{self, IoSlice, SeekFrom},
    pin::Pin,
    task::{Context, Poll, ready},
};

const PROBE_BYTES: usize = 32;
const READ_BUF_RESERVE_BYTES: usize = 64;

fn early_eof() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "early eof")
}

fn invalid_utf8() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "stream did not contain valid UTF-8",
    )
}

pub struct Read<'a, R: ?Sized> {
    reader: &'a mut R,
    buf: &'a mut [u8],
}

impl<R: AsyncRead + Unpin + ?Sized> Future for Read<'_, R> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut buf = ReadBuf::new(this.buf);
        ready!(Pin::new(&mut *this.reader).poll_read(context, &mut buf))?;
        Poll::Ready(Ok(buf.filled().len()))
    }
}

pub struct ReadExact<'a, R: ?Sized> {
    reader: &'a mut R,
    buf: &'a mut [u8],
    filled: usize,
}

impl<R: AsyncRead + Unpin + ?Sized> Future for ReadExact<'_, R> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        while this.filled < this.buf.len() {
            let mut buf = ReadBuf::new(&mut this.buf[this.filled..]);
            ready!(Pin::new(&mut *this.reader).poll_read(context, &mut buf))?;
            let read = buf.filled().len();
            if read == 0 {
                return Poll::Ready(Err(early_eof()));
            }
            this.filled += read;
        }
        Poll::Ready(Ok(this.buf.len()))
    }
}

pub struct ReadToEnd<'a, R: ?Sized> {
    reader: &'a mut R,
    buf: &'a mut Vec<u8>,
    filled: usize,
    starting_capacity: usize,
    read: usize,
}

struct Growing<'a> {
    buf: &'a mut Vec<u8>,
    filled: &'a mut usize,
    starting_capacity: usize,
}

impl Growing<'_> {
    fn reserve(&mut self, additional: usize) {
        if self.buf.capacity() - *self.filled >= additional {
            return;
        }
        self.buf.truncate(*self.filled);
        self.buf.reserve(additional);
    }

    fn poll_once<R: AsyncRead + Unpin + ?Sized>(
        &mut self,
        reader: &mut R,
        context: &mut Context<'_>,
    ) -> Poll<io::Result<usize>> {
        let spare = self.buf.capacity() - *self.filled;
        if spare < PROBE_BYTES
            && self.starting_capacity == self.buf.capacity()
            && self.starting_capacity >= PROBE_BYTES
        {
            let mut probe = [0; PROBE_BYTES];
            let mut chunk = ReadBuf::new(&mut probe);
            let polled = Pin::new(&mut *reader).poll_read(context, &mut chunk);
            let read = chunk.filled().len();
            if read > spare {
                self.reserve(PROBE_BYTES);
            }
            self.buf.truncate(*self.filled);
            self.buf.extend_from_slice(&probe[..read]);
            *self.filled += read;
            ready!(polled)?;
            return Poll::Ready(Ok(read));
        }
        if spare < PROBE_BYTES {
            self.reserve(PROBE_BYTES);
        }
        let capacity = self.buf.capacity();
        if self.buf.len() < capacity {
            self.buf.resize(capacity, 0);
        }
        let mut chunk = ReadBuf::new(&mut self.buf[*self.filled..]);
        let polled = Pin::new(&mut *reader).poll_read(context, &mut chunk);
        let read = chunk.filled().len();
        *self.filled += read;
        ready!(polled)?;
        Poll::Ready(Ok(read))
    }

    fn poll_to_end<R: AsyncRead + Unpin + ?Sized>(
        &mut self,
        reader: &mut R,
        read: &mut usize,
        context: &mut Context<'_>,
    ) -> Poll<io::Result<usize>> {
        loop {
            let polled = self.poll_once(reader, context);
            match polled {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(0)) => {
                    self.buf.truncate(*self.filled);
                    return Poll::Ready(Ok(std::mem::take(read)));
                }
                Poll::Ready(Ok(got)) => *read += got,
                Poll::Ready(Err(error)) => {
                    self.buf.truncate(*self.filled);
                    return Poll::Ready(Err(error));
                }
            }
        }
    }
}

impl<R: AsyncRead + Unpin + ?Sized> Future for ReadToEnd<'_, R> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        Growing {
            buf: this.buf,
            filled: &mut this.filled,
            starting_capacity: this.starting_capacity,
        }
        .poll_to_end(this.reader, &mut this.read, context)
    }
}

impl<R: ?Sized> Drop for ReadToEnd<'_, R> {
    fn drop(&mut self) {
        self.buf.truncate(self.filled);
    }
}

pub struct ReadBufVec<'a, R: ?Sized> {
    reader: &'a mut R,
    buf: &'a mut Vec<u8>,
}

impl<R: AsyncRead + Unpin + ?Sized> Future for ReadBufVec<'_, R> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.buf.len() == this.buf.capacity() {
            this.buf.reserve(READ_BUF_RESERVE_BYTES);
        }
        let start = this.buf.len();
        let end = this.buf.capacity();
        this.buf.resize(end, 0);
        let mut chunk = ReadBuf::new(&mut this.buf[start..]);
        let polled = Pin::new(&mut *this.reader).poll_read(context, &mut chunk);
        let read = chunk.filled().len();
        this.buf.truncate(start + read);
        ready!(polled)?;
        Poll::Ready(Ok(read))
    }
}

pub struct ReadToString<'a, R: ?Sized> {
    reader: &'a mut R,
    output: &'a mut String,
    bytes: Vec<u8>,
    prefix: usize,
    filled: usize,
    starting_capacity: usize,
    read: usize,
}

impl<R: AsyncRead + Unpin + ?Sized> Future for ReadToString<'_, R> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let outcome = ready!(
            Growing {
                buf: &mut this.bytes,
                filled: &mut this.filled,
                starting_capacity: this.starting_capacity,
            }
            .poll_to_end(this.reader, &mut this.read, context)
        );
        let mut bytes = std::mem::take(&mut this.bytes);
        let valid = std::str::from_utf8(&bytes[this.prefix..]).is_ok();
        if !valid || outcome.is_err() {
            bytes.truncate(this.prefix);
        }
        *this.output = String::from_utf8(bytes).expect("the prefix came from a string");
        match outcome {
            Ok(read) if valid => Poll::Ready(Ok(read)),
            Ok(_) => Poll::Ready(Err(invalid_utf8())),
            Err(error) => Poll::Ready(Err(error)),
        }
    }
}

impl<R: ?Sized> Drop for ReadToString<'_, R> {
    fn drop(&mut self) {
        if self.prefix > 0 && self.bytes.len() >= self.prefix {
            let mut bytes = std::mem::take(&mut self.bytes);
            bytes.truncate(self.filled);
            bytes.truncate(self.prefix);
            *self.output = String::from_utf8(bytes).expect("the prefix came from a string");
        }
    }
}

pub struct ReadInt<'a, R: ?Sized, const N: usize> {
    reader: &'a mut R,
    bytes: [u8; N],
    filled: usize,
}

impl<R: AsyncRead + Unpin + ?Sized, const N: usize> Future for ReadInt<'_, R, N> {
    type Output = io::Result<[u8; N]>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        while this.filled < N {
            let mut buf = ReadBuf::new(&mut this.bytes[this.filled..]);
            ready!(Pin::new(&mut *this.reader).poll_read(context, &mut buf))?;
            let read = buf.filled().len();
            if read == 0 {
                return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()));
            }
            this.filled += read;
        }
        Poll::Ready(Ok(this.bytes))
    }
}

pub struct ReadNumber<'a, R: ?Sized, T, const N: usize> {
    bytes: ReadInt<'a, R, N>,
    convert: fn([u8; N]) -> T,
}

impl<R: AsyncRead + Unpin + ?Sized, T, const N: usize> Future for ReadNumber<'_, R, T, N> {
    type Output = io::Result<T>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let bytes = ready!(Pin::new(&mut this.bytes).poll(context))?;
        Poll::Ready(Ok((this.convert)(bytes)))
    }
}

const fn read_number<R: ?Sized, T, const N: usize>(
    reader: &mut R,
    convert: fn([u8; N]) -> T,
) -> ReadNumber<'_, R, T, N> {
    ReadNumber {
        bytes: ReadInt {
            reader,
            bytes: [0; N],
            filled: 0,
        },
        convert,
    }
}

pub trait AsyncReadExt: AsyncRead {
    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> Read<'a, Self>
    where
        Self: Unpin,
    {
        Read { reader: self, buf }
    }

    fn read_exact<'a>(&'a mut self, buf: &'a mut [u8]) -> ReadExact<'a, Self>
    where
        Self: Unpin,
    {
        ReadExact {
            reader: self,
            buf,
            filled: 0,
        }
    }

    fn read_to_end<'a>(&'a mut self, buf: &'a mut Vec<u8>) -> ReadToEnd<'a, Self>
    where
        Self: Unpin,
    {
        ReadToEnd {
            reader: self,
            filled: buf.len(),
            starting_capacity: buf.capacity(),
            buf,
            read: 0,
        }
    }

    fn read_to_string<'a>(&'a mut self, buf: &'a mut String) -> ReadToString<'a, Self>
    where
        Self: Unpin,
    {
        let bytes = std::mem::take(buf).into_bytes();
        ReadToString {
            reader: self,
            output: buf,
            prefix: bytes.len(),
            filled: bytes.len(),
            starting_capacity: bytes.capacity(),
            bytes,
            read: 0,
        }
    }

    fn read_buf<'a>(&'a mut self, buf: &'a mut Vec<u8>) -> ReadBufVec<'a, Self>
    where
        Self: Unpin,
    {
        ReadBufVec { reader: self, buf }
    }

    fn take(&mut self, limit: u64) -> crate::Take<'_, Self>
    where
        Self: Unpin,
    {
        crate::Take::new(self, limit)
    }

    fn read_u8(&mut self) -> ReadNumber<'_, Self, u8, 1>
    where
        Self: Unpin,
    {
        read_number(self, u8::from_be_bytes)
    }

    fn read_u16(&mut self) -> ReadNumber<'_, Self, u16, 2>
    where
        Self: Unpin,
    {
        read_number(self, u16::from_be_bytes)
    }

    fn read_u32(&mut self) -> ReadNumber<'_, Self, u32, 4>
    where
        Self: Unpin,
    {
        read_number(self, u32::from_be_bytes)
    }

    fn read_u64(&mut self) -> ReadNumber<'_, Self, u64, 8>
    where
        Self: Unpin,
    {
        read_number(self, u64::from_be_bytes)
    }

    fn read_i32(&mut self) -> ReadNumber<'_, Self, i32, 4>
    where
        Self: Unpin,
    {
        read_number(self, i32::from_be_bytes)
    }
}

impl<R: AsyncRead + ?Sized> AsyncReadExt for R {}

pub struct Write<'a, W: ?Sized> {
    writer: &'a mut W,
    buf: &'a [u8],
}

impl<W: AsyncWrite + Unpin + ?Sized> Future for Write<'_, W> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        Pin::new(&mut *this.writer).poll_write(context, this.buf)
    }
}

pub struct WriteAll<'a, W: ?Sized> {
    writer: &'a mut W,
    buf: &'a [u8],
}

fn poll_write_all<W: AsyncWrite + Unpin + ?Sized>(
    writer: &mut W,
    buf: &mut &[u8],
    context: &mut Context<'_>,
) -> Poll<io::Result<()>> {
    while !buf.is_empty() {
        let written = ready!(Pin::new(&mut *writer).poll_write(context, buf))?;
        if written == 0 {
            return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
        }
        *buf = &buf[written..];
    }
    Poll::Ready(Ok(()))
}

impl<W: AsyncWrite + Unpin + ?Sized> Future for WriteAll<'_, W> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        poll_write_all(this.writer, &mut this.buf, context)
    }
}

pub struct WriteVectored<'a, 'b, W: ?Sized> {
    writer: &'a mut W,
    bufs: &'a [IoSlice<'b>],
}

impl<W: AsyncWrite + Unpin + ?Sized> Future for WriteVectored<'_, '_, W> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        Pin::new(&mut *this.writer).poll_write_vectored(context, this.bufs)
    }
}

pub struct WriteBytes<'a, W: ?Sized, const N: usize> {
    writer: &'a mut W,
    bytes: [u8; N],
    written: usize,
}

impl<W: AsyncWrite + Unpin + ?Sized, const N: usize> Future for WriteBytes<'_, W, N> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        while this.written < N {
            let written = ready!(
                Pin::new(&mut *this.writer).poll_write(context, &this.bytes[this.written..])
            )?;
            if written == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            this.written += written;
        }
        Poll::Ready(Ok(()))
    }
}

pub struct Flush<'a, W: ?Sized> {
    writer: &'a mut W,
}

impl<W: AsyncWrite + Unpin + ?Sized> Future for Flush<'_, W> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut *self.get_mut().writer).poll_flush(context)
    }
}

pub struct Shutdown<'a, W: ?Sized> {
    writer: &'a mut W,
}

impl<W: AsyncWrite + Unpin + ?Sized> Future for Shutdown<'_, W> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut *self.get_mut().writer).poll_shutdown(context)
    }
}

pub trait AsyncWriteExt: AsyncWrite {
    fn write<'a>(&'a mut self, buf: &'a [u8]) -> Write<'a, Self>
    where
        Self: Unpin,
    {
        Write { writer: self, buf }
    }

    fn write_all<'a>(&'a mut self, buf: &'a [u8]) -> WriteAll<'a, Self>
    where
        Self: Unpin,
    {
        WriteAll { writer: self, buf }
    }

    fn write_vectored<'a, 'b>(&'a mut self, bufs: &'a [IoSlice<'b>]) -> WriteVectored<'a, 'b, Self>
    where
        Self: Unpin,
    {
        WriteVectored { writer: self, bufs }
    }

    fn write_u8(&mut self, value: u8) -> WriteBytes<'_, Self, 1>
    where
        Self: Unpin,
    {
        WriteBytes {
            writer: self,
            bytes: value.to_be_bytes(),
            written: 0,
        }
    }

    fn write_u32(&mut self, value: u32) -> WriteBytes<'_, Self, 4>
    where
        Self: Unpin,
    {
        WriteBytes {
            writer: self,
            bytes: value.to_be_bytes(),
            written: 0,
        }
    }

    fn write_i32(&mut self, value: i32) -> WriteBytes<'_, Self, 4>
    where
        Self: Unpin,
    {
        WriteBytes {
            writer: self,
            bytes: value.to_be_bytes(),
            written: 0,
        }
    }

    fn flush(&mut self) -> Flush<'_, Self>
    where
        Self: Unpin,
    {
        Flush { writer: self }
    }

    fn shutdown(&mut self) -> Shutdown<'_, Self>
    where
        Self: Unpin,
    {
        Shutdown { writer: self }
    }
}

impl<W: AsyncWrite + ?Sized> AsyncWriteExt for W {}

pub struct ReadUntil<'a, R: ?Sized> {
    reader: &'a mut R,
    delimiter: u8,
    buf: &'a mut Vec<u8>,
    read: usize,
}

pub(crate) fn poll_read_until<R: AsyncBufRead + Unpin + ?Sized>(
    reader: &mut R,
    delimiter: u8,
    buf: &mut Vec<u8>,
    read: &mut usize,
    context: &mut Context<'_>,
) -> Poll<io::Result<usize>> {
    loop {
        let (done, used) = {
            let available = ready!(Pin::new(&mut *reader).poll_fill_buf(context))?;
            match available.iter().position(|byte| *byte == delimiter) {
                Some(at) => {
                    buf.extend_from_slice(&available[..=at]);
                    (true, at + 1)
                }
                None => {
                    buf.extend_from_slice(available);
                    (available.is_empty(), available.len())
                }
            }
        };
        Pin::new(&mut *reader).consume(used);
        *read += used;
        if done {
            return Poll::Ready(Ok(std::mem::take(read)));
        }
    }
}

impl<R: AsyncBufRead + Unpin + ?Sized> Future for ReadUntil<'_, R> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        poll_read_until(
            this.reader,
            this.delimiter,
            this.buf,
            &mut this.read,
            context,
        )
    }
}

pub struct ReadLine<'a, R: ?Sized> {
    reader: &'a mut R,
    buf: &'a mut String,
    bytes: Vec<u8>,
    read: usize,
}

pub(crate) fn poll_read_line<R: AsyncBufRead + Unpin + ?Sized>(
    reader: &mut R,
    buf: &mut String,
    bytes: &mut Vec<u8>,
    read: &mut usize,
    context: &mut Context<'_>,
) -> Poll<io::Result<usize>> {
    let polled = poll_read_until(reader, b'\n', bytes, read, context);
    let outcome = match polled {
        Poll::Pending => return Poll::Pending,
        Poll::Ready(outcome) => outcome,
    };
    let taken = std::mem::take(bytes);
    match (String::from_utf8(taken), outcome) {
        (Ok(text), outcome) => {
            if buf.is_empty() {
                *buf = text;
            } else {
                buf.push_str(&text);
            }
            Poll::Ready(outcome)
        }
        (Err(_), Ok(_)) => Poll::Ready(Err(invalid_utf8())),
        (Err(_), Err(error)) => Poll::Ready(Err(error)),
    }
}

impl<R: AsyncBufRead + Unpin + ?Sized> Future for ReadLine<'_, R> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        poll_read_line(
            this.reader,
            this.buf,
            &mut this.bytes,
            &mut this.read,
            context,
        )
    }
}

pub struct FillBuf<'a, R: ?Sized> {
    reader: Option<&'a mut R>,
}

impl<'a, R: AsyncBufRead + Unpin + ?Sized> Future for FillBuf<'a, R> {
    type Output = io::Result<&'a [u8]>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let reader = this.reader.take().expect("polled after completion");
        match Pin::new(&mut *reader).poll_fill_buf(context) {
            Poll::Ready(Ok(_)) => match Pin::new(reader).poll_fill_buf(context) {
                Poll::Ready(outcome) => Poll::Ready(outcome),
                Poll::Pending => unreachable!("a filled buffer stays filled"),
            },
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => {
                this.reader = Some(reader);
                Poll::Pending
            }
        }
    }
}

pub trait AsyncBufReadExt: AsyncBufRead {
    fn read_until<'a>(&'a mut self, delimiter: u8, buf: &'a mut Vec<u8>) -> ReadUntil<'a, Self>
    where
        Self: Unpin,
    {
        ReadUntil {
            reader: self,
            delimiter,
            buf,
            read: 0,
        }
    }

    fn read_line<'a>(&'a mut self, buf: &'a mut String) -> ReadLine<'a, Self>
    where
        Self: Unpin,
    {
        ReadLine {
            reader: self,
            buf,
            bytes: Vec::new(),
            read: 0,
        }
    }

    fn fill_buf(&mut self) -> FillBuf<'_, Self>
    where
        Self: Unpin,
    {
        FillBuf { reader: Some(self) }
    }

    fn consume(&mut self, amount: usize)
    where
        Self: Unpin,
    {
        Pin::new(self).consume(amount);
    }

    fn lines(self) -> crate::Lines<Self>
    where
        Self: Sized,
    {
        crate::Lines::new(self)
    }
}

impl<R: AsyncBufRead + ?Sized> AsyncBufReadExt for R {}

pub struct Seek<'a, S: ?Sized> {
    seeker: &'a mut S,
    position: Option<SeekFrom>,
}

impl<S: AsyncSeek + Unpin + ?Sized> Future for Seek<'_, S> {
    type Output = io::Result<u64>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Some(position) = this.position.take() {
            Pin::new(&mut *this.seeker).start_seek(position)?;
        }
        Pin::new(&mut *this.seeker).poll_complete(context)
    }
}

pub trait AsyncSeekExt: AsyncSeek {
    fn seek(&mut self, position: SeekFrom) -> Seek<'_, Self>
    where
        Self: Unpin,
    {
        Seek {
            seeker: self,
            position: Some(position),
        }
    }

    fn rewind(&mut self) -> Seek<'_, Self>
    where
        Self: Unpin,
    {
        self.seek(SeekFrom::Start(0))
    }

    fn stream_position(&mut self) -> Seek<'_, Self>
    where
        Self: Unpin,
    {
        self.seek(SeekFrom::Current(0))
    }
}

impl<S: AsyncSeek + ?Sized> AsyncSeekExt for S {}
