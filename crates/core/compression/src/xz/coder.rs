use super::stream::{Action, Error, Status};

pub(crate) trait Code: Send + Sync {
    fn code(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error>;
}

pub(crate) type Coder = alloc::boxed::Box<dyn Code>;

pub(crate) fn buffer_copy(
    input: &[u8],
    input_position: &mut usize,
    output: &mut [u8],
    output_position: &mut usize,
) -> usize {
    assert!(*input_position <= input.len());
    assert!(*output_position <= output.len());
    let count = (input.len() - *input_position).min(output.len() - *output_position);
    output[*output_position..*output_position + count]
        .copy_from_slice(&input[*input_position..*input_position + count]);
    *input_position += count;
    *output_position += count;
    count
}
