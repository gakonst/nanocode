use std::fmt;

const WHITESPACE: [u8; 6] = [b'\t', b'\n', 0x0b, 0x0c, b'\r', b' '];
const SAMPLE_MAX_8: u32 = 0xFF;
const SAMPLE_MAX_16: u32 = 0xFFFF;
const BIT_WHITE: u8 = 255;
const BIT_BLACK: u8 = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PnmError {
    UnexpectedEnd,
    MagicInvalid([u8; 2]),
    Unparsable(String),
    NonAsciiByte(u8),
    NonAsciiLine,
    NotUtf8Line,
    InvalidDigit,
    NotNewlineAfterMagic(u8),
    HeaderEnd,
    DuplicateLine(&'static str),
    UnknownLine(String),
    MissingLine,
    InputTooShort,
    UnexpectedRasterByte(u8),
    SampleOutOfBounds(u8),
    MaxvalZero,
    MaxvalTooBig(u32),
    InvalidDepthOrMaxval,
    Unsupported(String),
    Overflow,
}

impl fmt::Display for PnmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEnd => formatter.write_str("unexpected end of file"),
            Self::MagicInvalid(magic) => write!(
                formatter,
                "Expected magic constant for PNM: P1..P7, got [{:#04X?}, {:#04X?}]",
                magic[0], magic[1]
            ),
            Self::Unparsable(value) => write!(formatter, "Error parsing {value:?} as a number"),
            Self::NonAsciiByte(byte) => {
                write!(formatter, "Non-ASCII character {byte:#04X?} in header")
            }
            Self::NonAsciiLine => formatter.write_str("Non-ASCII line in PAM header"),
            Self::NotUtf8Line => formatter.write_str("PAM header line is not UTF-8"),
            Self::InvalidDigit => {
                formatter.write_str("Non-ASCII-digit character when parsing number in sample")
            }
            Self::NotNewlineAfterMagic(byte) => write!(
                formatter,
                "Expected newline after P7 magic, got {byte:#04X?}"
            ),
            Self::HeaderEnd => formatter.write_str("Unexpected end of PNM header"),
            Self::DuplicateLine(line) => write!(formatter, "Duplicate {line} line"),
            Self::UnknownLine(identifier) => write!(
                formatter,
                "Unknown header line with identifier {identifier:?}"
            ),
            Self::MissingLine => formatter.write_str("Missing header line"),
            Self::InputTooShort => formatter
                .write_str("Not enough data was provided to the Decoder to decode the image"),
            Self::UnexpectedRasterByte(byte) => write!(
                formatter,
                "Unexpected character {byte:#04X?} within sample raster"
            ),
            Self::SampleOutOfBounds(value) => {
                write!(formatter, "Sample value {value} outside of bounds")
            }
            Self::MaxvalZero => formatter.write_str("Image MAXVAL is zero"),
            Self::MaxvalTooBig(maxval) => {
                write!(formatter, "Image MAXVAL exceeds {SAMPLE_MAX_16}: {maxval}")
            }
            Self::InvalidDepthOrMaxval => {
                formatter.write_str("Invalid depth or maxval for the tuple type")
            }
            Self::Unsupported(what) => {
                write!(formatter, "The decoder for Pnm does not support {what}")
            }
            Self::Overflow => formatter.write_str("Overflow when parsing value"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Ascii,
    Binary,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tuple {
    Bit,
    BlackWhite,
    Gray8,
    Gray16,
    Rgb8,
    Rgb16,
}

impl Tuple {
    pub(crate) const fn channels(self) -> usize {
        match self {
            Self::Bit | Self::BlackWhite | Self::Gray8 | Self::Gray16 => 1,
            Self::Rgb8 | Self::Rgb16 => 3,
        }
    }

    pub(crate) const fn sample_bytes(self) -> usize {
        match self {
            Self::Gray16 | Self::Rgb16 => 2,
            Self::Bit | Self::BlackWhite | Self::Gray8 | Self::Rgb8 => 1,
        }
    }
}

pub(crate) enum Samples {
    Eight(Vec<u8>),
    Sixteen(Vec<u16>),
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.position)?;
        self.position += 1;
        Some(byte)
    }

    fn token(&mut self) -> Result<String, PnmError> {
        let mut token = Vec::new();
        let mut enabled = true;
        while let Some(byte) = self.byte() {
            let current = enabled && byte != b'#';
            enabled = current || byte == b'\r' || byte == b'\n';
            if !current {
                continue;
            }
            if WHITESPACE.contains(&byte) {
                if !token.is_empty() {
                    break;
                }
            } else if !byte.is_ascii() {
                return Err(PnmError::NonAsciiByte(byte));
            } else {
                token.push(byte);
            }
        }
        if token.is_empty() {
            return Err(PnmError::UnexpectedEnd);
        }
        Ok(String::from_utf8(token).expect("tokens hold ASCII bytes only"))
    }

    fn number(&mut self) -> Result<u32, PnmError> {
        let token = self.token()?;
        token
            .parse::<u32>()
            .map_err(|_| PnmError::Unparsable(token))
    }

    fn line(&mut self) -> Result<String, PnmError> {
        let mut line = Vec::new();
        while let Some(byte) = self.byte() {
            if byte == b'\n' {
                break;
            }
            line.push(byte);
        }
        String::from_utf8(line).map_err(|_| PnmError::NotUtf8Line)
    }

    fn ascii_sample(&mut self) -> Result<u16, PnmError> {
        while self
            .bytes
            .get(self.position)
            .is_some_and(|byte| WHITESPACE.contains(byte))
        {
            self.position += 1;
        }
        let mut value: u16 = 0;
        while let Some(byte) = self.byte() {
            if WHITESPACE.contains(&byte) {
                break;
            }
            let digit = match byte {
                b'0'..=b'9' => u16::from(byte - b'0'),
                _ => return Err(PnmError::InvalidDigit),
            };
            value = value.checked_mul(10).ok_or(PnmError::Overflow)?;
            value = value.checked_add(digit).ok_or(PnmError::Overflow)?;
        }
        Ok(value)
    }
}

pub(crate) struct Pnm<'a> {
    reader: Reader<'a>,
    width: u32,
    height: u32,
    maxval: u32,
    encoding: Encoding,
    tuple: Tuple,
}

impl<'a> Pnm<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Result<Self, PnmError> {
        let mut reader = Reader { bytes, position: 0 };
        let magic = [
            reader.byte().ok_or(PnmError::UnexpectedEnd)?,
            reader.byte().ok_or(PnmError::UnexpectedEnd)?,
        ];
        let (encoding, kind) = match magic {
            [b'P', kind @ b'1'..=b'3'] => (Encoding::Ascii, kind - b'0'),
            [b'P', kind @ b'4'..=b'6'] => (Encoding::Binary, kind - b'3'),
            [b'P', b'7'] => return Self::arbitrary(reader),
            _ => return Err(PnmError::MagicInvalid(magic)),
        };
        let width = reader.number()?;
        let height = reader.number()?;
        if kind == 1 {
            return Ok(Self {
                reader,
                width,
                height,
                maxval: 1,
                encoding,
                tuple: Tuple::Bit,
            });
        }
        let maxval = reader.number()?;
        let tuple = match (maxval, kind) {
            (0, _) => return Err(PnmError::MaxvalZero),
            (..=SAMPLE_MAX_8, 2) => Tuple::Gray8,
            (..=SAMPLE_MAX_16, 2) => Tuple::Gray16,
            (..=SAMPLE_MAX_8, _) => Tuple::Rgb8,
            (..=SAMPLE_MAX_16, _) => Tuple::Rgb16,
            (value, _) => return Err(PnmError::MaxvalTooBig(value)),
        };
        Ok(Self {
            reader,
            width,
            height,
            maxval,
            encoding,
            tuple,
        })
    }

    fn arbitrary(mut reader: Reader<'a>) -> Result<Self, PnmError> {
        match reader.byte() {
            None => return Err(PnmError::UnexpectedEnd),
            Some(b'\n') => {}
            Some(other) => return Err(PnmError::NotNewlineAfterMagic(other)),
        }
        let mut fields: [Option<u32>; 4] = [None; 4];
        let mut tuple_type: Option<String> = None;
        let lines_max = reader.bytes.len() + 1;
        for _ in 0..lines_max {
            let line = reader.line()?;
            if line.is_empty() {
                return Err(PnmError::HeaderEnd);
            }
            if line.as_bytes()[0] == b'#' {
                continue;
            }
            if !line.is_ascii() {
                return Err(PnmError::NonAsciiLine);
            }
            let split = line.find(char::is_whitespace).unwrap_or(line.len());
            let (identifier, rest) = line.trim_start().split_at(split);
            let slot = match identifier {
                "ENDHDR" => return Self::from_fields(reader, fields, tuple_type),
                "HEIGHT" => 0,
                "WIDTH" => 1,
                "DEPTH" => 2,
                "MAXVAL" => 3,
                "TUPLTYPE" => {
                    let value = rest.trim();
                    match &mut tuple_type {
                        Some(existing) => {
                            existing.push(' ');
                            existing.push_str(value);
                        }
                        None => tuple_type = Some(value.to_owned()),
                    }
                    continue;
                }
                other => return Err(PnmError::UnknownLine(other.to_owned())),
            };
            if fields[slot].is_some() {
                return Err(PnmError::DuplicateLine(
                    ["HEIGHT", "WIDTH", "DEPTH", "MAXVAL"][slot],
                ));
            }
            fields[slot] = Some(
                rest.trim()
                    .parse()
                    .map_err(|_| PnmError::Unparsable(rest.to_owned()))?,
            );
        }
        unreachable!("every header line consumes at least its newline")
    }

    fn from_fields(
        reader: Reader<'a>,
        fields: [Option<u32>; 4],
        tuple_type: Option<String>,
    ) -> Result<Self, PnmError> {
        let [Some(height), Some(width), Some(depth), Some(maxval)] = fields else {
            return Err(PnmError::MissingLine);
        };
        let unsupported = |what: &str| PnmError::Unsupported(what.to_owned());
        let tuple = match tuple_type.as_deref() {
            _ if maxval == 0 => return Err(PnmError::MaxvalZero),
            None if depth == 1 => Tuple::Gray8,
            None if depth == 2 => return Err(unsupported("the color type La8")),
            None if depth == 3 => Tuple::Rgb8,
            None if depth == 4 => return Err(unsupported("the color type Rgba8")),
            Some("BLACKANDWHITE") if maxval == 1 && depth == 1 => Tuple::BlackWhite,
            Some("BLACKANDWHITE") => return Err(PnmError::InvalidDepthOrMaxval),
            Some("GRAYSCALE") if depth == 1 && maxval <= SAMPLE_MAX_8 => Tuple::Gray8,
            Some("GRAYSCALE") if depth <= 1 && maxval <= SAMPLE_MAX_16 => Tuple::Gray16,
            Some("GRAYSCALE") => return Err(PnmError::InvalidDepthOrMaxval),
            Some("RGB") if depth == 3 && maxval <= SAMPLE_MAX_8 => Tuple::Rgb8,
            Some("RGB") if depth == 3 && maxval <= SAMPLE_MAX_16 => Tuple::Rgb16,
            Some("RGB") => return Err(PnmError::InvalidDepthOrMaxval),
            Some("BLACKANDWHITE_ALPHA") => {
                return Err(unsupported("the color type BLACKANDWHITE_ALPHA"));
            }
            Some("GRAYSCALE_ALPHA") => return Err(unsupported("the color type La8")),
            Some("RGB_ALPHA") => return Err(unsupported("the color type Rgba8")),
            Some(custom) => return Err(PnmError::Unsupported(format!("tuple type {custom:?}"))),
            None => {
                return Err(PnmError::Unsupported(
                    "an unrecognized tuple type".to_owned(),
                ));
            }
        };
        Ok(Self {
            reader,
            width,
            height,
            maxval,
            encoding: Encoding::Binary,
            tuple,
        })
    }

    pub(crate) const fn width(&self) -> u32 {
        self.width
    }

    pub(crate) const fn height(&self) -> u32 {
        self.height
    }

    pub(crate) const fn tuple(&self) -> Tuple {
        self.tuple
    }

    pub(crate) fn decode(mut self) -> Result<Samples, PnmError> {
        let samples = self.width as usize * self.height as usize * self.tuple.channels();
        let mut output = vec![0u8; samples * self.tuple.sample_bytes()];
        match self.encoding {
            Encoding::Binary => self.binary(&mut output)?,
            Encoding::Ascii => self.ascii(&mut output)?,
        }
        let target_max = if self.tuple.sample_bytes() == 1 {
            SAMPLE_MAX_8
        } else {
            SAMPLE_MAX_16
        };
        if self.tuple.sample_bytes() == 1 {
            if self.maxval != target_max {
                let factor = target_max as f32 / self.maxval as f32;
                for value in &mut output {
                    *value = (f32::from(*value) * factor).round() as u8;
                }
            }
            return Ok(Samples::Eight(output));
        }
        let mut wide: Vec<u16> = output
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_ne_bytes(*pair))
            .collect();
        if self.maxval != target_max {
            let factor = target_max as f32 / self.maxval as f32;
            for value in &mut wide {
                *value = (f32::from(*value) * factor).round() as u16;
            }
        }
        Ok(Samples::Sixteen(wide))
    }

    fn binary(&mut self, output: &mut [u8]) -> Result<(), PnmError> {
        let width = self.width;
        let height = self.height;
        let components = self.tuple.channels() as u32;
        let count = if self.tuple == Tuple::Bit {
            let bits = width.wrapping_mul(components);
            ((bits / 8) + u32::from(!bits.is_multiple_of(8))).wrapping_mul(height) as usize
        } else {
            width
                .wrapping_mul(height)
                .wrapping_mul(components)
                .wrapping_mul(self.tuple.sample_bytes() as u32) as usize
        };
        let start = self.reader.position.min(self.reader.bytes.len());
        let end = start.saturating_add(count).min(self.reader.bytes.len());
        let raster = &self.reader.bytes[start..end];
        self.reader.position = end;
        if raster.len() != count {
            return Err(PnmError::InputTooShort);
        }
        match self.tuple {
            Tuple::Bit => {
                let row = width as usize * components as usize;
                let expanded = expand_bits(row, raster);
                if expanded.len() != output.len() {
                    return Err(PnmError::InputTooShort);
                }
                for (target, bit) in output.iter_mut().zip(expanded) {
                    *target = !bit;
                }
            }
            Tuple::BlackWhite => {
                if raster.len() != output.len() {
                    return Err(PnmError::InputTooShort);
                }
                output.copy_from_slice(raster);
                if let Some(&value) = output.iter().find(|&&value| value > 1) {
                    return Err(PnmError::SampleOutOfBounds(value));
                }
            }
            Tuple::Gray8 | Tuple::Rgb8 => {
                if raster.len() != output.len() {
                    return Err(PnmError::InputTooShort);
                }
                output.copy_from_slice(raster);
            }
            Tuple::Gray16 | Tuple::Rgb16 => {
                if raster.len() != output.len() {
                    return Err(PnmError::InputTooShort);
                }
                for (target, pair) in output
                    .as_chunks_mut::<2>()
                    .0
                    .iter_mut()
                    .zip(raster.as_chunks::<2>().0)
                {
                    *target = u16::from_be_bytes(*pair).to_ne_bytes();
                }
            }
        }
        Ok(())
    }

    fn ascii(&mut self, output: &mut [u8]) -> Result<(), PnmError> {
        match self.tuple {
            Tuple::Bit => {
                for target in output.iter_mut() {
                    loop {
                        let byte = self.reader.byte().ok_or(PnmError::InputTooShort)?;
                        match byte {
                            byte if WHITESPACE.contains(&byte) => continue,
                            b'0' => *target = BIT_WHITE,
                            b'1' => *target = BIT_BLACK,
                            other => return Err(PnmError::UnexpectedRasterByte(other)),
                        }
                        break;
                    }
                }
            }
            Tuple::Gray8 | Tuple::Rgb8 => {
                for target in output.iter_mut() {
                    *target = u8::try_from(self.reader.ascii_sample()?)
                        .map_err(|_| PnmError::Overflow)?;
                }
            }
            Tuple::Gray16 | Tuple::Rgb16 => {
                for target in output.as_chunks_mut::<2>().0 {
                    *target = self.reader.ascii_sample()?.to_ne_bytes();
                }
            }
            Tuple::BlackWhite => unreachable!("PAM images are binary"),
        }
        Ok(())
    }
}

fn expand_bits(row: usize, raster: &[u8]) -> Vec<u8> {
    let skip = if row.is_multiple_of(8) {
        0
    } else {
        8 - row % 8
    };
    let row_length = row + skip;
    let mut expanded = Vec::with_capacity(raster.len() * 8);
    let mut index = 0usize;
    for &byte in raster {
        for shift in (0..8).rev() {
            if index % row_length < row {
                expanded.push(((byte >> shift) & 1) * BIT_WHITE);
            }
            index += 1;
        }
    }
    expanded
}
