use crate::pixels::{Image, Pixels};

trait Sample: Copy + Default {
    const LIMIT: u32;
    fn wide(self) -> u32;
    fn narrow(value: u32) -> Self;
    fn from_mix(value: f32) -> Self;
}

impl Sample for u8 {
    const LIMIT: u32 = Self::MAX as u32;

    fn wide(self) -> u32 {
        u32::from(self)
    }

    fn narrow(value: u32) -> Self {
        value.min(Self::LIMIT) as Self
    }

    fn from_mix(value: f32) -> Self {
        assert!(
            value > -1.0 && value < 256.0,
            "average sample value should fit into sample type"
        );
        value as Self
    }
}

impl Sample for u16 {
    const LIMIT: u32 = Self::MAX as u32;

    fn wide(self) -> u32 {
        u32::from(self)
    }

    fn narrow(value: u32) -> Self {
        value.min(Self::LIMIT) as Self
    }

    fn from_mix(value: f32) -> Self {
        assert!(
            value > -1.0 && value < 65536.0,
            "average sample value should fit into sample type"
        );
        value as Self
    }
}

struct Source<'a, S, const N: usize> {
    samples: &'a [S],
    width: usize,
}

impl<S: Sample, const N: usize> Source<'_, S, N> {
    fn pixel(&self, x: u32, y: u32) -> &[S] {
        let at = (y as usize * self.width + x as usize) * N;
        &self.samples[at..at + N]
    }
}

const fn clamp(value: u32, low: u32, high: u32) -> u32 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

fn sample_boxes<S: Sample, const N: usize>(
    samples: &[S],
    width: u32,
    height: u32,
    new_width: u32,
    new_height: u32,
) -> Vec<S> {
    let mut output = vec![S::default(); new_width as usize * new_height as usize * N];
    if width == 0 || height == 0 {
        return output;
    }
    let source = Source::<S, N> {
        samples,
        width: width as usize,
    };
    let x_ratio = width as f32 / new_width as f32;
    let y_ratio = height as f32 / new_height as f32;
    for out_y in 0..new_height {
        let bottom_f = out_y as f32 * y_ratio;
        let top_f = bottom_f + y_ratio;
        let bottom = clamp(bottom_f.ceil() as u32, 0, height - 1);
        let top = clamp(top_f.ceil() as u32, bottom, height);
        for out_x in 0..new_width {
            let left_f = out_x as f32 * x_ratio;
            let right_f = left_f + x_ratio;
            let left = clamp(left_f.ceil() as u32, 0, width - 1);
            let right = clamp(right_f.ceil() as u32, left, width);
            let at = (out_y as usize * new_width as usize + out_x as usize) * N;
            let target = &mut output[at..at + N];
            if bottom != top && left != right {
                block(&source, left, right, bottom, top, target);
            } else if bottom != top {
                let fraction = (left_f.fract() + right_f.fract()) / 2.0;
                horizontal(&source, right - 1, fraction, bottom, top, target);
            } else if left != right {
                let fraction = (top_f.fract() + bottom_f.fract()) / 2.0;
                vertical(&source, left, right, top - 1, fraction, target);
            } else {
                let fraction_y = (top_f.fract() + bottom_f.fract()) / 2.0;
                let fraction_x = (left_f.fract() + right_f.fract()) / 2.0;
                both(&source, right - 1, fraction_y, top - 1, fraction_x, target);
            }
        }
    }
    output
}

fn block<S: Sample, const N: usize>(
    source: &Source<'_, S, N>,
    left: u32,
    right: u32,
    bottom: u32,
    top: u32,
    target: &mut [S],
) {
    let mut sums = [0u32; N];
    for y in bottom..top {
        for x in left..right {
            for (sum, sample) in sums.iter_mut().zip(source.pixel(x, y)) {
                *sum = sum.wrapping_add(sample.wide());
            }
        }
    }
    let count = (right - left).wrapping_mul(top - bottom);
    let round = count / 2;
    for (value, sum) in target.iter_mut().zip(sums) {
        *value = S::narrow(sum.wrapping_add(round) / count);
    }
}

fn horizontal<S: Sample, const N: usize>(
    source: &Source<'_, S, N>,
    left: u32,
    fraction: f32,
    bottom: u32,
    top: u32,
    target: &mut [S],
) {
    let mut sums_left = [0u32; N];
    let mut sums_right = [0u32; N];
    for y in bottom..top {
        for (sum, sample) in sums_left.iter_mut().zip(source.pixel(left, y)) {
            *sum = sum.wrapping_add(sample.wide());
        }
        for (sum, sample) in sums_right.iter_mut().zip(source.pixel(left + 1, y)) {
            *sum = sum.wrapping_add(sample.wide());
        }
    }
    let factor_right = fraction / ((top - bottom) as f32);
    let factor_left = (1.0 - fraction) / ((top - bottom) as f32);
    for ((value, left), right) in target.iter_mut().zip(sums_left).zip(sums_right) {
        *value = S::from_mix(factor_left * left as f32 + factor_right * right as f32);
    }
}

fn vertical<S: Sample, const N: usize>(
    source: &Source<'_, S, N>,
    left: u32,
    right: u32,
    bottom: u32,
    fraction: f32,
    target: &mut [S],
) {
    let mut sums_bottom = [0u32; N];
    let mut sums_top = [0u32; N];
    for x in left..right {
        for (sum, sample) in sums_bottom.iter_mut().zip(source.pixel(x, bottom)) {
            *sum = sum.wrapping_add(sample.wide());
        }
        for (sum, sample) in sums_top.iter_mut().zip(source.pixel(x, bottom + 1)) {
            *sum = sum.wrapping_add(sample.wide());
        }
    }
    let factor_top = fraction / ((right - left) as f32);
    let factor_bottom = (1.0 - fraction) / ((right - left) as f32);
    for ((value, bottom), top) in target.iter_mut().zip(sums_bottom).zip(sums_top) {
        *value = S::from_mix(factor_bottom * bottom as f32 + factor_top * top as f32);
    }
}

fn both<S: Sample, const N: usize>(
    source: &Source<'_, S, N>,
    left: u32,
    fraction_vertical: f32,
    bottom: u32,
    fraction_horizontal: f32,
    target: &mut [S],
) {
    let bottom_left = source.pixel(left, bottom);
    let top_left = source.pixel(left, bottom + 1);
    let bottom_right = source.pixel(left + 1, bottom);
    let top_right = source.pixel(left + 1, bottom + 1);
    let factor_top_right = fraction_vertical * fraction_horizontal;
    let factor_top_left = fraction_vertical * (1.0 - fraction_horizontal);
    let factor_bottom_right = (1.0 - fraction_vertical) * fraction_horizontal;
    let factor_bottom_left = (1.0 - fraction_vertical) * (1.0 - fraction_horizontal);
    for channel in 0..N {
        let mix = factor_bottom_right * bottom_right[channel].wide() as f32
            + factor_top_right * top_right[channel].wide() as f32
            + factor_bottom_left * bottom_left[channel].wide() as f32
            + factor_top_left * top_left[channel].wide() as f32;
        target[channel] = S::from_mix(mix);
    }
}

pub fn thumbnail(image: &Image, width: u32, height: u32) -> Image {
    let (from_width, from_height) = (image.width(), image.height());
    let pixels = match image.pixels() {
        Pixels::Luma8(samples) => Pixels::Luma8(sample_boxes::<_, 1>(
            samples,
            from_width,
            from_height,
            width,
            height,
        )),
        Pixels::LumaAlpha8(samples) => Pixels::LumaAlpha8(sample_boxes::<_, 2>(
            samples,
            from_width,
            from_height,
            width,
            height,
        )),
        Pixels::Rgb8(samples) => Pixels::Rgb8(sample_boxes::<_, 3>(
            samples,
            from_width,
            from_height,
            width,
            height,
        )),
        Pixels::Rgba8(samples) => Pixels::Rgba8(sample_boxes::<_, 4>(
            samples,
            from_width,
            from_height,
            width,
            height,
        )),
        Pixels::Luma16(samples) => Pixels::Luma16(sample_boxes::<_, 1>(
            samples,
            from_width,
            from_height,
            width,
            height,
        )),
        Pixels::LumaAlpha16(samples) => Pixels::LumaAlpha16(sample_boxes::<_, 2>(
            samples,
            from_width,
            from_height,
            width,
            height,
        )),
        Pixels::Rgb16(samples) => Pixels::Rgb16(sample_boxes::<_, 3>(
            samples,
            from_width,
            from_height,
            width,
            height,
        )),
        Pixels::Rgba16(samples) => Pixels::Rgba16(sample_boxes::<_, 4>(
            samples,
            from_width,
            from_height,
            width,
            height,
        )),
    };
    Image::new(width, height, pixels).expect("thumbnails hold width * height pixels")
}
