use std::{
    io::{self, IoSlice, Read, Write},
    mem::{replace, swap},
};

use crate::{
    error::{CapacityError, Error, ProtocolError, Result},
    frame::{
        CONTROL_PAYLOAD_BYTES_MAX, CloseCode, CloseFrame, Frame, FrameHeader, HEADER_BYTES_MAX,
        MaskSource, OpCode, apply_mask,
    },
    message::{IncompleteMessage, Message, Utf8Bytes},
};

pub const READ_BUFFER_BYTES: usize = 128 * 1024;
const WRITE_BUFFER_BYTES: usize = 128 * 1024;
const MESSAGE_BYTES_MAX: usize = 64 << 20;
const FRAME_BYTES_MAX: usize = 16 << 20;
const PROTOCOL_VIOLATION_REASON: &str = "Protocol violation";

#[derive(Debug, Clone, Copy)]
pub struct WebSocketConfig {
    pub read_buffer_bytes: usize,
    pub message_bytes_max: usize,
    pub frame_bytes_max: usize,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            read_buffer_bytes: READ_BUFFER_BYTES,
            message_bytes_max: MESSAGE_BYTES_MAX,
            frame_bytes_max: FRAME_BYTES_MAX,
        }
    }
}

impl WebSocketConfig {
    #[must_use]
    pub const fn read_buffer_bytes(mut self, bytes: usize) -> Self {
        self.read_buffer_bytes = bytes;
        self
    }

    #[must_use]
    pub const fn message_bytes_max(mut self, bytes: usize) -> Self {
        self.message_bytes_max = bytes;
        self
    }

    #[must_use]
    pub const fn frame_bytes_max(mut self, bytes: usize) -> Self {
        self.frame_bytes_max = bytes;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Server,
    Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Active,
    ClosedByUs,
    ClosedByPeer,
    CloseAcknowledged,
    Terminated,
}

impl State {
    const fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }

    const fn can_read(self) -> bool {
        matches!(self, Self::Active | Self::ClosedByUs)
    }

    const fn check_not_terminated(self) -> Result<()> {
        match self {
            Self::Terminated => Err(Error::AlreadyClosed),
            _ => Ok(()),
        }
    }

    fn check_connection_reset<T>(self, result: Result<T>) -> Result<T> {
        match result {
            Err(Error::Io(error))
                if !self.can_read() && error.kind() == io::ErrorKind::ConnectionReset =>
            {
                Err(Error::ConnectionClosed)
            }
            other => other,
        }
    }
}

struct PartialPayload {
    header: FrameHeader,
    payload: Vec<u8>,
    filled: usize,
}

struct FrameReader {
    input: Vec<u8>,
    start: usize,
    end: usize,
    read_bytes_max: usize,
    partial: Option<PartialPayload>,
}

impl FrameReader {
    fn new(read_ahead: Vec<u8>, read_buffer_bytes: usize) -> Self {
        let read_bytes_max = read_buffer_bytes.max(HEADER_BYTES_MAX);
        let end = read_ahead.len();
        let mut input = read_ahead;
        input.resize(end + read_bytes_max, 0);
        Self {
            input,
            start: 0,
            end,
            read_bytes_max,
            partial: None,
        }
    }

    fn read_frame(
        &mut self,
        stream: &mut impl Read,
        frame_bytes_max: usize,
        role: Role,
    ) -> Result<Option<Frame>> {
        loop {
            if self.partial.is_none()
                && let Some(frame) = self.parse_buffered(frame_bytes_max)?
            {
                return finish(frame, role).map(Some);
            }
            let read = match &mut self.partial {
                Some(partial) => {
                    let read = stream.read(&mut partial.payload[partial.filled..])?;
                    partial.filled += read;
                    read
                }
                None => self.read_input(stream)?,
            };
            if read == 0 {
                return Ok(None);
            }
            let complete = |partial: &mut PartialPayload| partial.filled == partial.payload.len();
            if let Some(partial) = self.partial.take_if(complete) {
                let frame = Frame::from_payload(partial.header, partial.payload);
                return finish(frame, role).map(Some);
            }
        }
    }

    fn parse_buffered(&mut self, frame_bytes_max: usize) -> Result<Option<Frame>> {
        let Some((header, length, consumed)) =
            FrameHeader::parse(&self.input[self.start..self.end])?
        else {
            return Ok(None);
        };
        self.start += consumed;
        let length = usize::try_from(length)
            .ok()
            .filter(|length| *length <= frame_bytes_max)
            .ok_or(Error::Capacity(CapacityError::MessageTooLong {
                size: length,
                max_size: frame_bytes_max,
            }))?;
        let available = self.end - self.start;
        if available >= length {
            let payload = self.input[self.start..self.start + length].to_vec();
            self.start += length;
            return Ok(Some(Frame::from_payload(header, payload)));
        }
        let mut payload = vec![0; length];
        payload[..available].copy_from_slice(&self.input[self.start..self.end]);
        self.start = 0;
        self.end = 0;
        self.partial = Some(PartialPayload {
            header,
            payload,
            filled: available,
        });
        Ok(None)
    }

    fn read_input(&mut self, stream: &mut impl Read) -> io::Result<usize> {
        if self.start > 0 {
            self.input.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
        }
        let wanted = self.end + self.read_bytes_max;
        if self.input.len() < wanted {
            self.input.resize(wanted, 0);
        }
        let read = stream.read(&mut self.input[self.end..wanted])?;
        self.end += read;
        Ok(read)
    }
}

fn finish(frame: Frame, role: Role) -> Result<Frame> {
    if role == Role::Client {
        return Ok(frame);
    }
    let (mut header, mut payload) = frame.into_parts();
    let Some(mask) = header.mask.take() else {
        return Err(Error::Protocol(ProtocolError::UnmaskedFrameFromClient));
    };
    apply_mask(&mut payload, mask);
    Ok(Frame::from_payload(header, payload))
}

pub(crate) struct Protocol {
    role: Role,
    config: WebSocketConfig,
    state: State,
    incomplete: Option<IncompleteMessage>,
    additional_send: Option<Frame>,
    unflushed_additional: bool,
    reader: FrameReader,
    output: Vec<u8>,
    output_start: usize,
    large: LargePayload,
    masks: MaskSource,
}

impl Protocol {
    pub(crate) fn new(
        role: Role,
        read_ahead: Vec<u8>,
        config: WebSocketConfig,
        vectored: bool,
    ) -> Self {
        Self {
            role,
            config,
            state: State::Active,
            incomplete: None,
            additional_send: None,
            unflushed_additional: false,
            reader: FrameReader::new(read_ahead, config.read_buffer_bytes),
            output: Vec::new(),
            output_start: 0,
            large: LargePayload::new(vectored),
            masks: MaskSource::new(),
        }
    }

    pub(crate) fn read(&mut self, stream: &mut (impl Read + Write)) -> Result<Message> {
        self.state.check_not_terminated()?;
        loop {
            if self.additional_send.is_some() || self.unflushed_additional {
                match self.flush(stream) {
                    Ok(()) => {}
                    Err(Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {
                        self.unflushed_additional = true;
                    }
                    Err(error) => return Err(error),
                }
            } else if self.role == Role::Server && !self.state.can_read() {
                self.state = State::Terminated;
                return Err(Error::ConnectionClosed);
            }
            if let Some(message) = self.read_message_frame(stream)? {
                return Ok(message);
            }
        }
    }

    pub(crate) fn write(&mut self, stream: &mut impl Write, message: Message) -> Result<()> {
        self.state.check_not_terminated()?;
        if !self.state.is_active() {
            return Err(Error::Protocol(ProtocolError::SendAfterClosing));
        }
        let frame = match message {
            Message::Text(text) => {
                Frame::message(text.into_string().into_bytes(), OpCode::Text, true)
            }
            Message::Binary(data) => Frame::message(data, OpCode::Binary, true),
            Message::Ping(data) => Frame::ping(data),
            Message::Pong(data) => {
                self.set_additional(Frame::pong(data));
                return self.write_pending(stream, None).map(|_| ());
            }
            Message::Close(frame) => return self.close(stream, frame),
            Message::Frame(frame) => frame,
        };
        if self.write_pending(stream, Some(frame))? {
            self.flush(stream)?;
        }
        Ok(())
    }

    pub(crate) fn flush(&mut self, stream: &mut impl Write) -> Result<()> {
        self.write_pending(stream, None)?;
        self.write_out(stream)?;
        stream.flush()?;
        self.unflushed_additional = false;
        Ok(())
    }

    pub(crate) fn close(
        &mut self,
        stream: &mut impl Write,
        frame: Option<CloseFrame>,
    ) -> Result<()> {
        if self.state == State::Active {
            self.state = State::ClosedByUs;
            self.write_pending(stream, Some(Frame::close(frame)))?;
        }
        self.flush(stream)
    }

    fn write_pending(&mut self, stream: &mut impl Write, frame: Option<Frame>) -> Result<bool> {
        if let Some(frame) = frame {
            self.buffer_frame(stream, frame)?;
        }
        let should_flush = match self.additional_send.take() {
            Some(frame) => {
                self.buffer_frame(stream, frame)?;
                true
            }
            None => self.unflushed_additional,
        };
        if self.role == Role::Server && !self.state.can_read() {
            self.write_out(stream)?;
            self.state = State::Terminated;
            return Err(Error::ConnectionClosed);
        }
        Ok(should_flush)
    }

    fn buffer_frame(&mut self, stream: &mut impl Write, mut frame: Frame) -> Result<()> {
        if self.role == Role::Client {
            frame.header_mut().mask = Some(self.masks.next()?);
        }
        if !self.large.is_idle() || self.large.takes(&frame) {
            return self.buffer_large(stream, frame);
        }
        if self.output_start == self.output.len() {
            self.output.clear();
            self.output_start = 0;
        }
        frame.format_into(&mut self.output);
        let result = if self.output.len() - self.output_start > WRITE_BUFFER_BYTES {
            self.write_out(stream)
        } else {
            Ok(())
        };
        self.state.check_connection_reset(result)
    }

    #[cold]
    fn buffer_large(&mut self, stream: &mut impl Write, frame: Frame) -> Result<()> {
        if self.large.is_idle() {
            if self.output_start == self.output.len() {
                self.output.clear();
                self.output_start = 0;
            }
            self.large.payload = frame.format_header_into(&mut self.output);
            self.large.payload_start = 0;
        } else {
            frame.format_into(&mut self.large.behind);
        }
        let queued = self.output.len() - self.output_start + self.large.queued_bytes();
        let result = if queued > WRITE_BUFFER_BYTES {
            self.write_out(stream)
        } else {
            Ok(())
        };
        self.state.check_connection_reset(result)
    }

    fn write_out(&mut self, stream: &mut impl Write) -> Result<()> {
        if !self.large.is_idle() {
            self.write_large(stream)?;
        }
        while self.output_start < self.output.len() {
            let written = stream.write(&self.output[self.output_start..])?;
            check_written(written)?;
            self.output_start += written;
        }
        self.output.clear();
        self.output_start = 0;
        Ok(())
    }

    #[cold]
    fn write_large(&mut self, stream: &mut impl Write) -> Result<()> {
        let large = &mut self.large;
        while large.payload_start < large.payload.len() {
            let buffered = &self.output[self.output_start..];
            let payload = &large.payload[large.payload_start..];
            let written = if buffered.is_empty() {
                stream.write(payload)?
            } else {
                stream.write_vectored(&[IoSlice::new(buffered), IoSlice::new(payload)])?
            };
            check_written(written)?;
            let from_output = written.min(buffered.len());
            self.output_start += from_output;
            large.payload_start += written - from_output;
            assert!(large.payload_start <= large.payload.len());
        }
        assert_eq!(self.output_start, self.output.len());
        large.payload = Vec::new();
        large.payload_start = 0;
        swap(&mut self.output, &mut large.behind);
        large.behind.clear();
        self.output_start = 0;
        Ok(())
    }

    fn read_message_frame(&mut self, stream: &mut impl Read) -> Result<Option<Message>> {
        let frame = self
            .reader
            .read_frame(stream, self.config.frame_bytes_max, self.role);
        let Some(frame) = self.state.check_connection_reset(frame)? else {
            return match replace(&mut self.state, State::Terminated) {
                State::ClosedByPeer | State::CloseAcknowledged => Err(Error::ConnectionClosed),
                State::Active | State::ClosedByUs | State::Terminated => {
                    Err(Error::Protocol(ProtocolError::ResetWithoutClosingHandshake))
                }
            };
        };
        if !self.state.can_read() {
            return Err(Error::Protocol(ProtocolError::ReceivedAfterClosing));
        }
        let header = frame.header();
        if header.rsv1 || header.rsv2 || header.rsv3 {
            return Err(Error::Protocol(ProtocolError::NonZeroReservedBits));
        }
        if self.role == Role::Client && header.mask.is_some() {
            return Err(Error::Protocol(ProtocolError::MaskedFrameFromServer));
        }
        if header.opcode.is_control() {
            self.read_control(frame)
        } else {
            self.read_data(frame)
        }
    }

    fn read_control(&mut self, frame: Frame) -> Result<Option<Message>> {
        if !frame.header().is_final {
            return Err(Error::Protocol(ProtocolError::FragmentedControlFrame));
        }
        if frame.payload().len() > CONTROL_PAYLOAD_BYTES_MAX {
            return Err(Error::Protocol(ProtocolError::ControlFrameTooBig));
        }
        match frame.header().opcode {
            OpCode::Close => Ok(self.do_close(frame.into_close()?).map(Message::Close)),
            OpCode::Ping => {
                let data = frame.into_payload();
                if self.state.is_active() {
                    self.set_additional(Frame::pong(data.clone()));
                }
                Ok(Some(Message::Ping(data)))
            }
            OpCode::Pong => Ok(Some(Message::Pong(frame.into_payload()))),
            OpCode::Continue | OpCode::Text | OpCode::Binary => {
                unreachable!("a data frame reached the control frame path")
            }
        }
    }

    fn read_data(&mut self, frame: Frame) -> Result<Option<Message>> {
        let FrameHeader {
            opcode, is_final, ..
        } = *frame.header();
        let payload = frame.into_payload();
        let size_max = self.config.message_bytes_max;
        match (opcode, self.incomplete.as_mut()) {
            (OpCode::Continue, None) => {
                Err(Error::Protocol(ProtocolError::UnexpectedContinueFrame))
            }
            (OpCode::Continue, Some(incomplete)) => {
                incomplete.extend(&payload, size_max)?;
                match self.incomplete.take_if(|_| is_final) {
                    Some(incomplete) => incomplete.complete().map(Some),
                    None => Ok(None),
                }
            }
            (_, Some(_)) => Err(Error::Protocol(ProtocolError::ExpectedFragment(opcode))),
            (OpCode::Text, None) if is_final => {
                check_size(payload.len(), size_max)?;
                Ok(Some(Message::Text(String::from_utf8(payload)?.into())))
            }
            (_, None) if is_final => {
                check_size(payload.len(), size_max)?;
                Ok(Some(Message::Binary(payload)))
            }
            (_, None) => {
                let mut incomplete = if opcode == OpCode::Text {
                    IncompleteMessage::text()
                } else {
                    IncompleteMessage::Binary(Vec::new())
                };
                incomplete.extend(&payload, size_max)?;
                self.incomplete = Some(incomplete);
                Ok(None)
            }
        }
    }

    fn do_close(&mut self, close: Option<CloseFrame>) -> Option<Option<CloseFrame>> {
        match self.state {
            State::Active => {
                self.state = State::ClosedByPeer;
                let close = close.map(|frame| {
                    if frame.code.is_allowed() {
                        frame
                    } else {
                        CloseFrame {
                            code: CloseCode::Protocol,
                            reason: Utf8Bytes::from(PROTOCOL_VIOLATION_REASON),
                        }
                    }
                });
                self.set_additional(Frame::close(close.clone()));
                Some(close)
            }
            State::ClosedByPeer | State::CloseAcknowledged => None,
            State::ClosedByUs => {
                self.state = State::CloseAcknowledged;
                Some(close)
            }
            State::Terminated => unreachable!("a terminated socket read a close frame"),
        }
    }

    fn set_additional(&mut self, frame: Frame) {
        let empty_or_pong = self
            .additional_send
            .as_ref()
            .is_none_or(|pending| pending.header().opcode == OpCode::Pong);
        if empty_or_pong {
            self.additional_send = Some(frame);
        }
    }
}

struct LargePayload {
    vectored: bool,
    payload: Vec<u8>,
    payload_start: usize,
    behind: Vec<u8>,
}

impl LargePayload {
    const fn new(vectored: bool) -> Self {
        Self {
            vectored,
            payload: Vec::new(),
            payload_start: 0,
            behind: Vec::new(),
        }
    }

    const fn is_idle(&self) -> bool {
        self.payload.is_empty()
    }

    fn takes(&self, frame: &Frame) -> bool {
        self.vectored && frame.payload().len() >= WRITE_BUFFER_BYTES
    }

    const fn queued_bytes(&self) -> usize {
        self.payload.len() - self.payload_start + self.behind.len()
    }
}

fn check_written(written: usize) -> Result<()> {
    if written == 0 {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "Connection reset while sending",
        )));
    }
    Ok(())
}

fn check_size(size: usize, size_max: usize) -> Result<()> {
    if size > size_max {
        return Err(Error::Capacity(CapacityError::MessageTooLong {
            size: byte_count(size),
            max_size: size_max,
        }));
    }
    Ok(())
}

pub(crate) fn byte_count(bytes: usize) -> u64 {
    u64::try_from(bytes).expect("a byte count fits in 64 bits")
}
