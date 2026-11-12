use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::error::{ColorSpace, ComponentPosition, JpegError};
use crate::huffman::HuffmanTable;
use crate::marker::Marker;
use crate::reader::ByteReader;
use crate::upsample::SampleRatio;
use crate::{Color, Header};

pub(crate) const COMPONENTS_MAX: usize = 4;
const ICC_SEQUENCE_SLOTS: usize = 256;
const ICC_CHUNKS_LIMIT: usize = 255;
const START_OF_IMAGE: u16 = 0xFFD8;

pub(crate) struct IccChunk {
    sequence: u8,
    count: u8,
    data: Vec<u8>,
}

impl IccChunk {
    pub(crate) const fn new(sequence: u8, count: u8, data: Vec<u8>) -> Self {
        Self {
            sequence,
            count,
            data,
        }
    }
}

pub(crate) struct Component {
    pub(crate) position: ComponentPosition,
    pub(crate) identifier: u8,
    pub(crate) vertical_sample: usize,
    pub(crate) horizontal_sample: usize,
    pub(crate) dc_table: usize,
    pub(crate) ac_table: usize,
    pub(crate) quantization_number: u8,
    pub(crate) quantization: [i32; 64],
    pub(crate) dc_prediction: i32,
    pub(crate) width_stride: usize,
    pub(crate) needed: bool,
    pub(crate) samples: Vec<i16>,
    pub(crate) upsampled: Vec<i16>,
    pub(crate) row_up: Vec<i16>,
    pub(crate) row: Vec<i16>,
    pub(crate) first_row_upsampled: Vec<i16>,
    pub(crate) idct_position: usize,
    pub(crate) sample_ratio: SampleRatio,
    pub(crate) row_factor: usize,
}

impl Component {
    pub(crate) fn new(bytes: [u8; 3], index: u8) -> Result<Self, JpegError> {
        let position = match index {
            0 => ComponentPosition::Y,
            1 => ComponentPosition::Cb,
            2 => ComponentPosition::Cr,
            3 => ComponentPosition::Q,
            _ => return Err(JpegError::ComponentPosition(index)),
        };
        let horizontal_sample = usize::from(bytes[1] >> 4);
        let vertical_sample = usize::from(bytes[1] & 0x0F);
        let quantization_number = bytes[2];
        if usize::from(quantization_number) >= COMPONENTS_MAX {
            return Err(JpegError::QuantizationNumber(quantization_number));
        }
        if !horizontal_sample.is_power_of_two() {
            return Err(JpegError::HorizontalSample(horizontal_sample));
        }
        if !vertical_sample.is_power_of_two() {
            return Err(JpegError::VerticalSample(vertical_sample));
        }
        Ok(Self {
            position,
            identifier: bytes[0],
            vertical_sample,
            horizontal_sample,
            dc_table: 0,
            ac_table: 0,
            quantization_number,
            quantization: [0; 64],
            dc_prediction: 0,
            width_stride: horizontal_sample,
            needed: true,
            samples: Vec::new(),
            upsampled: Vec::new(),
            row_up: Vec::new(),
            row: Vec::new(),
            first_row_upsampled: Vec::new(),
            idct_position: 0,
            sample_ratio: SampleRatio::None,
            row_factor: 1,
        })
    }
}

pub(crate) struct Decoder<'a> {
    pub(crate) reader: ByteReader<'a>,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) component_count: u8,
    pub(crate) quantization_tables: [Option<[i32; 64]>; COMPONENTS_MAX],
    pub(crate) dc_tables: [Option<Box<HuffmanTable>>; COMPONENTS_MAX],
    pub(crate) ac_tables: [Option<Box<HuffmanTable>>; COMPONENTS_MAX],
    pub(crate) components: Vec<Component>,
    pub(crate) horizontal_max: usize,
    pub(crate) vertical_max: usize,
    pub(crate) mcu_columns: usize,
    pub(crate) mcu_rows: usize,
    pub(crate) interleaved: bool,
    pub(crate) sample_ratio: SampleRatio,
    pub(crate) input: ColorSpace,
    pub(crate) output: ColorSpace,
    pub(crate) progressive: bool,
    pub(crate) spectral_start: u8,
    pub(crate) spectral_end: u8,
    pub(crate) successive_high: u8,
    pub(crate) successive_low: u8,
    pub(crate) scan_components: u8,
    pub(crate) scan_order: [usize; COMPONENTS_MAX],
    pub(crate) restart_interval: usize,
    pub(crate) todo: usize,
    pub(crate) seen_frame: bool,
    pub(crate) exif: Option<Vec<u8>>,
    pub(crate) icc_chunks: Vec<IccChunk>,
    pub(crate) motion_jpeg: bool,
    pub(crate) row_factor: usize,
}

impl<'a> Decoder<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self {
            reader: ByteReader::new(bytes),
            width: 0,
            height: 0,
            component_count: 0,
            quantization_tables: [None; COMPONENTS_MAX],
            dc_tables: [None, None, None, None],
            ac_tables: [None, None, None, None],
            components: Vec::new(),
            horizontal_max: 1,
            vertical_max: 1,
            mcu_columns: 0,
            mcu_rows: 0,
            interleaved: false,
            sample_ratio: SampleRatio::None,
            input: ColorSpace::YCbCr,
            output: ColorSpace::Rgb,
            progressive: false,
            spectral_start: 0,
            spectral_end: 0,
            successive_high: 0,
            successive_low: 0,
            scan_components: 0,
            scan_order: [0; COMPONENTS_MAX],
            restart_interval: 0,
            todo: 0x7FFF_FFFF,
            seen_frame: false,
            exif: None,
            icc_chunks: Vec::new(),
            motion_jpeg: false,
            row_factor: 1,
        }
    }

    pub(crate) fn header(&self) -> Header {
        Header {
            width: u32::from(self.width),
            height: u32::from(self.height),
            color: match self.output {
                ColorSpace::Luma => Color::Luma,
                _ => Color::Rgb,
            },
        }
    }

    pub(crate) fn exif(&self) -> Option<Vec<u8>> {
        self.exif.clone()
    }

    pub(crate) fn icc_profile(&self) -> Option<Vec<u8>> {
        let count = self.icc_chunks.len();
        if count == 0 || count >= ICC_CHUNKS_LIMIT {
            return None;
        }
        let mut slots: [Option<&IccChunk>; ICC_SEQUENCE_SLOTS] = [None; ICC_SEQUENCE_SLOTS];
        for chunk in &self.icc_chunks {
            if usize::from(chunk.count) != count || chunk.sequence == 0 {
                return None;
            }
            let slot = &mut slots[usize::from(chunk.sequence)];
            if slot.is_some() {
                return None;
            }
            *slot = Some(chunk);
        }
        let mut profile = Vec::new();
        for slot in &slots[1..=count] {
            profile.extend_from_slice(&(*slot)?.data);
        }
        Some(profile)
    }

    pub(crate) fn parse_headers(&mut self) -> Result<(), JpegError> {
        let magic = self.reader.read_u16().ok_or(JpegError::NoMoreBytes)?;
        if magic != START_OF_IMAGE {
            return Err(JpegError::IllegalMagicBytes(magic));
        }
        let mut last_byte = 0_u8;
        loop {
            let mut byte = self.reader.read_u8().ok_or(JpegError::NoMoreBytes)?;
            if (byte == 0xFF || byte == 0) && last_byte == 0xFF {
                while byte == 0xFF || byte == 0 {
                    last_byte = byte;
                    byte = self.reader.read_u8().ok_or(JpegError::NoMoreBytes)?;
                }
            }
            if last_byte == 0xFF {
                if let Some(marker) = Marker::from_byte(byte) {
                    self.parse_marker(marker)?;
                    if marker == Marker::StartOfScan {
                        return Ok(());
                    }
                } else {
                    let length = self.reader.read_u16().ok_or(JpegError::NoMoreBytes)?;
                    if length < 2 {
                        return Err(JpegError::UnknownMarkerLength(length));
                    }
                    self.reader.skip(usize::from(length - 2));
                }
            }
            last_byte = byte;
        }
    }

    pub(crate) fn parse_marker(&mut self, marker: Marker) -> Result<(), JpegError> {
        match marker {
            Marker::StartOfFrame(number) => {
                if number == 2 {
                    self.progressive = true;
                }
                self.parse_frame()
            }
            Marker::Application(0) => self.parse_app0(),
            Marker::Application(1) => self.parse_app1(),
            Marker::Application(2) => self.parse_app2(),
            Marker::DefineQuantizationTable => self.parse_quantization(),
            Marker::DefineHuffmanTable => self.parse_huffman(),
            Marker::StartOfScan => self.parse_scan(),
            Marker::EndOfImage => Err(JpegError::PrematureEndOfImage),
            Marker::DefineArithmeticConditioning | Marker::DefineNumberOfLines => {
                Err(JpegError::UnsupportedMarker(marker))
            }
            Marker::DefineRestartInterval => {
                if self.reader.read_u16().ok_or(JpegError::NoMoreBytes)? != 4 {
                    return Err(JpegError::RestartIntervalLength);
                }
                self.restart_interval =
                    usize::from(self.reader.read_u16().ok_or(JpegError::NoMoreBytes)?);
                self.todo = self.restart_interval;
                Ok(())
            }
            Marker::Application(13) => self.parse_app13(),
            Marker::Application(14) => self.parse_app14(),
            Marker::Application(_)
            | Marker::Comment
            | Marker::StartOfImage
            | Marker::Restart(_) => {
                let length = self.reader.read_u16().ok_or(JpegError::NoMoreBytes)?;
                if length < 2 {
                    return Err(JpegError::MarkerLength(length));
                }
                self.reader.skip(usize::from(length - 2));
                Ok(())
            }
        }
    }
}
