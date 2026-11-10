use std::ops::Range;

use image::{DynamicImage, ImageBuffer, Pixel, imageops::FilterType};

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

fn resize<const N: usize>(
    source: &[u8],
    width: usize,
    new_width: usize,
    new_height: usize,
) -> Vec<u8> {
    let down = taps(source.len() / (width * N), new_height);
    let across = taps(width, new_width);
    let band = (BAND_BYTES / (width * N * size_of::<f32>())).clamp(1, new_height);
    let mut target = Vec::with_capacity(new_width * new_height * N);
    let mut rows = Vec::with_capacity(band * width * N);
    let mut columns = Vec::with_capacity(band * width);
    let mut narrowed = Vec::with_capacity(band * new_width * N);
    let mut pixels: Vec<[u8; N]> = Vec::with_capacity(band * new_width);
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
            nearest,
        );
        pixels.clear();
        transpose(narrowed.as_chunks::<N>().0, count, &mut pixels);
        target.extend_from_slice(pixels.as_flattened());
    }
    target
}

fn resized<P, const N: usize>(
    source: &ImageBuffer<P, Vec<u8>>,
    width: u32,
    height: u32,
) -> Option<ImageBuffer<P, Vec<u8>>>
where
    P: Pixel<Subpixel = u8>,
{
    ImageBuffer::from_raw(
        width,
        height,
        resize::<N>(
            source.as_raw(),
            source.width() as usize,
            width as usize,
            height as usize,
        ),
    )
}

pub fn lanczos3(image: &DynamicImage, width: u32, height: u32) -> DynamicImage {
    let source = (image.width(), image.height());
    let resampled = if source == (width, height) || [source.0, source.1, width, height].contains(&0)
    {
        None
    } else {
        match image {
            DynamicImage::ImageLuma8(pixels) => {
                resized::<_, 1>(pixels, width, height).map(DynamicImage::ImageLuma8)
            }
            DynamicImage::ImageLumaA8(pixels) => {
                resized::<_, 2>(pixels, width, height).map(DynamicImage::ImageLumaA8)
            }
            DynamicImage::ImageRgb8(pixels) => {
                resized::<_, 3>(pixels, width, height).map(DynamicImage::ImageRgb8)
            }
            DynamicImage::ImageRgba8(pixels) => {
                resized::<_, 4>(pixels, width, height).map(DynamicImage::ImageRgba8)
            }
            _ => None,
        }
    };
    resampled.unwrap_or_else(|| image.resize_exact(width, height, FilterType::Lanczos3))
}
