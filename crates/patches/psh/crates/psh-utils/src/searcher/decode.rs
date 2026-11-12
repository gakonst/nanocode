use std::io::{self, Read};

use encoding_rs::{CoderResult, Decoder, Encoding, UTF_8};

const BOM_BYTES_MAX: usize = 3;
const STEP_OUTPUT_BYTES: usize = 7;
const DIRECT_OUTPUT_BYTES_MIN: usize = 4;

#[derive(Clone, Copy, Debug)]
struct Bom {
    bytes: [u8; BOM_BYTES_MAX],
    length: usize,
}

impl Bom {
    fn kept(&self, strip: bool) -> &[u8] {
        let bytes = &self.bytes[..self.length];
        if !strip || bytes.len() <= 1 {
            bytes
        } else if bytes.starts_with(b"\xFF\xFE") || bytes.starts_with(b"\xFE\xFF") {
            &bytes[2..]
        } else if bytes == b"\xEF\xBB\xBF" {
            &[]
        } else {
            bytes
        }
    }

    fn encoding(&self) -> Option<&'static Encoding> {
        if self.length < BOM_BYTES_MAX {
            return None;
        }
        Encoding::for_bom(&self.bytes).map(|(encoding, _)| encoding)
    }
}

#[derive(Debug)]
struct BomReader<R> {
    reader: R,
    strip: bool,
    bom: Option<Bom>,
    bom_bytes_served: usize,
}

impl<R: Read> BomReader<R> {
    fn peek(&mut self) -> io::Result<Bom> {
        if let Some(bom) = self.bom {
            return Ok(bom);
        }
        self.bom = Some(Bom {
            bytes: [0; BOM_BYTES_MAX],
            length: 0,
        });
        let mut bytes = [0; BOM_BYTES_MAX];
        let length = read_full(&mut self.reader, &mut bytes)?;
        let bom = Bom { bytes, length };
        self.bom = Some(bom);
        Ok(bom)
    }
}

impl<R: Read> Read for BomReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.bom_bytes_served < BOM_BYTES_MAX {
            let bom = self.peek()?;
            let kept = bom.kept(self.strip);
            if self.bom_bytes_served < kept.len() {
                let rest = &kept[self.bom_bytes_served..];
                let length = output.len().min(rest.len());
                output[..length].copy_from_slice(&rest[..length]);
                self.bom_bytes_served += length;
                return Ok(length);
            }
            self.bom_bytes_served = BOM_BYTES_MAX;
        }
        self.reader.read(output)
    }
}

fn read_full<R: Read>(reader: &mut R, mut output: &mut [u8]) -> io::Result<usize> {
    let mut read = 0;
    while !output.is_empty() {
        match reader.read(output) {
            Ok(0) => break,
            Ok(count) => {
                read += count;
                output = &mut output[count..];
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(read)
}

#[derive(Debug)]
struct Input<'b, R> {
    source: BomReader<R>,
    buffer: &'b mut [u8],
    position: usize,
    length: usize,
    exhausted: bool,
}

impl<R: Read> Input<'_, R> {
    fn pending(&self) -> &[u8] {
        &self.buffer[self.position..self.length]
    }

    fn fill(&mut self) -> io::Result<()> {
        if self.position < self.length {
            assert!(
                self.length < self.buffer.len(),
                "the decode buffer is full of undecoded bytes"
            );
            self.buffer.copy_within(self.position..self.length, 0);
            self.length -= self.position;
        } else {
            self.length = 0;
        }
        self.position = 0;
        self.length += self.source.read(&mut self.buffer[self.length..])?;
        if self.length == 0 {
            self.exhausted = true;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct StepOutput {
    bytes: [u8; STEP_OUTPUT_BYTES],
    position: usize,
    length: usize,
}

impl StepOutput {
    fn decode(&mut self, decoder: &mut Decoder, input: &[u8], last: bool) -> (usize, usize) {
        assert!(
            self.position >= self.length,
            "the step output has unread bytes"
        );
        assert!(
            !last || input.is_empty(),
            "the last decoding step has input"
        );
        let (result, read, written, _) = decoder.decode_to_utf8(input, &mut self.bytes, last);
        assert!(
            !last || result == CoderResult::InputEmpty,
            "the last decoding step left input"
        );
        self.position = 0;
        self.length = written;
        (read, written)
    }

    fn take(&mut self, output: &mut [u8]) -> usize {
        let unread = &self.bytes[self.position..self.length];
        let count = unread.len().min(output.len());
        output[..count].copy_from_slice(&unread[..count]);
        self.position += count;
        count
    }
}

#[derive(Debug)]
pub(crate) struct DecodeReader<'b, R> {
    input: Input<'b, R>,
    decoder: Option<Decoder>,
    step_output: StepOutput,
    detected: bool,
}

impl<'b, R: Read> DecodeReader<'b, R> {
    pub(crate) fn new(
        reader: R,
        buffer: &'b mut [u8],
        encoding: Option<&'static Encoding>,
        bom_sniffing: bool,
    ) -> DecodeReader<'b, R> {
        assert!(
            buffer.len() >= DIRECT_OUTPUT_BYTES_MIN,
            "the decode buffer holds {} bytes",
            buffer.len()
        );
        DecodeReader {
            input: Input {
                source: BomReader {
                    reader,
                    strip: bom_sniffing,
                    bom: None,
                    bom_bytes_served: 0,
                },
                buffer,
                position: 0,
                length: 0,
                exhausted: false,
            },
            decoder: encoding.map(Encoding::new_decoder_with_bom_removal),
            step_output: StepOutput {
                bytes: [0; STEP_OUTPUT_BYTES],
                position: 0,
                length: 0,
            },
            detected: !bom_sniffing,
        }
    }

    fn detect(&mut self) -> io::Result<()> {
        if self.detected {
            return Ok(());
        }
        self.detected = true;
        let bom = self.input.source.peek()?;
        if let Some(encoding) = bom.encoding()
            && encoding != UTF_8
        {
            self.decoder = Some(encoding.new_decoder_with_bom_removal());
        }
        Ok(())
    }
}

impl<R: Read> Read for DecodeReader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.detect()?;
        match &mut self.decoder {
            None => self.input.source.read(output),
            Some(decoder) => transcode(decoder, &mut self.input, &mut self.step_output, output),
        }
    }
}

fn transcode<R: Read>(
    decoder: &mut Decoder,
    input: &mut Input<'_, R>,
    step_output: &mut StepOutput,
    output: &mut [u8],
) -> io::Result<usize> {
    if input.exhausted || output.is_empty() {
        return Ok(0);
    }
    let taken = step_output.take(output);
    if taken > 0 {
        return Ok(taken);
    }
    if input.position >= input.length {
        input.fill()?;
    }
    if output.len() < DIRECT_OUTPUT_BYTES_MIN {
        return transcode_by_step(decoder, input, step_output, output);
    }
    loop {
        let (_, read, written, _) = decoder.decode_to_utf8(input.pending(), output, false);
        input.position += read;
        if written > 0 {
            return Ok(written);
        }
        input.fill()?;
        if input.length == 0 {
            let (_, _, written, _) = decoder.decode_to_utf8(&[], output, true);
            return Ok(written);
        }
    }
}

fn transcode_by_step<R: Read>(
    decoder: &mut Decoder,
    input: &mut Input<'_, R>,
    step_output: &mut StepOutput,
    output: &mut [u8],
) -> io::Result<usize> {
    assert!(
        output.len() < DIRECT_OUTPUT_BYTES_MIN,
        "a step for {} bytes of room",
        output.len()
    );
    loop {
        let (read, written) = step_output.decode(decoder, input.pending(), false);
        input.position += read;
        if written > 0 {
            return Ok(step_output.take(output));
        }
        input.fill()?;
        if input.length == 0 {
            step_output.decode(decoder, &[], true);
            return Ok(step_output.take(output));
        }
    }
}
