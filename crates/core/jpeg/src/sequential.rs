use alloc::vec::Vec;

use crate::bitstream::BitStream;
use crate::decoder::{COMPONENTS_MAX, Decoder};
use crate::error::{ColorSpace, JpegError, TableClass};
use crate::idct;
use crate::marker::Marker;
use crate::process::{padded_width, zeroed};
use crate::reader::ByteReader;
use crate::upsample::SampleRatio;

const OVERREAD_BYTES_MAX: usize = 37;

pub(crate) enum Continuation {
    Proceed,
    AnotherScan,
    Terminate,
}

pub(crate) fn next_marker(
    reader: &mut ByteReader<'_>,
    stream: &mut BitStream,
) -> Result<Marker, JpegError> {
    if let Some(marker) = stream.marker.take() {
        return Ok(marker);
    }
    while !reader.eof() {
        let byte = reader.read_u8().ok_or(JpegError::NoMoreBytes)?;
        if byte == 0xFF {
            let mut next = reader.read_u8().ok_or(JpegError::NoMoreBytes)?;
            while next == 0xFF {
                next = reader.read_u8().ok_or(JpegError::NoMoreBytes)?;
            }
            if next != 0 {
                return Marker::from_byte(next).ok_or(JpegError::UnknownMarker(next));
            }
        }
    }
    Err(JpegError::ExhaustedData)
}

impl Decoder<'_> {
    pub(crate) fn decode_into(&mut self, pixels: &mut [u8]) -> Result<(), JpegError> {
        if self.progressive {
            self.decode_progressive(pixels)
        } else {
            self.decode_sequential(pixels)
        }
    }

    fn check_tables(&self) -> Result<(), JpegError> {
        for component in &self.components {
            let tables = [
                (TableClass::Dc, self.dc_tables.get(component.dc_table)),
                (TableClass::Ac, self.ac_tables.get(component.ac_table)),
            ];
            for (class, slot) in tables {
                match slot {
                    None => {
                        return Err(JpegError::MissingHuffmanSlot {
                            class,
                            component: component.position,
                        });
                    }
                    Some(None) => {
                        return Err(JpegError::MissingHuffmanTable {
                            class,
                            component: component.position,
                        });
                    }
                    Some(Some(_)) => {}
                }
            }
        }
        Ok(())
    }

    pub(crate) const fn subsampled_luma_rows(&self) -> bool {
        self.interleaved
            && self.input.components() > 1
            && self.output.components() == 1
            && matches!(
                self.sample_ratio,
                SampleRatio::Vertical | SampleRatio::HorizontalVertical
            )
    }

    fn decode_sequential(&mut self, pixels: &mut [u8]) -> Result<(), JpegError> {
        self.setup_component_params()?;
        self.check_tables()?;
        let (mut mcu_width, mut mcu_height);
        if self.interleaved {
            self.set_upsampling()?;
            mcu_width = self.mcu_columns;
            mcu_height = self.mcu_rows;
        } else {
            mcu_width = usize::from(self.width.wrapping_add(7) / 8);
            mcu_height = usize::from(self.height.wrapping_add(7) / 8);
        }
        if self.subsampled_luma_rows() {
            mcu_height *= self.vertical_max;
            mcu_height /= self.horizontal_max;
            self.row_factor = 2;
        }
        if self.input == ColorSpace::Luma && self.interleaved {
            self.reset_params();
            mcu_width = usize::from(self.width.wrapping_add(7) / 8);
            mcu_height = usize::from(self.height.wrapping_add(7) / 8);
        }
        let width = usize::from(self.width);
        let padded = padded_width(width, self.sample_ratio);
        let component_count = self.components.len();
        let output_last = self.output.components() - 1;
        for (index, component) in self.components.iter_mut().enumerate() {
            if output_last.min(index) == index || component_count == 4 {
                component.samples =
                    zeroed(&[component.width_stride, component.vertical_sample, 8])?;
                component.needed = true;
            } else {
                component.needed = false;
            }
        }
        let all_in_first_scan = usize::from(self.scan_components) == component_count;
        let mut buffers: [Vec<i16>; COMPONENTS_MAX] = Default::default();
        if !all_in_first_scan {
            for (component, buffer) in self.components.iter().zip(&mut buffers) {
                *buffer = zeroed(&[
                    mcu_width,
                    component.vertical_sample,
                    component.horizontal_sample,
                    mcu_height,
                    64,
                ])?;
            }
        }
        let mut stream = BitStream::new(0, 0, 0);
        let mut block = [0_i32; 64];
        let mut pixels_written = 0;
        let mut scratch = zeroed(&[
            usize::from(self.interleaved),
            self.components[0].width_stride,
        ])?;
        'scans: loop {
            for row in 0..mcu_height {
                if stream.overread > OVERREAD_BYTES_MAX {
                    break;
                }
                let continuation = self.decode_mcu_row(
                    mcu_width,
                    row,
                    &mut block,
                    &mut stream,
                    &mut buffers,
                    !all_in_first_scan,
                )?;
                if all_in_first_scan {
                    self.post_process(
                        pixels,
                        row,
                        mcu_height,
                        width,
                        padded,
                        &mut pixels_written,
                        &mut scratch,
                    )?;
                }
                match continuation {
                    Continuation::Proceed => {}
                    Continuation::AnotherScan if all_in_first_scan => return Ok(()),
                    Continuation::AnotherScan => continue 'scans,
                    Continuation::Terminate => return Ok(()),
                }
            }
            break;
        }
        if !all_in_first_scan {
            self.finish_sequential(&buffers, pixels)?;
        }
        Ok(())
    }

    fn finish_sequential(
        &mut self,
        buffers: &[Vec<i16>; COMPONENTS_MAX],
        pixels: &mut [u8],
    ) -> Result<(), JpegError> {
        let mcu_height = self.mcu_rows;
        let width = usize::from(self.width);
        let padded = padded_width(width, self.sample_ratio);
        let mut scratch = zeroed(&[
            usize::from(self.interleaved),
            self.components[0].width_stride,
        ])?;
        let output_last = self.output.components() - 1;
        let four_channel = matches!(self.input, ColorSpace::Ycck | ColorSpace::Cmyk);
        for (index, component) in self.components.iter_mut().enumerate() {
            component.needed = output_last.min(index) == index || four_channel;
        }
        let mut pixels_written = 0;
        for row in 0..mcu_height {
            for (component, buffer) in self.components.iter_mut().zip(buffers) {
                if !component.needed {
                    continue;
                }
                let step = buffer.len() / mcu_height;
                let source = &buffer[row * step..(row + 1) * step];
                component
                    .samples
                    .get_mut(..step)
                    .ok_or(JpegError::LayoutOutOfBounds)?
                    .copy_from_slice(source);
            }
            self.post_process(
                pixels,
                row,
                mcu_height,
                width,
                padded,
                &mut pixels_written,
                &mut scratch,
            )?;
        }
        Ok(())
    }

    fn decode_mcu_row(
        &mut self,
        mcu_width: usize,
        row: usize,
        block: &mut [i32; 64],
        stream: &mut BitStream,
        buffers: &mut [Vec<i16>; COMPONENTS_MAX],
        into_buffers: bool,
    ) -> Result<Continuation, JpegError> {
        let order = self.scan_order;
        for column in 0..mcu_width {
            for &index in &order[..usize::from(self.scan_components)] {
                let component = &mut self.components[index];
                let dc_table = self.dc_tables[component.dc_table % COMPONENTS_MAX]
                    .as_deref()
                    .ok_or(JpegError::LayoutOutOfBounds)?;
                let ac_table = self.ac_tables[component.ac_table % COMPONENTS_MAX]
                    .as_deref()
                    .ok_or(JpegError::LayoutOutOfBounds)?;
                let stride = component.width_stride;
                let channel: &mut [i16] = if into_buffers {
                    let offset = row * stride * 8 * component.vertical_sample;
                    buffers[index]
                        .get_mut(offset..)
                        .ok_or(JpegError::LayoutOutOfBounds)?
                } else {
                    &mut component.samples
                };
                for vertical in 0..component.vertical_sample {
                    for horizontal in 0..component.horizontal_sample {
                        block.fill(0);
                        stream.decode_block(
                            &mut self.reader,
                            dc_table,
                            ac_table,
                            &component.quantization,
                            block,
                            &mut component.dc_prediction,
                        )?;
                        if component.needed {
                            let position = stride * vertical * 8
                                + (column * component.horizontal_sample + horizontal) * 8;
                            idct::transform(block, channel, position, stride)?;
                        }
                    }
                }
            }
            self.todo = self.todo.wrapping_sub(1);
            if self.todo == 0 {
                self.handle_restart_main(stream)?;
                continue;
            }
            if stream.marker.is_some() && stream.bits_left == 0 {
                break;
            }
        }
        let Some(marker) = stream.marker else {
            return Ok(Continuation::Proceed);
        };
        match marker {
            Marker::EndOfImage => {
                stream.marker = None;
                stream.seen_end_of_image = true;
                Ok(Continuation::Proceed)
            }
            Marker::Restart(_) => {
                if self.todo == 0 {
                    self.handle_restart(stream)?;
                }
                Ok(Continuation::Proceed)
            }
            Marker::StartOfScan => {
                self.parse_marker(marker)?;
                stream.reset();
                Ok(Continuation::AnotherScan)
            }
            _ => {
                self.parse_marker(marker)?;
                stream.reset();
                Ok(Continuation::Terminate)
            }
        }
    }

    pub(crate) fn handle_restart_main(&mut self, stream: &mut BitStream) -> Result<(), JpegError> {
        if self.todo != 0 {
            return Ok(());
        }
        stream.refill(&mut self.reader);
        if self.restart_interval != 0 && stream.marker.is_none() && !stream.seen_end_of_image {
            let marker = next_marker(&mut self.reader, stream)?;
            stream.marker = Some(marker);
        }
        self.handle_restart(stream)
    }

    pub(crate) fn handle_restart(&mut self, stream: &mut BitStream) -> Result<(), JpegError> {
        self.todo = self.restart_interval;
        match stream.marker {
            Some(Marker::Restart(_)) => {
                stream.reset();
                for component in &mut self.components {
                    component.dc_prediction = 0;
                }
                Ok(())
            }
            Some(Marker::EndOfImage) | None => Ok(()),
            Some(marker) => Err(JpegError::RestartMarker(marker)),
        }
    }
}
