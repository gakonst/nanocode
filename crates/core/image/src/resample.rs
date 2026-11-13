use std::ops::Range;

use crate::pixels::{Image, Pixels};

const BAND_BYTES: usize = 2 << 20;

struct Taps {
    spans: Vec<(usize, Range<usize>)>,
    weights: Vec<f32>,
}

fn kernel(x: f32) -> f32 {
    let sinc = |t: f32| {
        let a = t * std::f32::consts::PI;
        if t == 0.0 { 1.0 } else { a.sin() / a }
    };
    if x.abs() < 3.0 {
        sinc(x) * sinc(x / 3.0)
    } else {
        0.0
    }
}

fn taps(input: usize, output: usize) -> Taps {
    let ratio = input as f32 / output as f32;
    let scale = if ratio < 1.0 { 1.0 } else { ratio };
    let support = 3.0 * scale;
    let mut taps = Taps {
        spans: Vec::with_capacity(output),
        weights: Vec::new(),
    };
    for index in 0..output {
        let center = (index as f32 + 0.5) * ratio;
        let left = ((center - support).floor() as i64).clamp(0, input as i64 - 1);
        let right = ((center + support).ceil() as i64).clamp(left + 1, input as i64);
        let center = center - 0.5;
        let first = taps.weights.len();
        let mut sum = 0.0;
        for source in left..right {
            let weight = kernel((source as f32 - center) / scale);
            taps.weights.push(weight);
            sum += weight;
        }
        for weight in &mut taps.weights[first..] {
            *weight /= sum;
        }
        taps.spans.push((left as usize, first..taps.weights.len()));
    }
    taps
}

fn lines<S, T>(
    source: &[S],
    samples: usize,
    taps: &Taps,
    range: Range<usize>,
    target: &mut Vec<T>,
    finish: impl Fn(f32) -> T,
) where
    S: Copy + Into<f32>,
{
    let mut sums = vec![0.0; samples];
    for (start, weights) in &taps.spans[range] {
        sums.fill(0.0);
        for (line, &weight) in source[start * samples..]
            .chunks_exact(samples)
            .zip(&taps.weights[weights.clone()])
        {
            for (sum, &value) in sums.iter_mut().zip(line) {
                *sum += value.into() * weight;
            }
        }
        target.extend(sums.iter().map(|&sum| finish(sum)));
    }
}

fn transpose<P: Copy>(source: &[P], width: usize, target: &mut Vec<P>) {
    for x in 0..width {
        target.extend(source.chunks_exact(width).map(|line| line[x]));
    }
}

fn nearest(value: f32) -> u8 {
    let floor = value as u8;
    floor.saturating_add(u8::from(value - f32::from(floor) >= 0.5))
}

fn nearest_wide(value: f32) -> u16 {
    let floor = value as u16;
    floor.saturating_add(u16::from(value - f32::from(floor) >= 0.5))
}

fn resize<S, const N: usize>(
    source: &[S],
    width: usize,
    new_width: usize,
    new_height: usize,
    finish: impl Fn(f32) -> S + Copy,
) -> Vec<S>
where
    S: Copy + Into<f32>,
{
    let down = taps(source.len() / (width * N), new_height);
    let across = taps(width, new_width);
    let band = (BAND_BYTES / (width * N * size_of::<f32>())).clamp(1, new_height);
    let mut target = Vec::with_capacity(new_width * new_height * N);
    let mut rows = Vec::with_capacity(band * width * N);
    let mut columns = Vec::with_capacity(band * width);
    let mut narrowed = Vec::with_capacity(band * new_width * N);
    let mut pixels: Vec<[S; N]> = Vec::with_capacity(band * new_width);
    for top in (0..new_height).step_by(band) {
        let count = band.min(new_height - top);
        rows.clear();
        lines(
            source,
            width * N,
            &down,
            top..top + count,
            &mut rows,
            |sum| sum,
        );
        columns.clear();
        transpose(rows.as_chunks::<N>().0, width, &mut columns);
        narrowed.clear();
        lines(
            columns.as_flattened(),
            count * N,
            &across,
            0..new_width,
            &mut narrowed,
            finish,
        );
        pixels.clear();
        transpose(narrowed.as_chunks::<N>().0, count, &mut pixels);
        target.extend_from_slice(pixels.as_flattened());
    }
    target
}

pub fn lanczos3(image: &Image, width: u32, height: u32) -> Image {
    let source = (image.width(), image.height());
    if source == (width, height) {
        return image.clone();
    }
    let pixels = if [source.0, source.1, width, height].contains(&0) {
        empty(image.pixels(), width, height)
    } else {
        let (from, to, new_height) = (source.0 as usize, width as usize, height as usize);
        match image.pixels() {
            Pixels::Luma8(samples) => {
                Pixels::Luma8(resize::<_, 1>(samples, from, to, new_height, nearest))
            }
            Pixels::LumaAlpha8(samples) => {
                Pixels::LumaAlpha8(resize::<_, 2>(samples, from, to, new_height, nearest))
            }
            Pixels::Rgb8(samples) => {
                Pixels::Rgb8(resize::<_, 3>(samples, from, to, new_height, nearest))
            }
            Pixels::Rgba8(samples) => {
                Pixels::Rgba8(resize::<_, 4>(samples, from, to, new_height, nearest))
            }
            Pixels::Luma16(samples) => {
                Pixels::Luma16(resize::<_, 1>(samples, from, to, new_height, nearest_wide))
            }
            Pixels::LumaAlpha16(samples) => {
                Pixels::LumaAlpha16(resize::<_, 2>(samples, from, to, new_height, nearest_wide))
            }
            Pixels::Rgb16(samples) => {
                Pixels::Rgb16(resize::<_, 3>(samples, from, to, new_height, nearest_wide))
            }
            Pixels::Rgba16(samples) => {
                Pixels::Rgba16(resize::<_, 4>(samples, from, to, new_height, nearest_wide))
            }
        }
    };
    Image::new(width, height, pixels).expect("resampling yields width * height pixels")
}

fn empty(pixels: &Pixels, width: u32, height: u32) -> Pixels {
    let samples = width as usize * height as usize * pixels.channels();
    match pixels {
        Pixels::Luma8(_) => Pixels::Luma8(vec![0; samples]),
        Pixels::LumaAlpha8(_) => Pixels::LumaAlpha8(vec![0; samples]),
        Pixels::Rgb8(_) => Pixels::Rgb8(vec![0; samples]),
        Pixels::Rgba8(_) => Pixels::Rgba8(vec![0; samples]),
        Pixels::Luma16(_) => Pixels::Luma16(vec![0; samples]),
        Pixels::LumaAlpha16(_) => Pixels::LumaAlpha16(vec![0; samples]),
        Pixels::Rgb16(_) => Pixels::Rgb16(vec![0; samples]),
        Pixels::Rgba16(_) => Pixels::Rgba16(vec![0; samples]),
    }
}
