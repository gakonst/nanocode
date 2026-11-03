use crate::{
    DecoderOptions,
    block::{BLOCK_SIZE_MAX, BlockHeader, BlockType},
    decode::{append_bytes, append_repeated, decode_compressed_block_into},
    decode_out::DecodeOut,
    error::{Error, Result},
    frame::{FrameHeader, ZstandardFrameHeader, parse_frame_header_with_format},
    literals::LiteralsState,
    sequence::{OutputLimit, RepeatOffsets, SequenceTablesState},
    xxhash::Xxh64State,
};
use std::mem;
use std::ops::Range;
pub struct StreamingDecoder {
    options: DecoderOptions,
    input: Vec<u8>,
    input_pos: usize,
    output: Vec<u8>,
    output_pos: usize,
    literals_scratch: Vec<u8>,
    state: DecoderState,
    spare_frame: Option<Box<FrameDecodeState>>,
    total_output_size: u64,
    finished_input: bool,
    input_dropped: usize,
    decoded_a_frame: bool,
}
impl StreamingDecoder {
    pub fn new(options: DecoderOptions) -> Self {
        Self::initialize(options)
    }
}
impl Default for StreamingDecoder {
    fn default() -> Self {
        Self::new(DecoderOptions::default())
    }
}
impl StreamingDecoder {
    pub const RECOMMENDED_INPUT_SIZE: usize = BLOCK_SIZE_MAX + BlockHeader::SIZE;
    pub const RECOMMENDED_OUTPUT_SIZE: usize = BLOCK_SIZE_MAX;
    pub fn push(&mut self, src: &[u8]) -> Result<()> {
        if self.finished_input {
            return Err(Error::InvalidParameter("cannot push after finish"));
        }
        self.input.extend_from_slice(src);
        self.process()
    }
    pub fn finish(&mut self) -> Result<()> {
        if self.finished_input {
            return self.finish_status();
        }
        self.finished_input = true;
        self.process()?;
        self.finish_status()
    }
    pub fn reset(&mut self) -> Result<()> {
        self.finish_status()?;
        self.reset_state();
        Ok(())
    }
    pub fn read(&mut self, dst: &mut [u8]) -> usize {
        let count = dst.len().min(self.pending_output_len());
        if count == 0 {
            return 0;
        }
        dst[..count].copy_from_slice(&self.pending_output()[..count]);
        self.consume_output(count);
        count
    }
    pub fn take_output(&mut self) -> Vec<u8> {
        if self.output_pos == 0 && !self.in_frame() {
            return mem::take(&mut self.output);
        }
        let out = self.output[self.output_pos..].to_vec();
        self.output_pos = self.output.len();
        self.compact_output();
        out
    }
    pub const fn pending_output_len(&self) -> usize {
        self.output.len().saturating_sub(self.output_pos)
    }
    pub(crate) fn pending_output(&self) -> &[u8] {
        &self.output[self.output_pos..]
    }
    pub(crate) fn consume_output(&mut self, count: usize) {
        assert!(
            count <= self.pending_output_len(),
            "consumed {count} bytes of pending output but only {} are pending",
            self.pending_output_len()
        );
        self.output_pos += count;
        self.compact_output();
    }
    pub fn unconsumed_input(&self) -> &[u8] {
        &self.input[self.input_pos..]
    }
    pub const fn input_consumed(&self) -> usize {
        self.stream_input_pos()
    }
    pub const fn is_finished(&self) -> bool {
        self.finished_input
            && matches!(self.state, DecoderState::FrameHeader)
            && self.input_pos == self.input.len()
    }
    fn initialize(options: DecoderOptions) -> Self {
        let mut decoder = Self {
            options,
            input: Vec::new(),
            input_pos: 0,
            output: Vec::new(),
            output_pos: 0,
            literals_scratch: Vec::new(),
            state: DecoderState::FrameHeader,
            spare_frame: None,
            total_output_size: 0,
            finished_input: false,
            input_dropped: 0,
            decoded_a_frame: false,
        };
        decoder.reset_state();
        decoder
    }
    fn process(&mut self) -> Result<()> {
        loop {
            let progress = match self.state {
                DecoderState::FrameHeader => self.process_frame_header()?,
                DecoderState::Skippable { .. } => self.process_skippable_frame(),
                DecoderState::Frame {
                    stage: FrameStage::BlockHeader,
                    ..
                } => self.process_block_header()?,
                DecoderState::Frame {
                    stage: FrameStage::BlockPayload { .. },
                    ..
                } => self.process_block_payload()?,
                DecoderState::Frame {
                    stage: FrameStage::Checksum,
                    ..
                } => self.process_frame_checksum()?,
            };
            if !progress {
                break;
            }
            self.compact_input();
        }
        Ok(())
    }
    fn process_frame_header(&mut self) -> Result<bool> {
        if self.input_pos == self.input.len() {
            return Ok(false);
        }
        if self.options.single_frame && self.decoded_a_frame {
            return Err(Error::TrailingInput {
                offset: self.stream_input_pos(),
            });
        }
        match parse_frame_header_with_format(&self.input[self.input_pos..], self.options.format) {
            Ok(FrameHeader::Skippable(skippable)) => {
                if self.options.single_frame {
                    return Err(Error::TrailingInput {
                        offset: self.stream_input_pos(),
                    });
                }
                self.input_pos += skippable.header_size;
                self.state = DecoderState::Skippable {
                    remaining: skippable.size as usize,
                };
                Ok(true)
            }
            Ok(FrameHeader::Zstandard(header)) => {
                self.input_pos += header.header_size;
                self.start_frame(header)?;
                Ok(true)
            }
            Err(Error::UnexpectedEof) => Ok(false),
            Err(err) => Err(err),
        }
    }
    fn process_skippable_frame(&mut self) -> bool {
        let DecoderState::Skippable { remaining } = &mut self.state else {
            unreachable!();
        };
        let available = self.input.len() - self.input_pos;
        let consumed = available.min(*remaining);
        self.input_pos += consumed;
        *remaining -= consumed;
        if *remaining == 0 {
            self.state = DecoderState::FrameHeader;
        }
        consumed != 0
    }
    fn process_block_header(&mut self) -> Result<bool> {
        if self.input.len() - self.input_pos < BlockHeader::SIZE {
            return Ok(false);
        }
        let header =
            BlockHeader::parse(&self.input[self.input_pos..self.input_pos + BlockHeader::SIZE])?;
        self.input_pos += BlockHeader::SIZE;
        self.set_stage(FrameStage::BlockPayload { header });
        Ok(true)
    }
    fn process_block_payload(&mut self) -> Result<bool> {
        let DecoderState::Frame {
            ref frame,
            stage: FrameStage::BlockPayload { header },
        } = self.state
        else {
            unreachable!();
        };
        let checksum = frame.header.checksum;
        let payload_size = header.payload_size();
        if self.input.len() - self.input_pos < payload_size {
            return Ok(false);
        }
        let payload = self.input_pos..self.input_pos + payload_size;
        self.input_pos += payload_size;
        self.decode_block(header, payload)?;
        match (header.last_block, checksum) {
            (false, _) => self.set_stage(FrameStage::BlockHeader),
            (true, true) => self.set_stage(FrameStage::Checksum),
            (true, false) => self.finish_frame()?,
        }
        Ok(true)
    }
    fn process_frame_checksum(&mut self) -> Result<bool> {
        if self.input.len() - self.input_pos < 4 {
            return Ok(false);
        }
        let expected = u32::from_le_bytes([
            self.input[self.input_pos],
            self.input[self.input_pos + 1],
            self.input[self.input_pos + 2],
            self.input[self.input_pos + 3],
        ]);
        self.input_pos += 4;
        let DecoderState::Frame { frame, .. } = &self.state else {
            return Err(Error::Generic);
        };
        if self.options.verify_checksum {
            let actual = frame
                .checksum
                .as_ref()
                .ok_or(Error::Corruption("missing frame checksum state"))?
                .digest() as u32;
            if actual != expected {
                return Err(Error::ChecksumMismatch { expected, actual });
            }
        }
        self.finish_frame()?;
        Ok(true)
    }
    fn start_frame(&mut self, header: ZstandardFrameHeader) -> Result<()> {
        self.compact_between_frames();
        if let Some(dictionary_id) = header.dictionary_id {
            return Err(Error::DictionaryRequired(Some(dictionary_id)));
        }
        if let Some(max_window_size) = self.options.max_window_size
            && header.window_size > max_window_size
        {
            return Err(Error::WindowSizeTooLarge {
                window_size: header.window_size,
                max_window_size,
            });
        }
        if let Some(content_size) = header.content_size {
            ensure_total_size_limit(
                self.total_output_size,
                content_size,
                self.options.max_output_size,
            )?;
        }
        let checksum = header.checksum;
        let window_size = usize::try_from(header.window_size).unwrap_or(usize::MAX);
        let frame = FrameDecodeState {
            literals_state: LiteralsState::default(),
            sequence_tables: SequenceTablesState::default(),
            repeat_offsets: RepeatOffsets::default(),
            checksum: checksum.then(|| Xxh64State::new(0)),
            frame_output_size: 0,
            window_size,
            frame_start: self.output.len(),
            header,
        };
        let frame = match self.spare_frame.take() {
            Some(mut recycled) => {
                *recycled = frame;
                recycled
            }
            None => Box::new(frame),
        };
        self.state = DecoderState::Frame {
            frame,
            stage: FrameStage::BlockHeader,
        };
        Ok(())
    }
    fn decode_block(
        &mut self,
        block_header: BlockHeader,
        payload_range: Range<usize>,
    ) -> Result<()> {
        let block_start = self.output.len();
        let released = self.total_output_size.saturating_sub(block_start as u64);
        let limit = OutputLimit::after(released, self.options.max_output_size);
        let payload = &self.input[payload_range];
        let dictionary_content = None;
        let DecoderState::Frame { frame, .. } = &mut self.state else {
            return Err(Error::Generic);
        };
        let oversized = block_header.block_size > frame.header.block_size_max;
        match block_header.block_type {
            BlockType::Raw => {
                if oversized {
                    return Err(Error::Corruption(
                        "block size exceeds frame block size limit",
                    ));
                }
                append_bytes(&mut DecodeOut::growable(&mut self.output), payload, limit)?
            }
            BlockType::Rle => {
                if oversized {
                    return Err(Error::Corruption(
                        "block size exceeds frame block size limit",
                    ));
                }
                append_repeated(
                    &mut DecodeOut::growable(&mut self.output),
                    *payload.first().ok_or(Error::UnexpectedEof)?,
                    block_header.block_size as usize,
                    limit,
                )?
            }
            BlockType::Compressed => decode_compressed_block_into(
                payload,
                frame.header.block_size_max as usize,
                frame.frame_start,
                frame.window_size,
                dictionary_content,
                &mut frame.literals_state,
                &mut frame.sequence_tables,
                &mut frame.repeat_offsets,
                &mut self.literals_scratch,
                &mut DecodeOut::growable(&mut self.output),
                limit,
            )?,
        }
        let produced = self.output.len() - block_start;
        frame.absorb_block_output(&self.output[block_start..]);
        self.total_output_size += produced as u64;
        Ok(())
    }
    fn finish_frame(&mut self) -> Result<()> {
        let frame = match mem::replace(&mut self.state, DecoderState::FrameHeader) {
            DecoderState::Frame { frame, .. } => frame,
            other => {
                self.state = other;
                return Err(Error::Generic);
            }
        };
        if let Some(expected_size) = frame.header.content_size
            && frame.frame_output_size != expected_size
        {
            return Err(Error::ContentSizeMismatch {
                expected: expected_size,
                actual: frame.frame_output_size,
            });
        }
        self.decoded_a_frame = true;
        self.spare_frame = Some(frame);
        Ok(())
    }
    const fn finish_status(&self) -> Result<()> {
        if self.input_dropped == 0 && self.input.is_empty() {
            return Err(Error::UnexpectedEof);
        }
        if !matches!(self.state, DecoderState::FrameHeader) {
            return Err(Error::UnexpectedEof);
        }
        if self.input_pos != self.input.len() {
            return Err(Error::UnexpectedEof);
        }
        Ok(())
    }
    fn compact_input(&mut self) {
        if self.input_pos == 0 {
            return;
        }
        if self.input_pos == self.input.len() {
            self.input_dropped += self.input_pos;
            self.input.clear();
            self.input_pos = 0;
            return;
        }
        if self.input_pos >= 4096 || self.input_pos * 2 >= self.input.len() {
            self.input_dropped += self.input_pos;
            self.input.drain(..self.input_pos);
            self.input_pos = 0;
        }
    }
    const fn stream_input_pos(&self) -> usize {
        self.input_dropped + self.input_pos
    }
    fn compact_output(&mut self) {
        let retain = self.retain_for_matches();
        let droppable = self
            .output_pos
            .min(self.output.len().saturating_sub(retain));
        if droppable == 0 {
            return;
        }
        if droppable == self.output.len() {
            self.output.clear();
            self.output_pos = 0;
            self.release_history(droppable);
            return;
        }
        if !self.in_frame() {
            return;
        }
        if droppable * 2 >= self.output.len() {
            self.output.drain(..droppable);
            self.output_pos -= droppable;
            self.release_history(droppable);
        }
    }
    fn compact_between_frames(&mut self) {
        debug_assert!(
            !self.in_frame(),
            "a frame is opening while another is still current"
        );
        if self.output_pos != 0 && self.output_pos * 2 >= self.output.len() {
            self.output.drain(..self.output_pos);
            self.output_pos = 0;
        }
    }
    const fn in_frame(&self) -> bool {
        matches!(self.state, DecoderState::Frame { .. })
    }
    const fn set_stage(&mut self, next: FrameStage) {
        if let DecoderState::Frame { stage, .. } = &mut self.state {
            *stage = next;
        }
    }
    fn retain_for_matches(&self) -> usize {
        match &self.state {
            DecoderState::Frame { frame, .. } => frame.window_size.saturating_add(1),
            DecoderState::FrameHeader | DecoderState::Skippable { .. } => 0,
        }
    }
    fn release_history(&mut self, dropped: usize) {
        let retained = self.output.len();
        if let DecoderState::Frame { frame, .. } = &mut self.state {
            frame.frame_start = frame.frame_start.saturating_sub(dropped);
            debug_assert!(
                frame.frame_start <= retained,
                "compaction left the frame starting past the end of the buffer"
            );
            let held = (retained - frame.frame_start) as u64;
            debug_assert!(
                held == frame.frame_output_size || held > frame.window_size as u64,
                "compaction left {held} bytes of a {}-byte frame, inside its {}-byte window",
                frame.frame_output_size,
                frame.window_size
            );
        }
    }
    fn reset_state(&mut self) {
        self.input.clear();
        self.input_pos = 0;
        if let DecoderState::Frame { frame, .. } =
            mem::replace(&mut self.state, DecoderState::FrameHeader)
        {
            self.spare_frame = Some(frame);
        }
        self.total_output_size = 0;
        self.finished_input = false;
        self.input_dropped = 0;
        self.decoded_a_frame = false;
    }
}
enum DecoderState {
    FrameHeader,
    Skippable {
        remaining: usize,
    },
    Frame {
        frame: Box<FrameDecodeState>,
        stage: FrameStage,
    },
}
#[derive(Clone, Copy)]
enum FrameStage {
    BlockHeader,
    BlockPayload { header: BlockHeader },
    Checksum,
}
struct FrameDecodeState {
    header: ZstandardFrameHeader,
    literals_state: LiteralsState,
    sequence_tables: SequenceTablesState,
    repeat_offsets: RepeatOffsets,
    checksum: Option<Xxh64State>,
    frame_output_size: u64,
    window_size: usize,
    frame_start: usize,
}
impl FrameDecodeState {
    fn absorb_block_output(&mut self, produced: &[u8]) {
        if let Some(checksum) = self.checksum.as_mut() {
            checksum.update(produced);
        }
        self.frame_output_size += produced.len() as u64;
    }
}
fn ensure_total_size_limit(
    current_output_size: u64,
    content_size: u64,
    max_output_size: Option<usize>,
) -> Result<()> {
    if let Some(limit) = max_output_size {
        let total = current_output_size
            .checked_add(content_size)
            .ok_or(Error::OutputSizeOverflow)?;
        if total > limit as u64 {
            return Err(Error::OutputSizeTooLarge {
                output_size: total,
                max_output_size: limit,
            });
        }
    }
    Ok(())
}
impl std::fmt::Debug for StreamingDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamingDecoder")
            .field("pending_output_len", &self.pending_output_len())
            .field("finished", &self.is_finished())
            .finish_non_exhaustive()
    }
}
