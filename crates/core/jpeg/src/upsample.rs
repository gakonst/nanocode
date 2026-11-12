use crate::error::JpegError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SampleRatio {
    HorizontalVertical,
    Vertical,
    Horizontal,
    Generic(usize, usize),
    None,
}

impl SampleRatio {
    pub(crate) const fn sample(self) -> usize {
        match self {
            Self::HorizontalVertical => 4,
            Self::Vertical | Self::Horizontal => 2,
            Self::Generic(horizontal, vertical) => horizontal * vertical,
            Self::None => 1,
        }
    }

    pub(crate) fn apply(
        self,
        input: &[i16],
        near: &[i16],
        far: &[i16],
        scratch: &mut [i16],
        output: &mut [i16],
    ) -> Result<(), JpegError> {
        match self {
            Self::None => Ok(()),
            Self::Horizontal => horizontal(input, output),
            Self::Vertical => vertical(input, near, far, output),
            Self::HorizontalVertical => horizontal_vertical(input, near, far, scratch, output),
            Self::Generic(_, _) => {
                generic(input, output);
                Ok(())
            }
        }
    }
}

fn horizontal(input: &[i16], output: &mut [i16]) -> Result<(), JpegError> {
    if input.len() * 2 != output.len() || output.len() <= 4 || input.len() <= 2 {
        return Err(JpegError::LayoutOutOfBounds);
    }
    output[0] = input[0];
    output[1] = (input[0] * 3 + input[1] + 2) >> 2;
    for (pair, window) in output[2..]
        .as_chunks_mut::<2>()
        .0
        .iter_mut()
        .zip(input.windows(3))
    {
        let sample = 3 * window[1] + 2;
        pair[0] = (sample + window[0]) >> 2;
        pair[1] = (sample + window[2]) >> 2;
    }
    let output_end = output.len() - 2;
    let input_end = input.len() - 2;
    output[output_end] = (3 * input[input_end] + input[input_end + 1] + 2) >> 2;
    output[output_end + 1] = input[input_end + 1];
    Ok(())
}

fn vertical(input: &[i16], near: &[i16], far: &[i16], output: &mut [i16]) -> Result<(), JpegError> {
    if input.len() * 2 != output.len() || near.len() != input.len() || far.len() != input.len() {
        return Err(JpegError::LayoutOutOfBounds);
    }
    let (top, bottom) = output.split_at_mut(input.len());
    for ((sample, neighbour), target) in input.iter().zip(near).zip(top) {
        *target = (3 * sample + 2 + neighbour) >> 2;
    }
    for ((sample, neighbour), target) in input.iter().zip(far).zip(bottom) {
        *target = (3 * sample + 2 + neighbour) >> 2;
    }
    Ok(())
}

fn horizontal_vertical(
    input: &[i16],
    near: &[i16],
    far: &[i16],
    scratch: &mut [i16],
    output: &mut [i16],
) -> Result<(), JpegError> {
    if input.len() * 4 != output.len() {
        return Err(JpegError::LayoutOutOfBounds);
    }
    vertical(input, near, far, scratch)?;
    let scratch_half = scratch.len() / 2;
    let output_half = output.len() / 2;
    let (output_top, output_bottom) = output.split_at_mut(output_half);
    horizontal(&scratch[..scratch_half], output_top)?;
    horizontal(&scratch[scratch_half..], output_bottom)
}

fn generic(input: &[i16], output: &mut [i16]) {
    if input.is_empty() {
        return;
    }
    let factor = output.len() / input.len();
    if factor == 0 {
        return;
    }
    for (&sample, chunk) in input.iter().zip(output.chunks_exact_mut(factor)) {
        chunk.fill(sample);
    }
}
