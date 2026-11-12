use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::bitstream::UNZIGZAG;
use crate::decoder::{COMPONENTS_MAX, Component, Decoder, IccChunk};
use crate::error::{ColorSpace, JpegError};
use crate::huffman::HuffmanTable;

const HUFFMAN_COUNTS_BYTES: i32 = 16;
const HUFFMAN_SYMBOLS_MAX: i32 = 256;
const SAMPLE_PRECISION: u8 = 8;
const SCAN_COMPONENTS_MAX: u8 = 4;
const SPECTRAL_MAX: u8 = 63;
const SUCCESSIVE_MAX: u8 = 13;
const MOTION_JPEG_TAG: &[u8; 5] = b"AVI1\0";
const EXIF_TAG: &[u8; 6] = b"Exif\0\0";
const ICC_TAG: &[u8; 12] = b"ICC_PROFILE\0";
const ADOBE_TAG: &[u8; 5] = b"Adobe";
const PHOTOSHOP_TAG: &[u8; 13] = b"Photoshop 3.0";
const ADOBE_SEGMENT_BYTES_MIN: usize = 14;

impl Decoder<'_> {
    fn read_u8(&mut self) -> Result<u8, JpegError> {
        self.reader.read_u8().ok_or(JpegError::NoMoreBytes)
    }

    fn read_u16(&mut self) -> Result<u16, JpegError> {
        self.reader.read_u16().ok_or(JpegError::NoMoreBytes)
    }

    pub(crate) fn parse_huffman(&mut self) -> Result<(), JpegError> {
        let mut remaining = i32::from(
            self.read_u16()?
                .checked_sub(2)
                .ok_or(JpegError::HuffmanLength)?,
        );
        while remaining > HUFFMAN_COUNTS_BYTES {
            let information = self.read_u8()?;
            let class = (information >> 4) & 0x0F;
            let index = usize::from(information & 0x0F);
            if index >= COMPONENTS_MAX {
                return Err(JpegError::HuffmanIndex(index));
            }
            if class > 1 {
                return Err(JpegError::HuffmanClass(class));
            }
            let mut counts = [0_u8; 17];
            self.reader
                .read_exact(&mut counts[1..17])
                .ok_or(JpegError::ExhaustedData)?;
            remaining -= 1 + HUFFMAN_COUNTS_BYTES;
            let symbols: i32 = counts.iter().map(|&count| i32::from(count)).sum();
            if symbols > HUFFMAN_SYMBOLS_MAX {
                return Err(JpegError::HuffmanSymbolCount);
            }
            if symbols > remaining {
                return Err(JpegError::HuffmanTableLength {
                    symbols,
                    length: remaining,
                });
            }
            remaining -= symbols;
            let mut values = [0_u8; 256];
            self.reader
                .read_exact(&mut values[..symbols as usize])
                .ok_or(JpegError::ShortSymbols)?;
            let table = Box::new(HuffmanTable::new(&counts, values, class == 0)?);
            if class == 0 {
                self.dc_tables[index] = Some(table);
            } else {
                self.ac_tables[index] = Some(table);
            }
        }
        if remaining > 0 {
            return Err(JpegError::HuffmanDefinition);
        }
        Ok(())
    }

    pub(crate) fn parse_quantization(&mut self) -> Result<(), JpegError> {
        let mut remaining = self
            .read_u16()?
            .checked_sub(2)
            .ok_or(JpegError::QuantizationLength)?;
        while remaining > 0 {
            let information = self.read_u8()?;
            let precision = usize::from(information >> 4);
            let position = usize::from(information & 0x0F);
            let needed = 64 * (precision + 1) + 1;
            if needed as u16 > remaining {
                return Err(JpegError::QuantizationTableBytes {
                    left: remaining,
                    needed,
                });
            }
            let mut table = [0_i32; 64];
            match precision {
                0 => {
                    let mut values = [0_u8; 64];
                    self.reader
                        .read_exact(&mut values)
                        .ok_or(JpegError::ShortSymbols)?;
                    for (index, value) in values.into_iter().enumerate() {
                        table[UNZIGZAG[index]] = i32::from(value);
                    }
                }
                1 => {
                    for target in UNZIGZAG {
                        table[target] = i32::from(self.read_u16()?);
                    }
                }
                _ => return Err(JpegError::QuantizationPrecision(precision)),
            }
            remaining -= needed as u16;
            if position >= COMPONENTS_MAX {
                return Err(JpegError::QuantizationPosition(position));
            }
            self.quantization_tables[position] = Some(table);
        }
        Ok(())
    }

    pub(crate) fn parse_frame(&mut self) -> Result<(), JpegError> {
        if self.seen_frame {
            return Err(JpegError::SecondFrame);
        }
        let length = self.read_u16()?;
        let precision = self.read_u8()?;
        if precision != SAMPLE_PRECISION {
            return Err(JpegError::Precision(precision));
        }
        self.height = self.read_u16()?;
        self.width = self.read_u16()?;
        if self.width == 0 || self.height == 0 {
            return Err(JpegError::ZeroDimension);
        }
        let count = self.read_u8()?;
        if count == 0 {
            return Err(JpegError::ZeroComponents);
        }
        let expected = 8 + 3 * u16::from(count);
        if length != expected {
            return Err(JpegError::FrameLength { expected, length });
        }
        if count == 1 {
            self.input = ColorSpace::Luma;
            self.output = ColorSpace::Luma;
        }
        if count == 4 && self.input == ColorSpace::YCbCr {
            self.input = ColorSpace::Cmyk;
        }
        self.component_count = count;
        let mut components = Vec::with_capacity(usize::from(count));
        for index in 0..count {
            let mut bytes = [0_u8; 3];
            self.reader
                .read_exact(&mut bytes)
                .ok_or(JpegError::ShortComponent)?;
            components.push(Component::new(bytes, index)?);
        }
        self.seen_frame = true;
        self.components = components;
        Ok(())
    }

    pub(crate) fn parse_scan(&mut self) -> Result<(), JpegError> {
        let length = self.read_u16()?;
        let count = self.read_u8()?;
        self.scan_components = count;
        if length != 6 + 2 * u16::from(count) {
            return Err(JpegError::ScanLength(length));
        }
        if !(1..=SCAN_COMPONENTS_MAX).contains(&count) {
            return Err(JpegError::ScanComponents(count));
        }
        if self.component_count == 0 {
            return Err(JpegError::ScanBeforeFrame);
        }
        let mut seen = [None; SCAN_COMPONENTS_MAX as usize];
        for scan_index in 0..usize::from(count) {
            let identifier = self.read_u8()?;
            if seen.contains(&Some(identifier)) {
                return Err(JpegError::DuplicateScanComponent(identifier));
            }
            seen[scan_index] = Some(identifier);
            let tables = self.read_u8()?;
            let found = self
                .components
                .iter()
                .take(usize::from(self.component_count))
                .position(|component| component.identifier == identifier)
                .ok_or(JpegError::ScanComponentId {
                    id: identifier,
                    count: self.components.len(),
                })?;
            self.components[found].dc_table = usize::from((tables >> 4) & 0x0F);
            self.components[found].ac_table = usize::from(tables & 0x0F);
            self.scan_order[scan_index] = found;
        }
        self.spectral_start = self.read_u8()?;
        self.spectral_end = self.read_u8()?;
        let approximation = self.read_u8()?;
        self.successive_high = approximation >> 4;
        if self.spectral_end > SPECTRAL_MAX {
            return Err(JpegError::SpectralEnd(self.spectral_end));
        }
        if self.spectral_start > SPECTRAL_MAX {
            return Err(JpegError::SpectralStart(self.spectral_start));
        }
        if self.successive_high > SUCCESSIVE_MAX {
            return Err(JpegError::SuccessiveHigh {
                previous_low: self.successive_low,
            });
        }
        self.successive_low = approximation & 0x0F;
        if self.successive_low > SUCCESSIVE_MAX {
            return Err(JpegError::SuccessiveLow(self.successive_low));
        }
        Ok(())
    }

    pub(crate) fn parse_app0(&mut self) -> Result<(), JpegError> {
        let mut length = self.read_u16()?;
        if length < 2 {
            return Err(JpegError::MarkerLength(length));
        }
        if length > 5 && self.reader.has(MOTION_JPEG_TAG.len()) {
            let mut tag = [0_u8; 5];
            assert!(self.reader.read_exact(&mut tag).is_some());
            if &tag == MOTION_JPEG_TAG {
                self.motion_jpeg = true;
            }
            length -= 5;
        }
        self.reader.skip(usize::from(length.saturating_sub(2)));
        Ok(())
    }

    fn segment_length(&mut self) -> Result<usize, JpegError> {
        let length = usize::from(self.reader.read_u16_or_zero());
        if length < 2 || !self.reader.has(length - 2) {
            return Err(JpegError::ExhaustedData);
        }
        Ok(length)
    }

    pub(crate) fn parse_app1(&mut self) -> Result<(), JpegError> {
        let mut length = self.segment_length()? - 2;
        if length > EXIF_TAG.len() && self.reader.peek(EXIF_TAG.len()) == Some(EXIF_TAG) {
            self.reader.skip(EXIF_TAG.len());
            length -= EXIF_TAG.len();
            let exif = self
                .reader
                .peek(length)
                .expect("segment_length checked that the segment is in the input");
            self.exif = Some(exif.to_vec());
        }
        self.reader.skip(length);
        Ok(())
    }

    pub(crate) fn parse_app2(&mut self) -> Result<(), JpegError> {
        let mut length = self.segment_length()? - 2;
        if length > ICC_TAG.len() + 2 && self.reader.peek(ICC_TAG.len()) == Some(ICC_TAG) {
            length -= ICC_TAG.len();
            self.reader.skip(ICC_TAG.len());
            let sequence = self.reader.read_u8_or_zero();
            let count = self.reader.read_u8_or_zero();
            length -= 2;
            let data = self
                .reader
                .peek(length)
                .expect("segment_length checked that the segment is in the input");
            self.icc_chunks
                .push(IccChunk::new(sequence, count, data.to_vec()));
        }
        self.reader.skip(length);
        Ok(())
    }

    pub(crate) fn parse_app13(&mut self) -> Result<(), JpegError> {
        let mut length = usize::from(self.reader.read_u16_or_zero());
        if length < 2 {
            return Err(JpegError::App13Length);
        }
        length -= 2;
        if length > PHOTOSHOP_TAG.len()
            && self
                .reader
                .peek(PHOTOSHOP_TAG.len())
                .ok_or(JpegError::NoMoreBytes)?
                == PHOTOSHOP_TAG
        {
            self.reader.skip(PHOTOSHOP_TAG.len());
            length -= PHOTOSHOP_TAG.len();
            self.reader.peek(length).ok_or(JpegError::NoMoreBytes)?;
        }
        self.reader.skip(length);
        Ok(())
    }

    pub(crate) fn parse_app14(&mut self) -> Result<(), JpegError> {
        let mut length = self.segment_length()?;
        if length < ADOBE_SEGMENT_BYTES_MIN {
            return Err(JpegError::App14Length);
        }
        if self.reader.peek(ADOBE_TAG.len()) == Some(ADOBE_TAG) {
            self.reader.skip(6);
            self.reader.skip(5);
            let transform = self.reader.read_u8_or_zero();
            self.input = match transform {
                0 => ColorSpace::Cmyk,
                1 => ColorSpace::YCbCr,
                2 => ColorSpace::Ycck,
                _ => return Err(JpegError::AdobeTransform(transform)),
            };
            length = length.saturating_sub(ADOBE_SEGMENT_BYTES_MIN);
        } else {
            length = length.saturating_sub(2);
        }
        self.reader.skip(length);
        Ok(())
    }
}
