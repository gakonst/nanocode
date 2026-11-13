use alloc::vec;
use alloc::vec::Vec;

use crate::boolean::{BooleanDecoder, tree};
use crate::container::{LOSSY_DIMENSION_MASK, LOSSY_MAGIC};
use crate::error::WebpError;
use crate::loop_filter::{self, EdgeKind, Thresholds};
use crate::lossy_tables::{
    AC_QUANTIZERS, DC_QUANTIZERS, INTRA_MODE_PROBABILITIES, TOKEN_PROBABILITIES,
    TOKEN_UPDATE_PROBABILITIES, TokenProbabilities,
};
use crate::stream::Cursor;

const SEGMENT_COUNT: usize = 4;
const PARTITION_COUNT_MAX: usize = 8;
const MACROBLOCK_SIZE: usize = 16;
const CHROMA_SIZE: usize = 8;
const SUBBLOCK_SIZE: usize = 4;
const BLOCK_COEFFICIENTS: usize = 16;
const MACROBLOCK_COEFFICIENTS: usize = 384;
const CHROMA_BLUE_FIRST_BLOCK: usize = 16;
const CHROMA_RED_FIRST_BLOCK: usize = 20;
const QUANTIZER_INDEX_MAX: i32 = 127;
const Y2_AC_SCALE_NUMERATOR: i32 = 155;
const Y2_AC_SCALE_DENOMINATOR: i32 = 100;
const Y2_AC_MIN: i16 = 8;
const UV_DC_MAX: i16 = 132;
const FILTER_LEVEL_MAX: i32 = 63;
const PROBABILITY_ALWAYS: u8 = 255;
const TOP_BORDER: u8 = 127;
const LEFT_BORDER: u8 = 129;
const DC_WITHOUT_EDGES: u32 = 128;
const LUMA_STRIDE: usize = 1 + MACROBLOCK_SIZE + SUBBLOCK_SIZE;
const LUMA_WORKSPACE: usize = (1 + MACROBLOCK_SIZE) * LUMA_STRIDE;
const CHROMA_STRIDE: usize = 1 + CHROMA_SIZE;
const CHROMA_WORKSPACE: usize = CHROMA_STRIDE * CHROMA_STRIDE;
const IDCT_CONSTANT_1: i64 = 20091;
const IDCT_CONSTANT_2: i64 = 35468;

const TOKEN_ZERO: u8 = 0;
const TOKEN_CATEGORY_FIRST: u8 = 5;
const TOKEN_CATEGORY_LAST: u8 = 10;
const TOKEN_END: u8 = 11;

const SEGMENT_TREE: [[u8; 2]; 3] = tree([2, 4, -0, -1, -2, -3]);
const LUMA_TREE: [[u8; 2]; 4] = tree([-4, 2, 4, 6, -0, -1, -2, -3]);
const LUMA_PROBABILITIES: [u8; 4] = [145, 156, 163, 128];
const INTRA_TREE: [[u8; 2]; 9] = tree([
    -0, 2, -1, 4, -2, 6, 8, 12, -3, 10, -5, -6, -4, 14, -7, 16, -8, -9,
]);
const CHROMA_TREE: [[u8; 2]; 3] = tree([-0, 2, -1, 4, -2, -3]);
const CHROMA_PROBABILITIES: [u8; 3] = [142, 114, 183];
const TOKEN_TREE: [[u8; 2]; 11] = tree([
    -11, 2, -0, 4, -1, 6, 8, 12, -2, 10, -3, -4, 14, 16, -5, -6, 18, 20, -7, -8, -9, -10,
]);
const CATEGORY_PROBABILITIES: [&[u8]; 6] = [
    &[159],
    &[165, 145],
    &[173, 148, 140],
    &[176, 155, 140, 135],
    &[180, 157, 141, 134, 130],
    &[254, 254, 243, 230, 196, 177, 153, 140, 133, 130, 129],
];
const CATEGORY_BASE: [i32; 6] = [5, 7, 11, 19, 35, 67];
const COEFFICIENT_BANDS: [u8; 16] = [0, 1, 2, 3, 6, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 7];
const ZIGZAG: [u8; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum LumaMode {
    #[default]
    Dc,
    Vertical,
    Horizontal,
    TrueMotion,
    Subblocks,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum ChromaMode {
    #[default]
    Dc,
    Vertical,
    Horizontal,
    TrueMotion,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum IntraMode {
    #[default]
    Dc,
    TrueMotion,
    Vertical,
    Horizontal,
    LeftDown,
    RightDown,
    VerticalRight,
    VerticalLeft,
    HorizontalDown,
    HorizontalUp,
}

impl LumaMode {
    fn from_value(value: u8) -> Self {
        match value {
            0 => Self::Dc,
            1 => Self::Vertical,
            2 => Self::Horizontal,
            3 => Self::TrueMotion,
            4 => Self::Subblocks,
            _ => unreachable!("the luma tree has five leaves"),
        }
    }

    const fn intra_mode(self) -> Option<IntraMode> {
        match self {
            Self::Dc => Some(IntraMode::Dc),
            Self::Vertical => Some(IntraMode::Vertical),
            Self::Horizontal => Some(IntraMode::Horizontal),
            Self::TrueMotion => Some(IntraMode::TrueMotion),
            Self::Subblocks => None,
        }
    }
}

impl ChromaMode {
    fn from_value(value: u8) -> Self {
        match value {
            0 => Self::Dc,
            1 => Self::Vertical,
            2 => Self::Horizontal,
            3 => Self::TrueMotion,
            _ => unreachable!("the chroma tree has four leaves"),
        }
    }
}

impl IntraMode {
    fn from_value(value: u8) -> Self {
        match value {
            0 => Self::Dc,
            1 => Self::TrueMotion,
            2 => Self::Vertical,
            3 => Self::Horizontal,
            4 => Self::LeftDown,
            5 => Self::RightDown,
            6 => Self::VerticalRight,
            7 => Self::VerticalLeft,
            8 => Self::HorizontalDown,
            9 => Self::HorizontalUp,
            _ => unreachable!("the intra tree has ten leaves"),
        }
    }
}

#[derive(Clone, Copy, Default)]
struct MacroBlock {
    intra_modes: [IntraMode; 16],
    complexity: [u8; 9],
    luma_mode: LumaMode,
    chroma_mode: ChromaMode,
    segment: u8,
    coefficients_skipped: bool,
    nonzero_coefficients: bool,
}

#[derive(Clone, Copy, Default)]
struct Segment {
    y_dc: i16,
    y_ac: i16,
    y2_dc: i16,
    y2_ac: i16,
    uv_dc: i16,
    uv_ac: i16,
    delta_values: bool,
    quantizer_level: i8,
    filter_level: i8,
}

pub(crate) struct Frame {
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) luma: Vec<u8>,
    pub(crate) chroma_blue: Vec<u8>,
    pub(crate) chroma_red: Vec<u8>,
}

impl Frame {
    pub(crate) fn buffer_width(&self) -> usize {
        usize::from(self.width).div_ceil(MACROBLOCK_SIZE) * MACROBLOCK_SIZE
    }
}

pub(crate) enum Expected {
    Size { width: u32, height: u32 },
    AreaAtMost(u64),
}

struct Borders {
    top_luma: Vec<u8>,
    left_luma: [u8; 1 + MACROBLOCK_SIZE],
    top_blue: Vec<u8>,
    left_blue: [u8; 1 + CHROMA_SIZE],
    top_red: Vec<u8>,
    left_red: [u8; 1 + CHROMA_SIZE],
}

struct LossyDecoder<'a> {
    stream: Cursor<'a>,
    header: BooleanDecoder<'a>,
    partitions: [BooleanDecoder<'a>; PARTITION_COUNT_MAX],
    partition_count: usize,
    width: u16,
    height: u16,
    macroblock_width: usize,
    macroblock_height: usize,
    segments_enabled: bool,
    segments_update_map: bool,
    segments: [Segment; SEGMENT_COUNT],
    segment_tree_probabilities: [u8; 3],
    simple_filter: bool,
    filter_level: u8,
    sharpness: u8,
    filter_adjustments: bool,
    reference_delta: [i32; 4],
    mode_delta: [i32; 4],
    token_probabilities: TokenProbabilities,
    skip_probability: Option<u8>,
    top: Vec<MacroBlock>,
    left: MacroBlock,
}

pub(crate) fn decode(stream: &[u8], expected: Expected) -> Result<Frame, WebpError> {
    let mut decoder = LossyDecoder::new(stream);
    decoder.read_frame_header()?;
    let width = u64::from(decoder.width);
    let height = u64::from(decoder.height);
    let mismatch = match expected {
        Expected::Size {
            width: expected_width,
            height: expected_height,
        } => (width != u64::from(expected_width) || height != u64::from(expected_height))
            .then_some(WebpError::ImageSizes),
        Expected::AreaAtMost(area) => (width * height > area).then_some(WebpError::FramePixels),
    };
    if let Some(error) = mismatch {
        decoder.decode_macroblocks(None)?;
        return Err(error);
    }
    let mut frame = Frame {
        width: decoder.width,
        height: decoder.height,
        luma: vec![
            0;
            decoder.macroblock_width
                * MACROBLOCK_SIZE
                * decoder.macroblock_height
                * MACROBLOCK_SIZE
        ],
        chroma_blue: vec![
            0;
            decoder.macroblock_width
                * CHROMA_SIZE
                * decoder.macroblock_height
                * CHROMA_SIZE
        ],
        chroma_red: vec![
            0;
            decoder.macroblock_width
                * CHROMA_SIZE
                * decoder.macroblock_height
                * CHROMA_SIZE
        ],
    };
    let macroblocks = decoder.decode_macroblocks(Some(&mut frame))?;
    decoder.filter(&mut frame, &macroblocks);
    Ok(frame)
}

impl<'a> LossyDecoder<'a> {
    fn new(stream: &'a [u8]) -> Self {
        Self {
            stream: Cursor::new(stream),
            header: BooleanDecoder::new(&[]),
            partitions: core::array::from_fn(|_| BooleanDecoder::new(&[])),
            partition_count: 1,
            width: 0,
            height: 0,
            macroblock_width: 0,
            macroblock_height: 0,
            segments_enabled: false,
            segments_update_map: false,
            segments: [Segment::default(); SEGMENT_COUNT],
            segment_tree_probabilities: [PROBABILITY_ALWAYS; 3],
            simple_filter: false,
            filter_level: 0,
            sharpness: 0,
            filter_adjustments: false,
            reference_delta: [0; 4],
            mode_delta: [0; 4],
            token_probabilities: TOKEN_PROBABILITIES,
            skip_probability: None,
            top: Vec::new(),
            left: MacroBlock::default(),
        }
    }

    fn read_frame_header(&mut self) -> Result<(), WebpError> {
        let tag = self.stream.read_u24()?;
        let keyframe = tag & 1 == 0;
        let first_partition_bytes = (tag >> 5) as usize;
        if keyframe {
            let magic = self.stream.read_array::<3>()?;
            if magic != LOSSY_MAGIC {
                return Err(WebpError::LossyMagic(magic));
            }
            self.width = self.stream.read_u16()? & LOSSY_DIMENSION_MASK;
            self.height = self.stream.read_u16()? & LOSSY_DIMENSION_MASK;
            self.macroblock_width = usize::from(self.width).div_ceil(MACROBLOCK_SIZE);
            self.macroblock_height = usize::from(self.height).div_ceil(MACROBLOCK_SIZE);
            self.top = vec![MacroBlock::default(); self.macroblock_width];
        }
        self.header = BooleanDecoder::new(self.stream.read(first_partition_bytes)?);
        if keyframe {
            let color_space = self.header.read_literal(1);
            let _pixel_type = self.header.read_literal(1);
            if color_space != 0 {
                return Err(WebpError::LossyColorSpace(color_space));
            }
        }
        self.segments_enabled = self.header.read_flag();
        if self.segments_enabled {
            self.read_segment_updates()?;
        }
        self.simple_filter = self.header.read_flag();
        self.filter_level = self.header.read_literal(6);
        self.sharpness = self.header.read_literal(3);
        self.filter_adjustments = self.header.read_flag();
        if self.filter_adjustments {
            self.read_filter_adjustments()?;
        }
        self.partition_count = 1 << self.header.read_literal(2);
        self.header.ensure()?;
        self.read_partitions()?;
        self.read_quantizers()?;
        if !keyframe {
            return Err(WebpError::NonKeyframe);
        }
        let _refresh_probabilities = self.header.read_literal(1);
        self.read_token_probabilities()?;
        self.skip_probability =
            (self.header.read_literal(1) == 1).then(|| self.header.read_literal(8));
        self.header.ensure()
    }

    fn read_segment_updates(&mut self) -> Result<(), WebpError> {
        self.segments_update_map = self.header.read_flag();
        if self.header.read_flag() {
            let absolute = self.header.read_flag();
            for segment in &mut self.segments {
                segment.delta_values = !absolute;
            }
            for segment in &mut self.segments {
                segment.quantizer_level = self.header.read_signed(7) as i8;
            }
            for segment in &mut self.segments {
                segment.filter_level = self.header.read_signed(6) as i8;
            }
        }
        if self.segments_update_map {
            for probability in &mut self.segment_tree_probabilities {
                *probability = if self.header.read_flag() {
                    self.header.read_literal(8)
                } else {
                    PROBABILITY_ALWAYS
                };
            }
        }
        self.header.ensure()
    }

    fn read_filter_adjustments(&mut self) -> Result<(), WebpError> {
        if self.header.read_flag() {
            for delta in &mut self.reference_delta {
                *delta = self.header.read_signed(6);
            }
            for delta in &mut self.mode_delta {
                *delta = self.header.read_signed(6);
            }
        }
        self.header.ensure()
    }

    fn read_partitions(&mut self) -> Result<(), WebpError> {
        let count = self.partition_count;
        if count > 1 {
            let sizes = self.stream.read(3 * (count - 1))?;
            for (index, size) in sizes.as_chunks::<3>().0.iter().enumerate() {
                let size = usize::from(size[0])
                    | (usize::from(size[1]) << 8)
                    | (usize::from(size[2]) << 16);
                self.partitions[index] = BooleanDecoder::new(self.stream.read(size)?);
            }
        }
        self.partitions[count - 1] = BooleanDecoder::new(self.stream.remaining());
        Ok(())
    }

    fn read_quantizers(&mut self) -> Result<(), WebpError> {
        let dc = |index: i32| DC_QUANTIZERS[index.clamp(0, QUANTIZER_INDEX_MAX) as usize];
        let ac = |index: i32| AC_QUANTIZERS[index.clamp(0, QUANTIZER_INDEX_MAX) as usize];
        let y_ac_index = self.header.read_literal(7);
        let y_dc_delta = self.header.read_signed(4);
        let y2_dc_delta = self.header.read_signed(4);
        let y2_ac_delta = self.header.read_signed(4);
        let uv_dc_delta = self.header.read_signed(4);
        let uv_ac_delta = self.header.read_signed(4);
        let count = if self.segments_enabled {
            SEGMENT_COUNT
        } else {
            1
        };
        for segment in &mut self.segments[..count] {
            let base = i32::from(if !self.segments_enabled {
                i16::from(y_ac_index)
            } else if segment.delta_values {
                i16::from(segment.quantizer_level) + i16::from(y_ac_index)
            } else {
                i16::from(segment.quantizer_level)
            });
            segment.y_dc = dc(base + y_dc_delta);
            segment.y_ac = ac(base);
            segment.y2_dc = dc(base + y2_dc_delta) * 2;
            segment.y2_ac = (i32::from(ac(base + y2_ac_delta)) * Y2_AC_SCALE_NUMERATOR
                / Y2_AC_SCALE_DENOMINATOR) as i16;
            segment.uv_dc = dc(base + uv_dc_delta);
            segment.uv_ac = ac(base + uv_ac_delta);
            if segment.y2_ac < Y2_AC_MIN {
                segment.y2_ac = Y2_AC_MIN;
            }
            if segment.uv_dc > UV_DC_MAX {
                segment.uv_dc = UV_DC_MAX;
            }
        }
        self.header.ensure()
    }

    fn read_token_probabilities(&mut self) -> Result<(), WebpError> {
        for (plane, updates) in self
            .token_probabilities
            .iter_mut()
            .zip(TOKEN_UPDATE_PROBABILITIES.iter())
        {
            for (band, band_updates) in plane.iter_mut().zip(updates.iter()) {
                for (context, context_updates) in band.iter_mut().zip(band_updates.iter()) {
                    for (probability, &update) in context.iter_mut().zip(context_updates.iter()) {
                        if self.header.read_bit(update) {
                            *probability = self.header.read_literal(8);
                        }
                    }
                }
            }
        }
        self.header.ensure()
    }

    fn decode_macroblocks(
        &mut self,
        mut frame: Option<&mut Frame>,
    ) -> Result<Vec<MacroBlock>, WebpError> {
        let mut macroblocks = Vec::with_capacity(if frame.is_some() {
            self.macroblock_width * self.macroblock_height
        } else {
            0
        });
        let mut borders = Borders {
            top_luma: vec![TOP_BORDER; usize::from(self.width) + SUBBLOCK_SIZE + MACROBLOCK_SIZE],
            left_luma: [LEFT_BORDER; 1 + MACROBLOCK_SIZE],
            top_blue: vec![TOP_BORDER; CHROMA_SIZE * self.macroblock_width],
            left_blue: [LEFT_BORDER; 1 + CHROMA_SIZE],
            top_red: vec![TOP_BORDER; CHROMA_SIZE * self.macroblock_width],
            left_red: [LEFT_BORDER; 1 + CHROMA_SIZE],
        };
        for row in 0..self.macroblock_height {
            let partition = row % self.partition_count;
            self.left = MacroBlock::default();
            for column in 0..self.macroblock_width {
                let mut macroblock = self.read_macroblock_header(column)?;
                let coefficients = if macroblock.coefficients_skipped {
                    if macroblock.luma_mode != LumaMode::Subblocks {
                        self.left.complexity[0] = 0;
                        self.top[column].complexity[0] = 0;
                    }
                    for index in 1..self.left.complexity.len() {
                        self.left.complexity[index] = 0;
                        self.top[column].complexity[index] = 0;
                    }
                    [0; MACROBLOCK_COEFFICIENTS]
                } else {
                    self.read_residuals(&mut macroblock, column, partition)?
                };
                if let Some(frame) = frame.as_deref_mut() {
                    predict_luma(frame, &mut borders, column, row, &macroblock, &coefficients);
                    predict_chroma(frame, &mut borders, column, row, &macroblock, &coefficients);
                    macroblocks.push(macroblock);
                }
            }
            borders.left_luma = [LEFT_BORDER; 1 + MACROBLOCK_SIZE];
            borders.left_blue = [LEFT_BORDER; 1 + CHROMA_SIZE];
            borders.left_red = [LEFT_BORDER; 1 + CHROMA_SIZE];
        }
        Ok(macroblocks)
    }

    fn read_macroblock_header(&mut self, column: usize) -> Result<MacroBlock, WebpError> {
        let mut macroblock = MacroBlock::default();
        let decoder = &mut self.header;
        if self.segments_enabled && self.segments_update_map {
            macroblock.segment =
                decoder.read_tree(&SEGMENT_TREE, &self.segment_tree_probabilities, 0);
        }
        macroblock.coefficients_skipped = match self.skip_probability {
            Some(probability) => decoder.read_bit(probability),
            None => false,
        };
        macroblock.luma_mode =
            LumaMode::from_value(decoder.read_tree(&LUMA_TREE, &LUMA_PROBABILITIES, 0));
        let top = &mut self.top[column];
        match macroblock.luma_mode.intra_mode() {
            None => {
                for y in 0..SUBBLOCK_SIZE {
                    for x in 0..SUBBLOCK_SIZE {
                        let above = top.intra_modes[12 + x] as usize;
                        let left = self.left.intra_modes[y] as usize;
                        let mode = IntraMode::from_value(decoder.read_tree(
                            &INTRA_TREE,
                            &INTRA_MODE_PROBABILITIES[above][left],
                            0,
                        ));
                        macroblock.intra_modes[x + y * SUBBLOCK_SIZE] = mode;
                        top.intra_modes[12 + x] = mode;
                        self.left.intra_modes[y] = mode;
                    }
                }
            }
            Some(mode) => {
                for index in 0..SUBBLOCK_SIZE {
                    macroblock.intra_modes[12 + index] = mode;
                    self.left.intra_modes[index] = mode;
                }
            }
        }
        macroblock.chroma_mode =
            ChromaMode::from_value(decoder.read_tree(&CHROMA_TREE, &CHROMA_PROBABILITIES, 0));
        top.chroma_mode = macroblock.chroma_mode;
        top.luma_mode = macroblock.luma_mode;
        top.intra_modes = macroblock.intra_modes;
        decoder.ensure()?;
        Ok(macroblock)
    }

    fn read_coefficients(
        &mut self,
        block: &mut [i32],
        partition: usize,
        plane: usize,
        complexity: usize,
        dc_quantizer: i16,
        ac_quantizer: i16,
    ) -> Result<bool, WebpError> {
        assert!(complexity <= 2);
        assert_eq!(block.len(), BLOCK_COEFFICIENTS);
        let first = usize::from(plane == 0);
        let probabilities = &self.token_probabilities[plane];
        let decoder = &mut self.partitions[partition];
        let mut complexity = complexity;
        let mut has_coefficients = false;
        let mut after_zero = false;
        for index in first..BLOCK_COEFFICIENTS {
            let band = usize::from(COEFFICIENT_BANDS[index]);
            let token = decoder.read_tree(
                &TOKEN_TREE,
                &probabilities[band][complexity],
                usize::from(after_zero),
            );
            let mut value = match token {
                TOKEN_END => break,
                TOKEN_ZERO => {
                    after_zero = true;
                    has_coefficients = true;
                    complexity = 0;
                    continue;
                }
                TOKEN_CATEGORY_FIRST..=TOKEN_CATEGORY_LAST => {
                    let category = usize::from(token - TOKEN_CATEGORY_FIRST);
                    let mut extra = 0i32;
                    for &probability in CATEGORY_PROBABILITIES[category] {
                        extra = extra + extra + i32::from(decoder.read_bit(probability));
                    }
                    CATEGORY_BASE[category] + extra
                }
                literal => i32::from(literal),
            };
            after_zero = false;
            complexity = match value {
                0 => 0,
                1 => 1,
                _ => 2,
            };
            if decoder.read_flag() {
                value = -value;
            }
            let position = usize::from(ZIGZAG[index]);
            let quantizer = if position > 0 {
                ac_quantizer
            } else {
                dc_quantizer
            };
            block[position] = value * i32::from(quantizer);
            has_coefficients = true;
        }
        decoder.ensure()?;
        Ok(has_coefficients)
    }

    fn read_residuals(
        &mut self,
        macroblock: &mut MacroBlock,
        column: usize,
        partition: usize,
    ) -> Result<[i32; MACROBLOCK_COEFFICIENTS], WebpError> {
        let segment = self.segments[usize::from(macroblock.segment)];
        let mut blocks = [0i32; MACROBLOCK_COEFFICIENTS];
        let mut plane = if macroblock.luma_mode == LumaMode::Subblocks {
            3
        } else {
            1
        };
        if plane == 1 {
            let complexity = self.top[column].complexity[0] + self.left.complexity[0];
            let mut block = [0i32; BLOCK_COEFFICIENTS];
            let nonzero = self.read_coefficients(
                &mut block,
                partition,
                plane,
                usize::from(complexity),
                segment.y2_dc,
                segment.y2_ac,
            )?;
            self.left.complexity[0] = u8::from(nonzero);
            self.top[column].complexity[0] = u8::from(nonzero);
            inverse_walsh_hadamard(&mut block);
            for (index, &value) in block.iter().enumerate() {
                blocks[BLOCK_COEFFICIENTS * index] = value;
            }
            plane = 0;
        }
        for y in 0..SUBBLOCK_SIZE {
            let mut left = self.left.complexity[y + 1];
            for x in 0..SUBBLOCK_SIZE {
                let index = x + y * SUBBLOCK_SIZE;
                let complexity = self.top[column].complexity[x + 1] + left;
                let block = &mut blocks[index * BLOCK_COEFFICIENTS..][..BLOCK_COEFFICIENTS];
                let nonzero = self.read_coefficients(
                    block,
                    partition,
                    plane,
                    usize::from(complexity),
                    segment.y_dc,
                    segment.y_ac,
                )?;
                if block[0] != 0 || nonzero {
                    macroblock.nonzero_coefficients = true;
                    inverse_dct(block);
                }
                left = u8::from(nonzero);
                self.top[column].complexity[x + 1] = u8::from(nonzero);
            }
            self.left.complexity[y + 1] = left;
        }
        for (context, first_block) in [(5, CHROMA_BLUE_FIRST_BLOCK), (7, CHROMA_RED_FIRST_BLOCK)] {
            for y in 0..2 {
                let mut left = self.left.complexity[y + context];
                for x in 0..2 {
                    let index = x + y * 2 + first_block;
                    let complexity = self.top[column].complexity[x + context] + left;
                    let block = &mut blocks[index * BLOCK_COEFFICIENTS..][..BLOCK_COEFFICIENTS];
                    let nonzero = self.read_coefficients(
                        block,
                        partition,
                        2,
                        usize::from(complexity),
                        segment.uv_dc,
                        segment.uv_ac,
                    )?;
                    if block[0] != 0 || nonzero {
                        macroblock.nonzero_coefficients = true;
                        inverse_dct(block);
                    }
                    left = u8::from(nonzero);
                    self.top[column].complexity[x + context] = u8::from(nonzero);
                }
                self.left.complexity[y + context] = left;
            }
        }
        Ok(blocks)
    }

    fn filter_parameters(&self, macroblock: &MacroBlock) -> (u8, u8, u8) {
        let segment = self.segments[usize::from(macroblock.segment)];
        let mut level = i32::from(self.filter_level);
        if level == 0 {
            return (0, 0, 0);
        }
        if self.segments_enabled {
            if segment.delta_values {
                level += i32::from(segment.filter_level);
            } else {
                level = i32::from(segment.filter_level);
            }
        }
        level = level.clamp(0, FILTER_LEVEL_MAX);
        if self.filter_adjustments {
            level += self.reference_delta[0];
            if macroblock.luma_mode == LumaMode::Subblocks {
                level += self.mode_delta[0];
            }
        }
        let level = level.clamp(0, FILTER_LEVEL_MAX) as u8;
        let mut interior = level;
        if self.sharpness > 0 {
            interior >>= if self.sharpness > 4 { 2 } else { 1 };
            interior = interior.min(9 - self.sharpness);
        }
        interior = interior.max(1);
        let hev_threshold = if level >= 40 {
            2
        } else if level >= 15 {
            1
        } else {
            0
        };
        (level, interior, hev_threshold)
    }

    fn filter(&self, frame: &mut Frame, macroblocks: &[MacroBlock]) {
        assert_eq!(
            macroblocks.len(),
            self.macroblock_width * self.macroblock_height
        );
        for row in 0..self.macroblock_height {
            for column in 0..self.macroblock_width {
                let macroblock = &macroblocks[row * self.macroblock_width + column];
                let (level, interior, hev_threshold) = self.filter_parameters(macroblock);
                if level == 0 {
                    continue;
                }
                let edges = Edges {
                    column,
                    row,
                    macroblock_edge: (level + 2) * 2 + interior,
                    subblock_edge: level * 2 + interior,
                    interior,
                    hev_threshold,
                    inner: macroblock.luma_mode == LumaMode::Subblocks
                        || (!macroblock.coefficients_skipped && macroblock.nonzero_coefficients),
                    luma_width: self.macroblock_width * MACROBLOCK_SIZE,
                    chroma_width: self.macroblock_width * CHROMA_SIZE,
                };
                if self.simple_filter {
                    edges.filter_simple(&mut frame.luma);
                } else {
                    edges.filter_normal(frame);
                }
            }
        }
    }
}

struct Edges {
    column: usize,
    row: usize,
    macroblock_edge: u8,
    subblock_edge: u8,
    interior: u8,
    hev_threshold: u8,
    inner: bool,
    luma_width: usize,
    chroma_width: usize,
}

impl Edges {
    fn filter_simple(&self, luma: &mut [u8]) {
        let stride = self.luma_width;
        let origin = self.row * MACROBLOCK_SIZE * stride + self.column * MACROBLOCK_SIZE;
        if self.column > 0 {
            loop_filter::simple_edge(luma, origin, stride, true, self.macroblock_edge);
        }
        if self.inner {
            for x in [4, 8, 12] {
                loop_filter::simple_edge(luma, origin + x, stride, true, self.subblock_edge);
            }
        }
        if self.row > 0 {
            loop_filter::simple_edge(luma, origin, stride, false, self.macroblock_edge);
        }
        if self.inner {
            for y in [4, 8, 12] {
                loop_filter::simple_edge(
                    luma,
                    origin + y * stride,
                    stride,
                    false,
                    self.subblock_edge,
                );
            }
        }
    }

    fn filter_normal(&self, frame: &mut Frame) {
        let luma_stride = self.luma_width;
        let chroma_stride = self.chroma_width;
        let luma_origin = self.row * MACROBLOCK_SIZE * luma_stride + self.column * MACROBLOCK_SIZE;
        let chroma_origin = self.row * CHROMA_SIZE * chroma_stride + self.column * CHROMA_SIZE;
        let macroblock = Thresholds {
            edge: self.macroblock_edge,
            interior: self.interior,
            hev: self.hev_threshold,
        };
        let subblock = Thresholds {
            edge: self.subblock_edge,
            ..macroblock
        };
        let luma = |frame: &mut Frame, point: usize, vertical: bool, kind: EdgeKind| {
            let thresholds = match kind {
                EdgeKind::Macroblock => macroblock,
                EdgeKind::Subblock => subblock,
            };
            loop_filter::luma_edge(
                &mut frame.luma,
                point,
                luma_stride,
                vertical,
                kind,
                thresholds,
            );
        };
        let chroma = |frame: &mut Frame, point: usize, vertical: bool, kind: EdgeKind| {
            let thresholds = match kind {
                EdgeKind::Macroblock => macroblock,
                EdgeKind::Subblock => subblock,
            };
            loop_filter::chroma_edge(
                [&mut frame.chroma_blue, &mut frame.chroma_red],
                point,
                chroma_stride,
                vertical,
                kind,
                thresholds,
            );
        };
        if self.column > 0 {
            luma(frame, luma_origin, true, EdgeKind::Macroblock);
            chroma(frame, chroma_origin, true, EdgeKind::Macroblock);
        }
        if self.inner {
            for x in [4, 8, 12] {
                luma(frame, luma_origin + x, true, EdgeKind::Subblock);
            }
            chroma(frame, chroma_origin + 4, true, EdgeKind::Subblock);
        }
        if self.row > 0 {
            luma(frame, luma_origin, false, EdgeKind::Macroblock);
            chroma(frame, chroma_origin, false, EdgeKind::Macroblock);
        }
        if self.inner {
            for y in [4, 8, 12] {
                luma(
                    frame,
                    luma_origin + y * luma_stride,
                    false,
                    EdgeKind::Subblock,
                );
            }
            chroma(
                frame,
                chroma_origin + 4 * chroma_stride,
                false,
                EdgeKind::Subblock,
            );
        }
    }
}

fn inverse_walsh_hadamard(block: &mut [i32; BLOCK_COEFFICIENTS]) {
    for index in 0..4 {
        let a1 = block[index] + block[12 + index];
        let b1 = block[4 + index] + block[8 + index];
        let c1 = block[4 + index] - block[8 + index];
        let d1 = block[index] - block[12 + index];
        block[index] = a1 + b1;
        block[4 + index] = c1 + d1;
        block[8 + index] = a1 - b1;
        block[12 + index] = d1 - c1;
    }
    for row in block.as_chunks_mut::<4>().0.iter_mut() {
        let a1 = row[0] + row[3];
        let b1 = row[1] + row[2];
        let c1 = row[1] - row[2];
        let d1 = row[0] - row[3];
        row[0] = (a1 + b1 + 3) >> 3;
        row[1] = (c1 + d1 + 3) >> 3;
        row[2] = (a1 - b1 + 3) >> 3;
        row[3] = (d1 - c1 + 3) >> 3;
    }
}

fn inverse_dct(block: &mut [i32]) {
    assert_eq!(block.len(), BLOCK_COEFFICIENTS);
    let at = |block: &[i32], index: usize| i64::from(block[index]);
    for index in 0..4 {
        let a1 = at(block, index) + at(block, 8 + index);
        let b1 = at(block, index) - at(block, 8 + index);
        let c1 = ((at(block, 4 + index) * IDCT_CONSTANT_2) >> 16)
            - (at(block, 12 + index) + ((at(block, 12 + index) * IDCT_CONSTANT_1) >> 16));
        let d1 = (at(block, 4 + index) + ((at(block, 4 + index) * IDCT_CONSTANT_1) >> 16))
            + ((at(block, 12 + index) * IDCT_CONSTANT_2) >> 16);
        block[index] = (a1 + d1) as i32;
        block[4 + index] = (b1 + c1) as i32;
        block[12 + index] = (a1 - d1) as i32;
        block[8 + index] = (b1 - c1) as i32;
    }
    for index in 0..4 {
        let row = 4 * index;
        let a1 = at(block, row) + at(block, row + 2);
        let b1 = at(block, row) - at(block, row + 2);
        let c1 = ((at(block, row + 1) * IDCT_CONSTANT_2) >> 16)
            - (at(block, row + 3) + ((at(block, row + 3) * IDCT_CONSTANT_1) >> 16));
        let d1 = (at(block, row + 1) + ((at(block, row + 1) * IDCT_CONSTANT_1) >> 16))
            + ((at(block, row + 3) * IDCT_CONSTANT_2) >> 16);
        block[row] = ((a1 + d1 + 4) >> 3) as i32;
        block[row + 3] = ((a1 - d1 + 4) >> 3) as i32;
        block[row + 1] = ((b1 + c1 + 4) >> 3) as i32;
        block[row + 2] = ((b1 - c1 + 4) >> 3) as i32;
    }
}

fn predict_luma(
    frame: &mut Frame,
    borders: &mut Borders,
    column: usize,
    row: usize,
    macroblock: &MacroBlock,
    coefficients: &[i32; MACROBLOCK_COEFFICIENTS],
) {
    let stride = LUMA_STRIDE;
    let macroblock_width = frame.buffer_width() / MACROBLOCK_SIZE;
    let mut workspace = luma_workspace(
        column,
        row,
        macroblock_width,
        &borders.top_luma,
        &borders.left_luma,
    );
    match macroblock.luma_mode {
        LumaMode::Vertical => predict_vertical(&mut workspace, MACROBLOCK_SIZE, stride),
        LumaMode::Horizontal => predict_horizontal(&mut workspace, MACROBLOCK_SIZE, stride),
        LumaMode::TrueMotion => predict_true_motion(&mut workspace, MACROBLOCK_SIZE, 1, 1, stride),
        LumaMode::Dc => predict_dc(
            &mut workspace,
            MACROBLOCK_SIZE,
            stride,
            row != 0,
            column != 0,
        ),
        LumaMode::Subblocks => predict_subblocks(
            &mut workspace,
            stride,
            &macroblock.intra_modes,
            coefficients,
        ),
    }
    if macroblock.luma_mode != LumaMode::Subblocks {
        for y in 0..SUBBLOCK_SIZE {
            for x in 0..SUBBLOCK_SIZE {
                let index = x + y * SUBBLOCK_SIZE;
                let residue = &coefficients[index * BLOCK_COEFFICIENTS..][..BLOCK_COEFFICIENTS];
                add_residue(
                    &mut workspace,
                    residue,
                    1 + y * SUBBLOCK_SIZE,
                    1 + x * SUBBLOCK_SIZE,
                    stride,
                );
            }
        }
    }
    borders.left_luma[0] = workspace[MACROBLOCK_SIZE];
    for (index, left) in borders.left_luma[1..].iter_mut().enumerate() {
        *left = workspace[(index + 1) * stride + MACROBLOCK_SIZE];
    }
    borders.top_luma[column * MACROBLOCK_SIZE..][..MACROBLOCK_SIZE]
        .copy_from_slice(&workspace[MACROBLOCK_SIZE * stride + 1..][..MACROBLOCK_SIZE]);
    let buffer_width = macroblock_width * MACROBLOCK_SIZE;
    for y in 0..MACROBLOCK_SIZE {
        frame.luma[(row * MACROBLOCK_SIZE + y) * buffer_width + column * MACROBLOCK_SIZE..]
            [..MACROBLOCK_SIZE]
            .copy_from_slice(&workspace[(1 + y) * stride + 1..][..MACROBLOCK_SIZE]);
    }
}

fn predict_chroma(
    frame: &mut Frame,
    borders: &mut Borders,
    column: usize,
    row: usize,
    macroblock: &MacroBlock,
    coefficients: &[i32; MACROBLOCK_COEFFICIENTS],
) {
    let stride = CHROMA_STRIDE;
    let buffer_width = frame.buffer_width() / 2;
    let planes = [
        (
            &mut frame.chroma_blue,
            &mut borders.top_blue,
            &mut borders.left_blue,
            CHROMA_BLUE_FIRST_BLOCK,
        ),
        (
            &mut frame.chroma_red,
            &mut borders.top_red,
            &mut borders.left_red,
            CHROMA_RED_FIRST_BLOCK,
        ),
    ];
    for (plane, top, left, first_block) in planes {
        let mut workspace = chroma_workspace(column, row, top, left);
        match macroblock.chroma_mode {
            ChromaMode::Dc => {
                predict_dc(&mut workspace, CHROMA_SIZE, stride, row != 0, column != 0)
            }
            ChromaMode::Vertical => predict_vertical(&mut workspace, CHROMA_SIZE, stride),
            ChromaMode::Horizontal => predict_horizontal(&mut workspace, CHROMA_SIZE, stride),
            ChromaMode::TrueMotion => {
                predict_true_motion(&mut workspace, CHROMA_SIZE, 1, 1, stride)
            }
        }
        for y in 0..2 {
            for x in 0..2 {
                let index = first_block + x + y * 2;
                let residue = &coefficients[index * BLOCK_COEFFICIENTS..][..BLOCK_COEFFICIENTS];
                add_residue(
                    &mut workspace,
                    residue,
                    1 + y * SUBBLOCK_SIZE,
                    1 + x * SUBBLOCK_SIZE,
                    stride,
                );
            }
        }
        left[0] = workspace[CHROMA_SIZE];
        for (index, value) in left[1..].iter_mut().enumerate() {
            *value = workspace[(index + 1) * stride + CHROMA_SIZE];
        }
        top[column * CHROMA_SIZE..][..CHROMA_SIZE]
            .copy_from_slice(&workspace[CHROMA_SIZE * stride + 1..][..CHROMA_SIZE]);
        for y in 0..CHROMA_SIZE {
            plane[(row * CHROMA_SIZE + y) * buffer_width + column * CHROMA_SIZE..][..CHROMA_SIZE]
                .copy_from_slice(&workspace[(1 + y) * stride + 1..][..CHROMA_SIZE]);
        }
    }
}

fn luma_workspace(
    column: usize,
    row: usize,
    macroblock_width: usize,
    top: &[u8],
    left: &[u8; 1 + MACROBLOCK_SIZE],
) -> [u8; LUMA_WORKSPACE] {
    let stride = LUMA_STRIDE;
    let mut workspace = [0u8; LUMA_WORKSPACE];
    if row == 0 {
        workspace[1..stride].fill(TOP_BORDER);
    } else {
        let start = column * MACROBLOCK_SIZE;
        workspace[1..=MACROBLOCK_SIZE].copy_from_slice(&top[start..start + MACROBLOCK_SIZE]);
        if column == macroblock_width - 1 {
            workspace[1 + MACROBLOCK_SIZE..stride].fill(top[start + MACROBLOCK_SIZE - 1]);
        } else {
            workspace[1 + MACROBLOCK_SIZE..stride].copy_from_slice(
                &top[start + MACROBLOCK_SIZE..start + MACROBLOCK_SIZE + SUBBLOCK_SIZE],
            );
        }
    }
    for index in 1 + MACROBLOCK_SIZE..stride {
        workspace[4 * stride + index] = workspace[index];
        workspace[8 * stride + index] = workspace[index];
        workspace[12 * stride + index] = workspace[index];
    }
    for y in 0..MACROBLOCK_SIZE {
        workspace[(y + 1) * stride] = if column == 0 {
            LEFT_BORDER
        } else {
            left[y + 1]
        };
    }
    workspace[0] = if row == 0 {
        TOP_BORDER
    } else if column == 0 {
        LEFT_BORDER
    } else {
        left[0]
    };
    workspace
}

fn chroma_workspace(
    column: usize,
    row: usize,
    top: &[u8],
    left: &[u8; 1 + CHROMA_SIZE],
) -> [u8; CHROMA_WORKSPACE] {
    let stride = CHROMA_STRIDE;
    let mut workspace = [0u8; CHROMA_WORKSPACE];
    if row == 0 {
        workspace[1..stride].fill(TOP_BORDER);
    } else {
        workspace[1..stride].copy_from_slice(&top[column * CHROMA_SIZE..][..CHROMA_SIZE]);
    }
    for y in 0..CHROMA_SIZE {
        workspace[(y + 1) * stride] = if column == 0 {
            LEFT_BORDER
        } else {
            left[y + 1]
        };
    }
    workspace[0] = if row == 0 {
        TOP_BORDER
    } else if column == 0 {
        LEFT_BORDER
    } else {
        left[0]
    };
    workspace
}

fn add_residue(workspace: &mut [u8], residue: &[i32], y0: usize, x0: usize, stride: usize) {
    for (row, values) in residue.as_chunks::<4>().0.iter().enumerate() {
        let start = (y0 + row) * stride + x0;
        for (pixel, &value) in workspace[start..start + 4].iter_mut().zip(values) {
            *pixel = (value + i32::from(*pixel)).clamp(0, 255) as u8;
        }
    }
}

fn predict_vertical(workspace: &mut [u8], size: usize, stride: usize) {
    let (above, rest) = workspace.split_at_mut(stride);
    for row in rest.chunks_exact_mut(stride).take(size) {
        row[1..].copy_from_slice(&above[1..]);
    }
}

fn predict_horizontal(workspace: &mut [u8], size: usize, stride: usize) {
    for row in workspace.chunks_exact_mut(stride).skip(1).take(size) {
        let left = row[0];
        row[1..].fill(left);
    }
}

fn predict_dc(workspace: &mut [u8], size: usize, stride: usize, above: bool, left: bool) {
    let mut sum = 0u32;
    let mut shift = if size == CHROMA_SIZE { 2 } else { 3 };
    if left {
        for y in 0..size {
            sum += u32::from(workspace[(y + 1) * stride]);
        }
        shift += 1;
    }
    if above {
        sum += workspace[1..=size]
            .iter()
            .map(|&value| u32::from(value))
            .sum::<u32>();
        shift += 1;
    }
    let value = if !left && !above {
        DC_WITHOUT_EDGES
    } else {
        (sum + (1 << (shift - 1))) >> shift
    };
    for y in 0..size {
        workspace[1 + stride * (y + 1)..][..size].fill(value as u8);
    }
}

fn predict_true_motion(workspace: &mut [u8], size: usize, x0: usize, y0: usize, stride: usize) {
    let corner = i32::from(workspace[(y0 - 1) * stride + x0 - 1]);
    let above_start = (y0 - 1) * stride + x0;
    for y in 0..size {
        let row = (y0 + y) * stride;
        let left_minus_corner = i32::from(workspace[row + x0 - 1]) - corner;
        for x in 0..size {
            let above = i32::from(workspace[above_start + x]);
            workspace[row + x0 + x] = (left_minus_corner + above).clamp(0, 255) as u8;
        }
    }
}

const fn average3(left: u8, this: u8, right: u8) -> u8 {
    ((left as u16 + 2 * this as u16 + right as u16 + 2) >> 2) as u8
}

const fn average2(this: u8, right: u8) -> u8 {
    ((this as u16 + right as u16 + 1) >> 1) as u8
}

fn predict_subblocks(
    workspace: &mut [u8],
    stride: usize,
    modes: &[IntraMode; 16],
    coefficients: &[i32; MACROBLOCK_COEFFICIENTS],
) {
    for y in 0..SUBBLOCK_SIZE {
        for x in 0..SUBBLOCK_SIZE {
            let index = x + y * SUBBLOCK_SIZE;
            let y0 = y * SUBBLOCK_SIZE + 1;
            let x0 = x * SUBBLOCK_SIZE + 1;
            predict_subblock(workspace, modes[index], x0, y0, stride);
            let residue = &coefficients[index * BLOCK_COEFFICIENTS..][..BLOCK_COEFFICIENTS];
            add_residue(workspace, residue, y0, x0, stride);
        }
    }
}

fn predict_subblock(workspace: &mut [u8], mode: IntraMode, x0: usize, y0: usize, stride: usize) {
    let top_row = (y0 - 1) * stride + x0;
    let top: [u8; 8] = core::array::from_fn(|index| workspace[top_row + index]);
    let corner = workspace[top_row - 1];
    let left: [u8; 4] = core::array::from_fn(|index| workspace[(y0 + index) * stride + x0 - 1]);
    let edge = [
        left[3], left[2], left[1], left[0], corner, top[0], top[1], top[2], top[3],
    ];
    let mut block = [[0u8; 4]; 4];
    match mode {
        IntraMode::TrueMotion => {
            predict_true_motion(workspace, SUBBLOCK_SIZE, x0, y0, stride);
            return;
        }
        IntraMode::Vertical => {
            let row = [
                average3(corner, top[0], top[1]),
                average3(top[0], top[1], top[2]),
                average3(top[1], top[2], top[3]),
                average3(top[2], top[3], top[4]),
            ];
            block = [row; 4];
        }
        IntraMode::Horizontal => {
            let values = [
                average3(corner, left[0], left[1]),
                average3(left[0], left[1], left[2]),
                average3(left[1], left[2], left[3]),
                average3(left[2], left[3], left[3]),
            ];
            for (row, value) in block.iter_mut().zip(values) {
                *row = [value; 4];
            }
        }
        IntraMode::Dc => {
            let sum = top[..4]
                .iter()
                .chain(left.iter())
                .map(|&value| u32::from(value))
                .sum::<u32>();
            block = [[((sum + 4) >> 3) as u8; 4]; 4];
        }
        IntraMode::LeftDown => {
            let values = [
                average3(top[0], top[1], top[2]),
                average3(top[1], top[2], top[3]),
                average3(top[2], top[3], top[4]),
                average3(top[3], top[4], top[5]),
                average3(top[4], top[5], top[6]),
                average3(top[5], top[6], top[7]),
                average3(top[6], top[7], top[7]),
            ];
            for (offset, row) in block.iter_mut().enumerate() {
                row.copy_from_slice(&values[offset..offset + 4]);
            }
        }
        IntraMode::RightDown => {
            let values: [u8; 7] = core::array::from_fn(|index| {
                average3(edge[index], edge[index + 1], edge[index + 2])
            });
            for (offset, row) in block.iter_mut().enumerate() {
                row.copy_from_slice(&values[3 - offset..7 - offset]);
            }
        }
        IntraMode::VerticalRight => block = predict_vertical_right(edge),
        IntraMode::VerticalLeft => block = predict_vertical_left(top),
        IntraMode::HorizontalDown => block = predict_horizontal_down(edge),
        IntraMode::HorizontalUp => block = predict_horizontal_up(left),
    }
    for (offset, row) in block.iter().enumerate() {
        let start = (y0 + offset) * stride + x0;
        workspace[start..start + 4].copy_from_slice(row);
    }
}

const fn predict_vertical_right(edge: [u8; 9]) -> [[u8; 4]; 4] {
    let mut block = [[0u8; 4]; 4];
    let [_, e1, e2, e3, e4, e5, e6, e7, e8] = edge;
    block[3][0] = average3(e1, e2, e3);
    block[2][0] = average3(e2, e3, e4);
    block[3][1] = average3(e3, e4, e5);
    block[1][0] = average3(e3, e4, e5);
    block[2][1] = average2(e4, e5);
    block[0][0] = average2(e4, e5);
    block[3][2] = average3(e4, e5, e6);
    block[1][1] = average3(e4, e5, e6);
    block[2][2] = average2(e5, e6);
    block[0][1] = average2(e5, e6);
    block[3][3] = average3(e5, e6, e7);
    block[1][2] = average3(e5, e6, e7);
    block[2][3] = average2(e6, e7);
    block[0][2] = average2(e6, e7);
    block[1][3] = average3(e6, e7, e8);
    block[0][3] = average2(e7, e8);
    block
}

const fn predict_vertical_left(top: [u8; 8]) -> [[u8; 4]; 4] {
    let mut block = [[0u8; 4]; 4];
    let [a0, a1, a2, a3, a4, a5, a6, a7] = top;
    block[0][0] = average2(a0, a1);
    block[1][0] = average3(a0, a1, a2);
    block[2][0] = average2(a1, a2);
    block[0][1] = average2(a1, a2);
    block[1][1] = average3(a1, a2, a3);
    block[3][0] = average3(a1, a2, a3);
    block[2][1] = average2(a2, a3);
    block[0][2] = average2(a2, a3);
    block[3][1] = average3(a2, a3, a4);
    block[1][2] = average3(a2, a3, a4);
    block[2][2] = average2(a3, a4);
    block[0][3] = average2(a3, a4);
    block[3][2] = average3(a3, a4, a5);
    block[1][3] = average3(a3, a4, a5);
    block[2][3] = average3(a4, a5, a6);
    block[3][3] = average3(a5, a6, a7);
    block
}

const fn predict_horizontal_down(edge: [u8; 9]) -> [[u8; 4]; 4] {
    let mut block = [[0u8; 4]; 4];
    let [e0, e1, e2, e3, e4, e5, e6, e7, _] = edge;
    block[3][0] = average2(e0, e1);
    block[3][1] = average3(e0, e1, e2);
    block[2][0] = average2(e1, e2);
    block[3][2] = average2(e1, e2);
    block[2][1] = average3(e1, e2, e3);
    block[3][3] = average3(e1, e2, e3);
    block[2][2] = average2(e2, e3);
    block[1][0] = average2(e2, e3);
    block[2][3] = average3(e2, e3, e4);
    block[1][1] = average3(e2, e3, e4);
    block[1][2] = average2(e3, e4);
    block[0][0] = average2(e3, e4);
    block[1][3] = average3(e3, e4, e5);
    block[0][1] = average3(e3, e4, e5);
    block[0][2] = average3(e4, e5, e6);
    block[0][3] = average3(e5, e6, e7);
    block
}

const fn predict_horizontal_up(left: [u8; 4]) -> [[u8; 4]; 4] {
    let mut block = [[0u8; 4]; 4];
    let [l0, l1, l2, l3] = left;
    block[0][0] = average2(l0, l1);
    block[0][1] = average3(l0, l1, l2);
    block[0][2] = average2(l1, l2);
    block[1][0] = average2(l1, l2);
    block[0][3] = average3(l1, l2, l3);
    block[1][1] = average3(l1, l2, l3);
    block[1][2] = average2(l2, l3);
    block[2][0] = average2(l2, l3);
    block[1][3] = average3(l2, l3, l3);
    block[2][1] = average3(l2, l3, l3);
    block[2][2] = l3;
    block[2][3] = l3;
    block[3] = [l3; 4];
    block
}
