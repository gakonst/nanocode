use crate::GifError;
use crate::lzw::{Lzw, Status};

const READ_BUFFER_BYTES: usize = 8 * 1024;
const MEMORY_LIMIT_BYTES: usize = 50_000_000;
const PALETTE_CHANNELS: usize = 3;
const HEADER_BYTES: usize = 6;
const SCREEN_DESCRIPTOR_BYTES: usize = 7;
const IMAGE_DESCRIPTOR_BYTES: usize = 9;
const IMAGE_BLOCK: u8 = 0x2C;
const EXTENSION_BLOCK: u8 = 0x21;
const TRAILER_BLOCK: u8 = 0x3B;
const TEXT_EXTENSION: u8 = 0x01;
const CONTROL_EXTENSION: u8 = 0xF9;
const COMMENT_EXTENSION: u8 = 0xFE;
const APPLICATION_EXTENSION: u8 = 0xFF;
const CONTROL_EXTENSION_BYTES: usize = 5;
const TABLE_FLAG: u8 = 0x80;
const TABLE_SIZE_MASK: u8 = 0b111;
const INTERLACE_FLAG: u8 = 0b0100_0000;
const TRANSPARENCY_FLAG: u8 = 1;
const CODE_BITS_MIN: u8 = 1;
const CODE_BITS_MAX: u8 = 11;
const STEP_SLACK_BYTES: usize = 16;
const BITS_PER_BYTE: usize = 8;

#[derive(Clone, Copy)]
enum State {
    Magic,
    ScreenDescriptor,
    ImageBlockStart,
    GlobalPalette(usize),
    BlockStart(u8),
    BlockEnd,
    ExtensionBlockStart,
    ExtensionDataBlock(usize),
    ApplicationExtension,
    LocalPalette(usize),
    LzwInit(u8),
    DecodeSubBlock(usize),
    FrameDecoded,
    Trailer,
}

#[derive(Clone, Default)]
pub(crate) struct Frame {
    pub(crate) left: u16,
    pub(crate) top: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) interlaced: bool,
    pub(crate) transparent: Option<u8>,
    pub(crate) palette: Option<Vec<u8>>,
}

pub(crate) enum Event {
    GlobalPalette(Vec<u8>),
    HeaderEnd,
    FrameMetadata,
    BytesDecoded(usize),
    DataEnd,
    Other,
}

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
    state: State,
    at_end: bool,
    lzw: Option<Lzw>,
    pub(crate) width: u16,
    pub(crate) height: u16,
    global_table_bytes: usize,
    global_table: Vec<u8>,
    extension_label: u8,
    extension: Vec<u8>,
    current: Option<Frame>,
    header_end_reached: bool,
}

const fn known_extension(label: u8) -> bool {
    matches!(
        label,
        TEXT_EXTENSION | CONTROL_EXTENSION | COMMENT_EXTENSION | APPLICATION_EXTENSION
    )
}

impl<'a> Reader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            position: 0,
            state: State::Magic,
            at_end: false,
            lzw: None,
            width: 0,
            height: 0,
            global_table_bytes: 0,
            global_table: Vec::new(),
            extension_label: 0,
            extension: Vec::new(),
            current: None,
            header_end_reached: false,
        }
    }

    pub(crate) fn take_frame(&mut self) -> Frame {
        let frame = self
            .current
            .as_mut()
            .expect("frame metadata follows an image block");
        let taken = frame.clone();
        frame.palette = None;
        taken
    }

    fn window(&self) -> &'a [u8] {
        let end = (self.position / READ_BUFFER_BYTES + 1) * READ_BUFFER_BYTES;
        &self.bytes[self.position..end.min(self.bytes.len())]
    }

    fn fixed(&mut self, count: usize) -> Result<&'a [u8], GifError> {
        let end = self
            .position
            .checked_add(count)
            .ok_or(GifError::UnexpectedEnd)?;
        let taken = self
            .bytes
            .get(self.position..end)
            .ok_or(GifError::UnexpectedEnd)?;
        self.position = end;
        Ok(taken)
    }

    pub(crate) fn next(
        &mut self,
        mut output: Option<&mut [u8]>,
    ) -> Result<Option<Event>, GifError> {
        let steps_max = (self.bytes.len() + STEP_SLACK_BYTES) * BITS_PER_BYTE;
        for _ in 0..steps_max {
            if self.at_end {
                return Ok(None);
            }
            if self.position == self.bytes.len() {
                return Err(GifError::UnexpectedEnd);
            }
            match self.step(output.as_deref_mut())? {
                None => {}
                Some(Step::Trailer) => self.at_end = true,
                Some(Step::Event(event)) => return Ok(Some(event)),
            }
        }
        unreachable!("every decoding step consumes input or emits an event")
    }

    fn step(&mut self, output: Option<&mut [u8]>) -> Result<Option<Step>, GifError> {
        let byte = self.bytes[self.position];
        match self.state {
            State::Magic => {
                let magic = self.fixed(HEADER_BYTES)?;
                if magic != b"GIF87a" && magic != b"GIF89a" {
                    return Err(GifError::Format("malformed GIF header"));
                }
                self.state = State::ScreenDescriptor;
                Ok(None)
            }
            State::ScreenDescriptor => {
                let descriptor = self.fixed(SCREEN_DESCRIPTOR_BYTES)?;
                self.width = u16::from_le_bytes([descriptor[0], descriptor[1]]);
                self.height = u16::from_le_bytes([descriptor[2], descriptor[3]]);
                let flags = descriptor[4];
                self.global_table_bytes = if flags & TABLE_FLAG != 0 {
                    PALETTE_CHANNELS * (1 << ((flags & TABLE_SIZE_MASK) + 1))
                } else {
                    0
                };
                self.state = State::GlobalPalette(self.global_table_bytes);
                Ok(Some(Step::Event(Event::Other)))
            }
            State::GlobalPalette(left) => {
                if left > 0 {
                    let piece = &self.window()[..left.min(self.window().len())];
                    self.global_table.extend_from_slice(piece);
                    self.position += piece.len();
                    self.state = State::GlobalPalette(left - piece.len());
                    Ok(None)
                } else {
                    self.position += 1;
                    self.state = State::BlockStart(byte);
                    Ok(Some(Step::Event(Event::GlobalPalette(std::mem::take(
                        &mut self.global_table,
                    )))))
                }
            }
            State::BlockStart(kind) => self.block_start(kind, byte),
            State::BlockEnd => {
                if byte != TRAILER_BLOCK {
                    self.position += 1;
                }
                self.state = State::BlockStart(byte);
                Ok(None)
            }
            State::ImageBlockStart => {
                let descriptor = self.fixed(IMAGE_DESCRIPTOR_BYTES)?;
                let frame = self.current.as_mut().ok_or(GifError::Format("bad state"))?;
                let word = |at: usize| u16::from_le_bytes([descriptor[at], descriptor[at + 1]]);
                frame.left = word(0);
                frame.top = word(2);
                frame.width = word(4);
                frame.height = word(6);
                let flags = descriptor[8];
                frame.interlaced = flags & INTERLACE_FLAG != 0;
                let local = if flags & TABLE_FLAG != 0 {
                    let bytes = PALETTE_CHANNELS * (1 << ((flags & TABLE_SIZE_MASK) + 1));
                    frame
                        .palette
                        .get_or_insert_with(Vec::new)
                        .reserve_exact(bytes);
                    bytes
                } else {
                    0
                };
                self.state = State::LocalPalette(local);
                Ok(None)
            }
            State::ExtensionBlockStart => {
                self.extension.push(byte);
                self.position += 1;
                self.state = State::ExtensionDataBlock(usize::from(byte));
                Ok(None)
            }
            State::ExtensionDataBlock(left) => self.extension_data(left, byte),
            State::ApplicationExtension => {
                self.position += 1;
                self.state = State::BlockEnd;
                Ok(None)
            }
            State::LocalPalette(left) => {
                if left > 0 {
                    let window = self.window();
                    let piece = &window[..left.min(window.len())];
                    let frame = self.current.as_mut().ok_or(GifError::Format("bad state"))?;
                    if let Some(palette) = frame.palette.as_mut() {
                        palette.extend_from_slice(piece);
                    }
                    self.position += piece.len();
                    self.state = State::LocalPalette(left - piece.len());
                } else {
                    self.position += 1;
                    self.state = State::LzwInit(byte);
                }
                Ok(None)
            }
            State::LzwInit(code_bits_min) => {
                if !(CODE_BITS_MIN..=CODE_BITS_MAX).contains(&code_bits_min) {
                    return Err(GifError::Format("invalid minimal code size"));
                }
                match &mut self.lzw {
                    Some(lzw) if lzw.code_bits_min() == code_bits_min => lzw.reset(),
                    _ => self.lzw = Some(Lzw::new(code_bits_min)),
                }
                self.position += 1;
                self.state = State::DecodeSubBlock(usize::from(byte));
                Ok(Some(Step::Event(Event::FrameMetadata)))
            }
            State::DecodeSubBlock(left) => self.decode_sub_block(left, byte, output),
            State::FrameDecoded => {
                self.current = None;
                self.position += 1;
                self.state = State::BlockEnd;
                Ok(Some(Step::Event(Event::DataEnd)))
            }
            State::Trailer => Ok(Some(Step::Event(Event::Other))),
        }
    }

    fn block_start(&mut self, kind: u8, byte: u8) -> Result<Option<Step>, GifError> {
        if !self.header_end_reached && kind != EXTENSION_BLOCK {
            self.header_end_reached = true;
            return Ok(Some(Step::Event(Event::HeaderEnd)));
        }
        match kind {
            IMAGE_BLOCK => {
                self.current.get_or_insert_with(Frame::default);
                self.state = State::ImageBlockStart;
                Ok(Some(Step::Event(Event::Other)))
            }
            EXTENSION_BLOCK => {
                self.extension.clear();
                self.extension_label = byte;
                if !known_extension(byte) {
                    return Err(GifError::Format("unknown extension block encountered"));
                }
                self.position += 1;
                self.state = State::ExtensionBlockStart;
                Ok(Some(Step::Event(Event::Other)))
            }
            TRAILER_BLOCK => {
                self.position += 1;
                self.state = State::Trailer;
                Ok(Some(Step::Trailer))
            }
            _ => Err(GifError::Format("unknown block type encountered")),
        }
    }

    fn extension_data(&mut self, left: usize, byte: u8) -> Result<Option<Step>, GifError> {
        if left > 0 {
            let window = self.window();
            let piece = &window[..left.min(window.len())];
            if self.extension.len() + piece.len() > MEMORY_LIMIT_BYTES {
                return Err(GifError::Format("memory limit reached"));
            }
            self.extension.extend_from_slice(piece);
            self.position += piece.len();
            self.state = State::ExtensionDataBlock(left - piece.len());
            return Ok(None);
        }
        if byte != 0 {
            self.position += 1;
            self.state = State::ExtensionDataBlock(usize::from(byte));
            return Ok(Some(Step::Event(Event::Other)));
        }
        match self.extension_label {
            APPLICATION_EXTENSION => {
                self.state = State::ApplicationExtension;
            }
            CONTROL_EXTENSION => {
                self.control_extension()?;
                self.position += 1;
                self.state = State::BlockEnd;
            }
            _ => {
                self.position += 1;
                self.state = State::BlockEnd;
            }
        }
        Ok(Some(Step::Event(Event::Other)))
    }

    fn control_extension(&mut self) -> Result<(), GifError> {
        if self.extension.len() != CONTROL_EXTENSION_BYTES {
            return Err(GifError::Format("control extension has wrong length"));
        }
        let flags = self.extension[1];
        let transparent = self.extension[4];
        let frame = self.current.get_or_insert_with(Frame::default);
        frame.transparent = (flags & TRANSPARENCY_FLAG != 0).then_some(transparent);
        Ok(())
    }

    fn decode_sub_block(
        &mut self,
        left: usize,
        byte: u8,
        output: Option<&mut [u8]>,
    ) -> Result<Option<Step>, GifError> {
        let window = self.window();
        let lzw = self
            .lzw
            .as_mut()
            .expect("a frame's decoder exists once its data starts");
        if left > 0 {
            let piece = &window[..left.min(window.len())];
            let Some(output) = output.filter(|_| !lzw.ended()) else {
                self.position += piece.len();
                self.state = State::DecodeSubBlock(left - piece.len());
                return Ok(None);
            };
            let decoded = lzw.decode(piece, output);
            let status = decoded.status.map_err(|_| GifError::InvalidCode)?;
            let read = if status == Status::NoProgress {
                piece.len()
            } else {
                decoded.read
            };
            self.position += read;
            self.state = State::DecodeSubBlock(left - read);
            return Ok(
                (decoded.written > 0).then_some(Step::Event(Event::BytesDecoded(decoded.written)))
            );
        }
        if byte != 0 {
            self.position += 1;
            self.state = State::DecodeSubBlock(usize::from(byte));
            return Ok(None);
        }
        let decoded = lzw.decode(&[], output.unwrap_or(&mut []));
        let status = decoded.status.map_err(|_| GifError::InvalidCode)?;
        if decoded.written > 0 {
            return Ok(Some(Step::Event(Event::BytesDecoded(decoded.written))));
        }
        if status != Status::Progress {
            self.state = State::FrameDecoded;
        }
        Ok(None)
    }
}

enum Step {
    Event(Event),
    Trailer,
}
