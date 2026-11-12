use core::fmt;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    StartOfFrame(u8),
    DefineHuffmanTable,
    DefineArithmeticConditioning,
    Restart(u8),
    StartOfImage,
    EndOfImage,
    StartOfScan,
    DefineQuantizationTable,
    DefineNumberOfLines,
    DefineRestartInterval,
    Application(u8),
    Comment,
}

impl Marker {
    pub(crate) const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0xFE => Some(Self::Comment),
            0xC0 => Some(Self::StartOfFrame(0)),
            0xC1 => Some(Self::StartOfFrame(1)),
            0xC2 => Some(Self::StartOfFrame(2)),
            0xC4 => Some(Self::DefineHuffmanTable),
            0xCC => Some(Self::DefineArithmeticConditioning),
            0xD0..=0xD7 => Some(Self::Restart(byte - 0xD0)),
            0xD8 => Some(Self::StartOfImage),
            0xD9 => Some(Self::EndOfImage),
            0xDA => Some(Self::StartOfScan),
            0xDB => Some(Self::DefineQuantizationTable),
            0xDC => Some(Self::DefineNumberOfLines),
            0xDD => Some(Self::DefineRestartInterval),
            0xE0 => Some(Self::Application(0)),
            0xE1 => Some(Self::Application(1)),
            0xE2 => Some(Self::Application(2)),
            0xED => Some(Self::Application(13)),
            0xEE => Some(Self::Application(14)),
            _ => None,
        }
    }
}

impl fmt::Debug for Marker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StartOfFrame(number) => write!(formatter, "SOF({number})"),
            Self::DefineHuffmanTable => formatter.write_str("DHT"),
            Self::DefineArithmeticConditioning => formatter.write_str("DAC"),
            Self::Restart(number) => write!(formatter, "RST({number})"),
            Self::StartOfImage => formatter.write_str("SOI"),
            Self::EndOfImage => formatter.write_str("EOI"),
            Self::StartOfScan => formatter.write_str("SOS"),
            Self::DefineQuantizationTable => formatter.write_str("DQT"),
            Self::DefineNumberOfLines => formatter.write_str("DNL"),
            Self::DefineRestartInterval => formatter.write_str("DRI"),
            Self::Application(number) => write!(formatter, "APP({number})"),
            Self::Comment => formatter.write_str("COM"),
        }
    }
}
