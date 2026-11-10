use std::ops::Range;

use image::{DynamicImage, ImageBuffer, Pixel, imageops::FilterType};

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

fn lines<S, T>(source: &[S], samples: usize, output: usize, finish: impl Fn(f32) -> T) -> Vec<T>
where
    S: Copy + Into<f32>,
{
    let taps = taps(source.len() / samples, output);
    let mut target = Vec::with_capacity(samples * output);
    let mut sums = vec![0.0; samples];
    for (start, weights) in &taps.spans {
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
    target
}

fn transpose<P: Copy>(source: &[P], width: usize) -> Vec<P> {
    let mut target = Vec::with_capacity(source.len());
    for x in 0..width {
        target.extend(source.chunks_exact(width).map(|line| line[x]));
    }
    target
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
    let rows = lines(source, width * N, new_height, |sum| sum);
    let columns = transpose(rows.as_chunks::<N>().0, width).into_flattened();
    drop(rows);
    let columns = lines(&columns, new_height * N, new_width, nearest);
    transpose(columns.as_chunks::<N>().0, new_height).into_flattened()
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
