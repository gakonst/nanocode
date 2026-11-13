use alloc::vec::Vec;

use crate::bitstream::BitStream;
use crate::decoder::{COMPONENTS_MAX, Decoder};
use crate::error::{ColorSpace, ComponentPosition, JpegError};
use crate::idct;
use crate::marker::Marker;
use crate::process::{padded_width, zeroed};
use crate::sequential::next_marker;

const SCANS_MAX: usize = 100;
const BLOCK_COEFFICIENTS: usize = 64;

fn zeroed_masks(count: usize) -> Result<Vec<u64>, JpegError> {
    let mut masks = Vec::new();
    masks
        .try_reserve_exact(count)
        .map_err(|_| JpegError::Allocation)?;
    masks.resize(count, 0);
    Ok(masks)
}

impl Decoder<'_> {
    pub(crate) fn decode_progressive(&mut self, pixels: &mut [u8]) -> Result<(), JpegError> {
        self.setup_component_params()?;
        if self.input == ColorSpace::Luma && self.interleaved {
            self.reset_params();
        }
        if self.interleaved {
            self.set_upsampling()?;
        }
        let (mut mcu_width, mut mcu_height) = if self.interleaved {
            (self.mcu_columns, self.mcu_rows)
        } else {
            (
                usize::from(self.width).div_ceil(8),
                usize::from(self.height).div_ceil(8),
            )
        };
        if self.subsampled_luma_rows() {
            mcu_height *= self.vertical_max;
            mcu_height /= self.horizontal_max;
            self.row_factor = 2;
        }
        mcu_width *= 64;
        let mut buffers: [Vec<i16>; COMPONENTS_MAX] = Default::default();
        let mut masks: [Vec<u64>; COMPONENTS_MAX] = Default::default();
        for ((component, buffer), mask) in self
            .components
            .iter()
            .zip(&mut buffers)
            .zip(&mut masks)
            .take(self.input.components())
        {
            *buffer = zeroed(&[
                mcu_width,
                component.vertical_sample,
                component.horizontal_sample,
                mcu_height,
            ])?;
            *mask = zeroed_masks(buffer.len() / BLOCK_COEFFICIENTS)?;
        }
        let mut stream =
            BitStream::new(self.successive_low, self.spectral_start, self.spectral_end);
        self.decode_scan(&mut stream, &mut buffers, &mut masks)?;
        let mut marker = stream.marker.take().ok_or(JpegError::MarkerMissing)?;
        let mut seen_scans = 1;
        while marker != Marker::EndOfImage {
            match marker {
                Marker::StartOfScan => {
                    self.parse_scan()?;
                    stream.update_progressive(
                        self.successive_low,
                        self.spectral_start,
                        self.spectral_end,
                    );
                    self.decode_scan(&mut stream, &mut buffers, &mut masks)?;
                    let Ok(next) = next_marker(&mut self.reader, &mut stream) else {
                        break;
                    };
                    marker = next;
                    seen_scans += 1;
                    if seen_scans > SCANS_MAX {
                        return Err(JpegError::TooManyScans(SCANS_MAX));
                    }
                    stream.reset();
                    continue;
                }
                Marker::Restart(_) => self.handle_restart(&mut stream)?,
                _ => self.parse_marker(marker)?,
            }
            match next_marker(&mut self.reader, &mut stream) {
                Ok(next) => marker = next,
                Err(_) => {
                    if matches!(marker, Marker::Restart(_)) && self.reader.eof() {
                        return Err(JpegError::RestartAtEnd);
                    }
                }
            }
        }
        self.finish_progressive(&buffers, pixels)
    }

    fn decode_scan(
        &mut self,
        stream: &mut BitStream,
        buffers: &mut [Vec<i16>; COMPONENTS_MAX],
        masks: &mut [Vec<u64>; COMPONENTS_MAX],
    ) -> Result<(), JpegError> {
        stream.reset();
        for component in &mut self.components {
            component.dc_prediction = 0;
        }
        self.todo = if self.restart_interval != 0 {
            self.restart_interval
        } else {
            usize::MAX
        };
        if usize::from(self.scan_components) > self.input.components() {
            return Err(JpegError::ScanCount {
                scans: self.scan_components,
                components: self.input.components(),
            });
        }
        if self.scan_components == 1 {
            self.decode_single_component_scan(stream, buffers, masks)
        } else {
            self.decode_interleaved_dc_scan(stream, buffers)
        }
    }

    fn decode_single_component_scan(
        &mut self,
        stream: &mut BitStream,
        buffers: &mut [Vec<i16>; COMPONENTS_MAX],
        masks: &mut [Vec<u64>; COMPONENTS_MAX],
    ) -> Result<(), JpegError> {
        if self.spectral_end != 0 && self.spectral_start == 0 {
            return Err(JpegError::MergedScan);
        }
        let index = self.scan_order[0];
        let Some(component) = self.components.get(index) else {
            return Err(JpegError::ScanComponentMissing(index));
        };
        let sampled_luma = component.position == ComponentPosition::Y
            && (component.vertical_sample != 1 || component.horizontal_sample != 1);
        let (mcu_width, mcu_height) = if sampled_luma || !self.interleaved {
            (
                usize::from(self.width.wrapping_add(7) / 8),
                usize::from(self.height.wrapping_add(7) / 8),
            )
        } else {
            (self.mcu_columns, self.mcu_rows)
        };
        let blocks_per_row = component.width_stride / 8;
        for row in 0..mcu_height {
            for column in 0..mcu_width {
                if self.spectral_start != 0
                    && self.successive_high == 0
                    && stream.end_of_band_run > 0
                {
                    stream.end_of_band_run -= 1;
                } else {
                    let start = 64 * (column + row * blocks_per_row);
                    let data: &mut [i16; 64] = buffers[index]
                        .get_mut(start..start + 64)
                        .ok_or(JpegError::LayoutOutOfBounds)?
                        .try_into()
                        .expect("the range holds 64 coefficients");
                    let component = &mut self.components[index];
                    if self.spectral_start == 0 {
                        let table = self.dc_tables[component.dc_table & (COMPONENTS_MAX - 1)]
                            .as_deref()
                            .ok_or(JpegError::ProgressiveDcTable)?;
                        if self.successive_high == 0 {
                            stream.decode_dc_first(
                                &mut self.reader,
                                table,
                                &mut data[0],
                                &mut component.dc_prediction,
                            )?;
                        } else {
                            stream.decode_dc_refine(&mut self.reader, &mut data[0]);
                        }
                    } else {
                        let slot = component.ac_table;
                        let table = self
                            .ac_tables
                            .get(slot)
                            .ok_or(JpegError::HuffmanSlot(slot))?
                            .as_deref()
                            .ok_or(JpegError::HuffmanTableMissing(slot))?;
                        let mask = masks[index]
                            .get_mut(start / BLOCK_COEFFICIENTS)
                            .ok_or(JpegError::LayoutOutOfBounds)?;
                        if self.successive_high == 0 {
                            stream.decode_ac_first(&mut self.reader, table, data, mask)?;
                        } else {
                            stream.decode_ac_refine(&mut self.reader, table, data, mask)?;
                        }
                    }
                }
                self.todo = self.todo.wrapping_sub(1);
                self.handle_restart_main(stream)?;
            }
        }
        Ok(())
    }

    fn decode_interleaved_dc_scan(
        &mut self,
        stream: &mut BitStream,
        buffers: &mut [Vec<i16>; COMPONENTS_MAX],
    ) -> Result<(), JpegError> {
        if self.spectral_end != 0 {
            return Err(JpegError::MergedInterleavedScan);
        }
        let count = usize::from(self.scan_components);
        for &index in &self.scan_order[..count] {
            let Some(component) = self.components.get(index) else {
                return Err(JpegError::ScanComponentMissing(index));
            };
            let slot = component.dc_table;
            self.dc_tables
                .get(slot)
                .ok_or(JpegError::HuffmanSlot(slot))?
                .as_deref()
                .ok_or(JpegError::HuffmanTableMissing(slot))?;
        }
        let order = self.scan_order;
        for row in 0..self.mcu_rows {
            for column in 0..self.mcu_columns {
                for &index in &order[..count] {
                    let component = &mut self.components[index];
                    let table = self.dc_tables[component.dc_table]
                        .as_deref()
                        .expect("the scan's tables were checked above");
                    for vertical in 0..component.vertical_sample {
                        for horizontal in 0..component.horizontal_sample {
                            let x = column * component.horizontal_sample + horizontal;
                            let y = row * component.vertical_sample + vertical;
                            let position = 64 * (x + y * component.width_stride / 8);
                            let data = buffers[index]
                                .get_mut(position)
                                .ok_or(JpegError::InvalidImage)?;
                            if self.successive_high == 0 {
                                stream.decode_dc_first(
                                    &mut self.reader,
                                    table,
                                    data,
                                    &mut component.dc_prediction,
                                )?;
                            } else {
                                stream.decode_dc_refine(&mut self.reader, data);
                            }
                        }
                    }
                }
                self.todo = self.todo.wrapping_sub(1);
                self.handle_restart_main(stream)?;
            }
        }
        Ok(())
    }

    fn finish_progressive(
        &mut self,
        buffers: &[Vec<i16>; COMPONENTS_MAX],
        pixels: &mut [u8],
    ) -> Result<(), JpegError> {
        let mcu_height = if self.interleaved {
            self.mcu_rows
        } else {
            usize::from(self.height.wrapping_add(7) / 8)
        };
        let width = usize::from(self.width);
        let padded = padded_width(width, self.sample_ratio);
        let mut scratch = zeroed(&[
            usize::from(self.interleaved),
            self.components[0].width_stride,
        ])?;
        let output_last = self.output.components() - 1;
        let four_channel = matches!(self.input, ColorSpace::Ycck | ColorSpace::Cmyk);
        for (index, component) in self.components.iter_mut().enumerate() {
            if output_last.min(index) == index || four_channel {
                component.samples =
                    zeroed(&[component.width_stride, component.vertical_sample, 8])?;
                component.needed = true;
            } else {
                component.needed = false;
            }
        }
        let mut coefficients = [0_i32; 64];
        let mut pixels_written = 0;
        for row in 0..mcu_height {
            for (component, buffer) in self.components.iter_mut().zip(buffers) {
                if !component.needed {
                    continue;
                }
                let step = buffer.len() / mcu_height;
                let source = &buffer[row * step..(row + 1) * step];
                let stride = component.width_stride;
                let blocks = stride / 8;
                for vertical in 0..component.vertical_sample {
                    for column in 0..blocks {
                        let start = column * 64 + vertical * 8 * stride;
                        let quantized = source
                            .get(start..start + 64)
                            .ok_or(JpegError::InvalidSlice)?;
                        for ((target, &value), &factor) in coefficients
                            .iter_mut()
                            .zip(quantized)
                            .zip(&component.quantization)
                        {
                            *target = i32::from(value).wrapping_mul(factor);
                        }
                        let position = component.idct_position;
                        component.idct_position += 8;
                        idct::transform(
                            &mut coefficients,
                            &mut component.samples,
                            position,
                            stride,
                        )?;
                    }
                    component.idct_position += 7 * stride;
                }
                component.idct_position = 0;
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
}
