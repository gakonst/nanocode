use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::color::convert_row;
use crate::decoder::{Component, Decoder};
use crate::error::{ColorSpace, ComponentPosition, JpegError};
use crate::huffman::HuffmanTable;
use crate::upsample::SampleRatio;

const DC_LUMA_COUNTS: [u8; 17] = [0, 0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_CHROMA_COUNTS: [u8; 17] = [0, 0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_VALUES: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const AC_LUMA_COUNTS: [u8; 17] = [0, 0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7D];
const AC_CHROMA_COUNTS: [u8; 17] = [0, 0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
pub(crate) const AC_LUMA_VALUES: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08, 0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7,
    0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5,
    0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2,
    0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8,
    0xF9, 0xFA,
];
pub(crate) const AC_CHROMA_VALUES: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xA1, 0xB1, 0xC1, 0x09, 0x23, 0x33, 0x52, 0xF0,
    0x15, 0x62, 0x72, 0xD1, 0x0A, 0x16, 0x24, 0x34, 0xE1, 0x25, 0xF1, 0x17, 0x18, 0x19, 0x1A, 0x26,
    0x27, 0x28, 0x29, 0x2A, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5,
    0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3,
    0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA,
    0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8,
    0xF9, 0xFA,
];

pub(crate) fn zeroed(factors: &[usize]) -> Result<Vec<i16>, JpegError> {
    let length = factors
        .iter()
        .try_fold(1_usize, |product, &factor| product.checked_mul(factor))
        .ok_or(JpegError::Allocation)?;
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(length)
        .map_err(|_| JpegError::Allocation)?;
    buffer.resize(length, 0);
    Ok(buffer)
}

pub(crate) const fn padded_width(width: usize, ratio: SampleRatio) -> usize {
    match ratio {
        SampleRatio::None | SampleRatio::Vertical => width.div_ceil(8) * 8,
        SampleRatio::Horizontal | SampleRatio::HorizontalVertical => width.div_ceil(16) * 16,
        SampleRatio::Generic(horizontal, _) => {
            let unit = horizontal * 8;
            (width + unit.saturating_sub(1)) / unit * unit
        }
    }
}

const fn ratio_for(horizontal: usize, vertical: usize) -> SampleRatio {
    match (horizontal, vertical) {
        (1, 1) => SampleRatio::None,
        (1, 2) => SampleRatio::Vertical,
        (2, 1) => SampleRatio::Horizontal,
        (2, 2) => SampleRatio::HorizontalVertical,
        (horizontal, vertical) => SampleRatio::Generic(horizontal, vertical),
    }
}

impl Decoder<'_> {
    pub(crate) fn setup_component_params(&mut self) -> Result<(), JpegError> {
        if self.components.len() == 3 && self.input == ColorSpace::Cmyk {
            self.input = ColorSpace::Rgb;
        }
        let width = usize::from(self.width);
        let height = usize::from(self.height);
        for component in &mut self.components {
            self.horizontal_max = self.horizontal_max.max(component.horizontal_sample);
            self.vertical_max = self.vertical_max.max(component.vertical_sample);
            let mcu_width = self.horizontal_max * 8;
            let mcu_height = self.vertical_max * 8;
            self.mcu_columns = width.div_ceil(mcu_width);
            self.mcu_rows = height.div_ceil(mcu_height);
            if self.horizontal_max != 1 || self.vertical_max != 1 {
                self.interleaved = true;
            }
            component.quantization = self.quantization_tables
                [usize::from(component.quantization_number)]
            .ok_or(JpegError::MissingQuantizationTable(component.position))?;
            component.width_stride *= self.mcu_columns * 8;
        }
        let luma_sampled = self
            .components
            .iter()
            .find(|component| component.position == ComponentPosition::Y)
            .is_some_and(|luma| luma.horizontal_sample == 2 || luma.vertical_sample == 2);
        if luma_sampled {
            for component in &mut self.components {
                if component.position != ComponentPosition::Y
                    && (component.horizontal_sample != 1 || component.vertical_sample != 1)
                {
                    component.row_factor = 2;
                }
            }
        }
        if self.motion_jpeg {
            self.fill_motion_jpeg_tables()?;
        }
        if self.input.components() > self.components.len() {
            if self.input == ColorSpace::Ycck {
                self.input = ColorSpace::YCbCr;
            } else {
                return Err(JpegError::ComponentCount {
                    expected: self.input.components(),
                    found: self.components.len(),
                });
            }
        }
        Ok(())
    }

    fn fill_motion_jpeg_tables(&mut self) -> Result<(), JpegError> {
        if self.dc_tables[0].is_none() {
            self.dc_tables[0] = Some(Box::new(HuffmanTable::from_symbols(
                &DC_LUMA_COUNTS,
                &DC_VALUES,
                true,
            )?));
        }
        if self.dc_tables[1].is_none() {
            self.dc_tables[1] = Some(Box::new(HuffmanTable::from_symbols(
                &DC_CHROMA_COUNTS,
                &DC_VALUES,
                true,
            )?));
        }
        if self.ac_tables[0].is_none() {
            self.ac_tables[0] = Some(Box::new(HuffmanTable::from_symbols(
                &AC_LUMA_COUNTS,
                &AC_LUMA_VALUES,
                false,
            )?));
        }
        if self.ac_tables[1].is_none() {
            self.ac_tables[1] = Some(Box::new(HuffmanTable::from_symbols(
                &AC_CHROMA_COUNTS,
                &AC_CHROMA_VALUES,
                false,
            )?));
        }
        Ok(())
    }

    pub(crate) fn set_upsampling(&mut self) -> Result<(), JpegError> {
        if self.horizontal_max == 1 && self.vertical_max == 1 {
            return Ok(());
        }
        self.sample_ratio = ratio_for(self.horizontal_max, self.vertical_max);
        for component in &mut self.components {
            component.sample_ratio = ratio_for(
                self.horizontal_max / component.horizontal_sample,
                self.vertical_max / component.vertical_sample,
            );
            let row = component.width_stride * component.vertical_sample;
            let sample = component.sample_ratio.sample();
            component.row = zeroed(&[row])?;
            component.row_up = zeroed(&[row])?;
            component.first_row_upsampled = zeroed(&[row, sample])?;
            component.first_row_upsampled.fill(128);
            component.upsampled =
                zeroed(&[component.width_stride, sample, component.row_factor, 8])?;
        }
        Ok(())
    }

    pub(crate) fn reset_params(&mut self) {
        self.horizontal_max = 1;
        self.output = ColorSpace::Luma;
        self.vertical_max = 1;
        self.sample_ratio = SampleRatio::None;
        self.interleaved = false;
        let width_stride = usize::from(self.width).div_ceil(8) * 8;
        let first = &mut self.components[0];
        first.vertical_sample = 1;
        first.width_stride = width_stride;
        first.horizontal_sample = 1;
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn post_process(
        &mut self,
        pixels: &mut [u8],
        row: usize,
        mcu_height: usize,
        width: usize,
        padded_width: usize,
        pixels_written: &mut usize,
        scratch: &mut [i16],
    ) -> Result<(), JpegError> {
        let output_components = self.output.components();
        let vertically_sampled = self.components.iter().any(|component| {
            matches!(
                component.sample_ratio,
                SampleRatio::HorizontalVertical | SampleRatio::Vertical
            )
        });
        let mut component_count = self.components.len();
        if output_components < component_count && self.output == ColorSpace::Luma {
            component_count = output_components;
        }
        let mut rows = RowWriter {
            pixels,
            written: *pixels_written,
            row_bytes: width * output_components,
            component_count,
            input: self.input,
            output: self.output,
            width,
            padded_width,
        };
        if self.interleaved && self.output != ColorSpace::Luma {
            for component in &mut self.components {
                upsample_component(component, mcu_height, row, scratch, vertically_sampled)?;
            }
            if vertically_sampled {
                if row > 0 {
                    let samples =
                        planes(&self.components, |component| &component.first_row_upsampled);
                    rows.write(self.row_factor * self.vertical_max, &samples)?;
                }
                for component in &mut self.components {
                    if component.sample_ratio != SampleRatio::Horizontal {
                        let size = component.vertical_sample
                            * component.width_stride
                            * component.sample_ratio.sample();
                        let last = component
                            .samples
                            .rchunks_exact(size)
                            .next()
                            .ok_or(JpegError::LayoutOutOfBounds)?;
                        if last.len() != component.first_row_upsampled.len() {
                            return Err(JpegError::LayoutOutOfBounds);
                        }
                        component.first_row_upsampled.copy_from_slice(last);
                    }
                }
            }
            let samples = planes(&self.components, |component| {
                if component.sample_ratio == SampleRatio::None {
                    &component.samples
                } else {
                    &component.upsampled
                }
            });
            let last_considered = vertically_sampled && row != mcu_height.saturating_sub(1);
            let count = (8 - usize::from(last_considered)) * self.row_factor * self.vertical_max;
            rows.write(count, &samples)?;
        } else {
            let samples = planes(&self.components, |component| &component.samples);
            let count = match self.sample_ratio {
                SampleRatio::Generic(_, vertical) => 8 * vertical * self.row_factor,
                _ => 8 * self.row_factor,
            };
            rows.write(count, &samples)?;
        }
        *pixels_written = rows.written;
        Ok(())
    }
}

fn planes<'a>(
    components: &'a [Component],
    select: impl Fn(&'a Component) -> &'a Vec<i16>,
) -> [&'a [i16]; 4] {
    let mut samples: [&[i16]; 4] = [&[]; 4];
    for (plane, component) in samples.iter_mut().zip(components) {
        *plane = select(component);
    }
    samples
}

struct RowWriter<'p> {
    pixels: &'p mut [u8],
    written: usize,
    row_bytes: usize,
    component_count: usize,
    input: ColorSpace,
    output: ColorSpace,
    width: usize,
    padded_width: usize,
}

impl RowWriter<'_> {
    fn write(&mut self, count: usize, samples: &[&[i16]; 4]) -> Result<(), JpegError> {
        let padded = self.padded_width;
        for (index, target) in self.pixels[self.written..]
            .chunks_exact_mut(self.row_bytes)
            .take(count)
            .enumerate()
        {
            let mut rows: [&[i16]; 4] = [&[]; 4];
            for (row, plane) in rows.iter_mut().zip(samples).take(self.component_count) {
                *row = plane
                    .get(index * padded..(index + 1) * padded)
                    .ok_or(JpegError::LayoutOutOfBounds)?;
            }
            convert_row(&rows, self.input, self.output, target, self.width, padded)?;
            self.written += self.row_bytes;
        }
        Ok(())
    }
}

fn upsample_component(
    component: &mut Component,
    mcu_height: usize,
    row: usize,
    scratch: &mut [i16],
    vertically_sampled: bool,
) -> Result<(), JpegError> {
    let ratio = component.sample_ratio;
    let stride = component.width_stride;
    match ratio {
        SampleRatio::Vertical | SampleRatio::HorizontalVertical => {
            upsample_vertical_rows(component, mcu_height, row, scratch)
        }
        SampleRatio::Horizontal => {
            if component.samples.len() * 2 != component.upsampled.len() {
                return Err(JpegError::LayoutOutOfBounds);
            }
            if vertically_sampled {
                let length = component.first_row_upsampled.len();
                let last = component
                    .upsampled
                    .rchunks_exact(length)
                    .next()
                    .ok_or(JpegError::LayoutOutOfBounds)?;
                component.first_row_upsampled.copy_from_slice(last);
            }
            for (input, output) in component
                .samples
                .chunks_exact(stride)
                .zip(component.upsampled.chunks_exact_mut(stride * 2))
            {
                ratio.apply(input, &[], &[], &mut [], output)?;
            }
            Ok(())
        }
        SampleRatio::Generic(horizontal, vertical) => {
            for (input, output) in component.samples.chunks_exact(stride).zip(
                component
                    .upsampled
                    .chunks_exact_mut(stride * horizontal * vertical),
            ) {
                ratio.apply(input, &[], &[], &mut [], output)?;
            }
            Ok(())
        }
        SampleRatio::None => Ok(()),
    }
}

fn upsample_vertical_rows(
    component: &mut Component,
    mcu_height: usize,
    row: usize,
    scratch: &mut [i16],
) -> Result<(), JpegError> {
    let ratio = component.sample_ratio;
    let width_stride = component.width_stride;
    let written = width_stride * ratio.sample();
    let samples = &component.samples;
    let span =
        |start: usize, end: usize| samples.get(start..end).ok_or(JpegError::LayoutOutOfBounds);
    if row > 0 {
        let destination = component
            .first_row_upsampled
            .get_mut(..written)
            .ok_or(JpegError::LayoutOutOfBounds)?;
        ratio.apply(
            &component.row,
            &component.row_up,
            span(0, width_stride)?,
            scratch,
            destination,
        )?;
    }
    let stride = width_stride * component.vertical_sample;
    let stop = samples.len() / width_stride;
    let mut destination_start = 0;
    for (position, current) in samples.chunks_exact(width_stride).enumerate() {
        let (up, down): (&[i16], &[i16]) = if row == 0 && position == 0 {
            (span(0, stride)?, span(stride, 2 * stride)?)
        } else if row > 0 && position == 0 {
            (&component.row, span(stride, 2 * stride)?)
        } else if row == mcu_height.saturating_sub(1) && position == stop - 1 {
            (
                span((position - 1) * stride, position * stride)?,
                span(position * stride, (position + 1) * stride)?,
            )
        } else if position > 0 && position < stop - 1 {
            (
                span((position - 1) * stride, position * stride)?,
                span((position + 1) * stride, (position + 2) * stride)?,
            )
        } else {
            assert_eq!(position, stop - 1);
            let previous = span((position - 1) * stride, position * stride)?;
            if component.row_up.len() != previous.len() || component.row.len() != current.len() {
                return Err(JpegError::LayoutOutOfBounds);
            }
            component.row_up.copy_from_slice(previous);
            component.row.copy_from_slice(current);
            break;
        };
        let destination = component
            .upsampled
            .get_mut(destination_start..destination_start + written)
            .ok_or(JpegError::LayoutOutOfBounds)?;
        destination_start += written;
        ratio.apply(current, up, down, scratch, destination)?;
    }
    Ok(())
}
