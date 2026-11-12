use alloc::boxed::Box;

use super::coder::{Code, Coder};
use super::stream::{Action, Error, Status};

pub(crate) const MEMUSAGE_BYTES: u64 = 352;
const DISTANCE_MIN: u32 = 1;
const DISTANCE_MAX: u32 = 256;
const HISTORY_BYTES: usize = DISTANCE_MAX as usize;
const HISTORY_MASK: usize = HISTORY_BYTES - 1;

struct History {
    distance: usize,
    position: u8,
    bytes: [u8; HISTORY_BYTES],
}

impl History {
    fn new(distance: u32) -> Result<Self, Error> {
        if !(DISTANCE_MIN..=DISTANCE_MAX).contains(&distance) {
            return Err(Error::Options);
        }
        Ok(Self {
            distance: usize::try_from(distance).expect("a delta distance fits in usize"),
            position: 0,
            bytes: [0; HISTORY_BYTES],
        })
    }

    fn recent(&self, back: usize) -> u8 {
        assert!(back >= 1);
        assert!(back <= HISTORY_BYTES);
        self.bytes[(usize::from(self.position) + back) & HISTORY_MASK]
    }

    fn remember(&mut self, tail: &[u8], length: usize) {
        assert!(tail.len() <= HISTORY_BYTES);
        assert!(tail.len() <= length);
        let position = usize::from(self.position);
        let tail_start = length - tail.len();
        for (offset, &byte) in tail.iter().enumerate() {
            let back = (tail_start + offset) & HISTORY_MASK;
            self.bytes[(position + HISTORY_BYTES - back) & HISTORY_MASK] = byte;
        }
        let advanced = (position + HISTORY_BYTES - (length & HISTORY_MASK)) & HISTORY_MASK;
        self.position = u8::try_from(advanced).expect("a history position is below 256");
    }

    fn head_length(&self, length: usize) -> usize {
        length.min(self.distance)
    }

    fn decode(&mut self, buffer: &mut [u8]) {
        let distance = self.distance;
        for index in 0..self.head_length(buffer.len()) {
            buffer[index] = buffer[index].wrapping_add(self.recent(distance - index));
        }
        for index in distance..buffer.len() {
            buffer[index] = buffer[index].wrapping_add(buffer[index - distance]);
        }
        let tail_start = buffer.len() - buffer.len().min(HISTORY_BYTES);
        self.remember(&buffer[tail_start..], buffer.len());
    }

    fn encode(&mut self, input: &[u8], output: &mut [u8]) {
        assert_eq!(input.len(), output.len());
        let distance = self.distance;
        let head = self.head_length(input.len());
        for index in 0..head {
            output[index] = input[index].wrapping_sub(self.recent(distance - index));
        }
        for ((target, &byte), &previous) in output[head..].iter_mut().zip(&input[head..]).zip(input)
        {
            *target = byte.wrapping_sub(previous);
        }
        let tail_start = input.len() - input.len().min(HISTORY_BYTES);
        self.remember(&input[tail_start..], input.len());
    }

    fn encode_in_place(&mut self, buffer: &mut [u8]) {
        let tail_start = buffer.len() - buffer.len().min(HISTORY_BYTES);
        let mut tail = [0; HISTORY_BYTES];
        let tail = &mut tail[..buffer.len() - tail_start];
        tail.copy_from_slice(&buffer[tail_start..]);
        let distance = self.distance;
        for index in (distance..buffer.len()).rev() {
            buffer[index] = buffer[index].wrapping_sub(buffer[index - distance]);
        }
        for index in 0..self.head_length(buffer.len()) {
            buffer[index] = buffer[index].wrapping_sub(self.recent(distance - index));
        }
        self.remember(tail, buffer.len());
    }
}

struct DeltaDecoder {
    next: Coder,
    history: History,
}

impl Code for DeltaDecoder {
    fn code(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        let output_start = *output_position;
        let result = self
            .next
            .code(input, input_position, output, output_position, action);
        assert!(*output_position >= output_start);
        self.history
            .decode(&mut output[output_start..*output_position]);
        result
    }
}

struct DeltaEncoder {
    next: Option<Coder>,
    history: History,
}

impl DeltaEncoder {
    fn copy_and_encode(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Status {
        assert!(*input_position <= input.len());
        assert!(*output_position <= output.len());
        let size = (input.len() - *input_position).min(output.len() - *output_position);
        self.history.encode(
            &input[*input_position..*input_position + size],
            &mut output[*output_position..*output_position + size],
        );
        *input_position += size;
        *output_position += size;
        if action != Action::Run && *input_position == input.len() {
            Status::StreamEnd
        } else {
            Status::Ok
        }
    }
}

impl Code for DeltaEncoder {
    fn code(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        let Some(next) = self.next.as_mut() else {
            return Ok(self.copy_and_encode(
                input,
                input_position,
                output,
                output_position,
                action,
            ));
        };
        let output_start = *output_position;
        let result = next.code(input, input_position, output, output_position, action);
        assert!(*output_position >= output_start);
        self.history
            .encode_in_place(&mut output[output_start..*output_position]);
        result
    }
}

pub(crate) fn validate(distance: u32) -> Result<(), Error> {
    History::new(distance).map(|_| ())
}

pub(crate) fn decoder(distance: u32, next: Coder) -> Result<Coder, Error> {
    let history = History::new(distance)?;
    Ok(Box::new(DeltaDecoder { next, history }))
}

pub(crate) fn encoder(distance: u32, next: Option<Coder>) -> Result<Coder, Error> {
    let history = History::new(distance)?;
    Ok(Box::new(DeltaEncoder { next, history }))
}
