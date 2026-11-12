use alloc::format;
use alloc::string::String;
use core::fmt;

use crate::marker::Marker;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ComponentPosition {
    Y,
    Cb,
    Cr,
    Q,
}

impl fmt::Debug for ComponentPosition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Y => "Y",
            Self::Cb => "Cb",
            Self::Cr => "Cr",
            Self::Q => "Q",
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    Rgb,
    YCbCr,
    Luma,
    Ycck,
    Cmyk,
}

impl ColorSpace {
    pub(crate) const fn components(self) -> usize {
        match self {
            Self::Luma => 1,
            Self::Rgb | Self::YCbCr => 3,
            Self::Ycck | Self::Cmyk => 4,
        }
    }
}

impl fmt::Debug for ColorSpace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Rgb => "RGB",
            Self::YCbCr => "YCbCr",
            Self::Luma => "Luma",
            Self::Ycck => "YCCK",
            Self::Cmyk => "CMYK",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableClass {
    Dc,
    Ac,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JpegError {
    NoMoreBytes,
    IllegalMagicBytes(u16),
    UnknownMarkerLength(u16),
    MarkerLength(u16),
    PrematureEndOfImage,
    UnsupportedMarker(Marker),
    RestartIntervalLength,
    HuffmanLength,
    HuffmanIndex(usize),
    HuffmanClass(u8),
    ExhaustedData,
    HuffmanSymbolCount,
    HuffmanTableLength {
        symbols: i32,
        length: i32,
    },
    ShortSymbols,
    HuffmanTable,
    HuffmanDefinition,
    QuantizationLength,
    QuantizationTableBytes {
        left: u16,
        needed: usize,
    },
    QuantizationPrecision(usize),
    QuantizationPosition(usize),
    SecondFrame,
    Precision(u8),
    ZeroDimension,
    ZeroComponents,
    FrameLength {
        expected: u16,
        length: u16,
    },
    ShortComponent,
    ComponentPosition(u8),
    QuantizationNumber(u8),
    HorizontalSample(usize),
    VerticalSample(usize),
    ScanLength(u16),
    ScanComponents(u8),
    ScanBeforeFrame,
    DuplicateScanComponent(u8),
    ScanComponentId {
        id: u8,
        count: usize,
    },
    SpectralEnd(u8),
    SpectralStart(u8),
    SuccessiveHigh {
        previous_low: u8,
    },
    SuccessiveLow(u8),
    App13Length,
    App14Length,
    AdobeTransform(u8),
    MissingHuffmanSlot {
        class: TableClass,
        component: ComponentPosition,
    },
    MissingHuffmanTable {
        class: TableClass,
        component: ComponentPosition,
    },
    RestartMarker(Marker),
    MissingQuantizationTable(ComponentPosition),
    ComponentCount {
        expected: usize,
        found: usize,
    },
    HuffmanCode(i32),
    RefineCode,
    ColorConversion {
        input: ColorSpace,
        output: ColorSpace,
    },
    MarkerMissing,
    TooManyScans(usize),
    UnknownMarker(u8),
    ScanCount {
        scans: u8,
        components: usize,
    },
    MergedScan,
    MergedInterleavedScan,
    ScanComponentMissing(usize),
    ProgressiveDcTable,
    HuffmanSlot(usize),
    HuffmanTableMissing(usize),
    InvalidImage,
    InvalidSlice,
    LayoutOutOfBounds,
    RestartAtEnd,
    Allocation,
    PixelBytes {
        bytes: u64,
        max: u64,
    },
    Dimensions {
        width: u32,
        height: u32,
    },
    IccProfileTooLarge(usize),
    PixelLength {
        expected: u64,
        actual: usize,
    },
    Quality(u8),
}

impl JpegError {
    fn quoted(&self) -> Option<String> {
        Some(match self {
            Self::NoMoreBytes => String::from("No more bytes"),
            Self::UnknownMarkerLength(length) => {
                format!("Found a marker with invalid length : {length}")
            }
            Self::MarkerLength(length) => format!("Found a marker with invalid length:{length}\n"),
            Self::PrematureEndOfImage => String::from("Premature End of image"),
            Self::UnsupportedMarker(marker) => format!(
                "Parsing of the following header `{marker:?}` is not supported,cannot continue"
            ),
            Self::RestartIntervalLength => String::from("Bad DRI length, Corrupt JPEG"),
            Self::HuffmanLength => String::from("Invalid Huffman length in image"),
            Self::HuffmanSymbolCount => {
                String::from("Encountered Huffman table with excessive length in DHT")
            }
            Self::ShortSymbols => String::from(
                "Could not read symbols into the buffer\nCould not read into the whole buffer",
            ),
            Self::HuffmanDefinition => String::from("Bogus Huffman table definition"),
            Self::QuantizationLength => {
                String::from("Invalid DQT length. Length should be greater than 2")
            }
            Self::ShortComponent => {
                String::from("Could not read component data\nCould not read into the whole buffer")
            }
            Self::ComponentPosition(position) => {
                format!("Unknown component id found,{position}, expected value between 1 and 4")
            }
            Self::QuantizationNumber(number) => {
                format!("Too large quantization number :{number}, expected value between 0 and 4")
            }
            Self::HorizontalSample(sample) => {
                format!("Horizontal sample is not a power of two({sample}) cannot decode")
            }
            Self::VerticalSample(sample) => {
                format!("Vertical sub-sample is not power of two({sample}) cannot decode")
            }
            Self::ScanBeforeFrame => {
                String::from("Error decoding SOF Marker, Number of components cannot be zero.")
            }
            Self::App13Length => String::from("Too small APP13 length"),
            Self::App14Length => String::from("Too short of a length for App14 segment"),
            Self::AdobeTransform(transform) => format!("Unknown Adobe colorspace {transform}"),
            Self::ComponentCount { expected, found } => {
                format!(" Expected {expected} number of components but found {found}")
            }
            Self::HuffmanCode(symbol) => format!("Bad Huffman Code 0x{symbol:X}, corrupt JPEG"),
            Self::ColorConversion { input, output } => {
                format!("Unimplemented colorspace mapping from {input:?} to {output:?}")
            }
            Self::MarkerMissing => String::from("Marker missing where expected"),
            Self::TooManyScans(limit) => format!("Too many scans, exceeded limit of {limit}"),
            Self::UnknownMarker(byte) => format!("Unknown marker 0xFF{byte:X}"),
            Self::ScanCount { scans, components } => format!(
                "Number of scans {scans} cannot be greater than number of components, {components}"
            ),
            Self::MergedScan => String::from("Can't merge DC and AC corrupt jpeg"),
            Self::ScanComponentMissing(component) => {
                format!("Cannot find component {component}, corrupt image")
            }
            Self::ProgressiveDcTable => String::from("Huffman table at index  {} not initialized"),
            Self::HuffmanSlot(slot) => format!("No huffman table for component:{slot}"),
            Self::HuffmanTableMissing(slot) => {
                format!("Huffman table at index  {slot} not initialized")
            }
            Self::InvalidImage => String::from("Invalid image"),
            Self::InvalidSlice => String::from("Invalid slice , would panic, invalid image"),
            _ => return None,
        })
    }
}

impl fmt::Display for JpegError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(message) = self.quoted() {
            return write!(formatter, "{message:?}");
        }
        let huffman = "Error decoding huffman values: ";
        let quantization = "Error parsing DQT segment. Reason:";
        let scan = "Error parsing SOS Segment. Reason:";
        let frame = "Error parsing SOF segment. Reason:";
        match self {
            Self::IllegalMagicBytes(bytes) => {
                write!(
                    formatter,
                    "Error parsing image. Illegal start bytes:{bytes:X}"
                )
            }
            Self::HuffmanIndex(index) => write!(
                formatter,
                "{huffman}Invalid DHT index {index}, expected between 0 and 3"
            ),
            Self::HuffmanClass(class) => write!(
                formatter,
                "{huffman}Invalid DHT position {class}, should be 0 or 1"
            ),
            Self::ExhaustedData => formatter.write_str("Exhausted data in the image"),
            Self::HuffmanTableLength { symbols, length } => write!(
                formatter,
                "{huffman}Excessive Huffman table of length {symbols} found when header length is {length}"
            ),
            Self::HuffmanTable => write!(formatter, "{huffman}Bad Huffman Table"),
            Self::QuantizationTableBytes { left, needed } => write!(
                formatter,
                "{quantization}Invalid QT table bytes left :{left}. Too small to construct a valid qt table which should be {needed} long"
            ),
            Self::QuantizationPrecision(precision) => write!(
                formatter,
                "{quantization}Expected QT precision value of either 0 or 1, found {precision}"
            ),
            Self::QuantizationPosition(position) => write!(
                formatter,
                "{quantization}Too large table position for QT :{position}, expected between 0 and 3"
            ),
            Self::SecondFrame => write!(formatter, "{frame}Two Start of Frame Markers"),
            Self::Precision(precision) => write!(
                formatter,
                "{frame}The library can only parse 8-bit images, the image has {precision} bits of precision"
            ),
            Self::ZeroDimension => {
                formatter.write_str("Image width or height is set to zero, cannot continue")
            }
            Self::ZeroComponents => {
                write!(formatter, "{frame}Number of components cannot be zero.")
            }
            Self::FrameLength { expected, length } => write!(
                formatter,
                "{frame}Length of start of frame differs from expected {expected},value is {length}"
            ),
            Self::ScanLength(length) => {
                write!(formatter, "{scan}Bad SOS length {length},corrupt jpeg")
            }
            Self::ScanComponents(count) => write!(
                formatter,
                "{scan}Number of components in start of scan should be less than 3 but more than 0. Found {count}"
            ),
            Self::DuplicateScanComponent(id) => write!(
                formatter,
                "{frame}Duplicate ID {id} seen twice in the same component"
            ),
            Self::ScanComponentId { id, count } => write!(
                formatter,
                "{frame}Invalid component id {id}, expected a value between 0 and {count}"
            ),
            Self::SpectralEnd(value) => write!(
                formatter,
                "{scan}Invalid Se parameter {value}, range should be 0-63"
            ),
            Self::SpectralStart(value) => write!(
                formatter,
                "{scan}Invalid Ss parameter {value}, range should be 0-63"
            ),
            Self::SuccessiveHigh { previous_low } => write!(
                formatter,
                "{scan}Invalid Ah parameter {previous_low}, range should be 0-13"
            ),
            Self::SuccessiveLow(value) => write!(
                formatter,
                "{scan}Invalid Al parameter {value}, range should be 0-13"
            ),
            Self::MissingHuffmanSlot { class, component } => write!(
                formatter,
                "{huffman}No Huffman {} table for component {component:?} ",
                class_name(*class)
            ),
            Self::MissingHuffmanTable { class, component } => write!(
                formatter,
                "{huffman}No {} table for component {component:?}",
                class_name(*class)
            ),
            Self::RestartMarker(marker) => write!(
                formatter,
                "Error in decoding MCU. Reason Marker {marker:?} found in bitstream, possibly corrupt jpeg"
            ),
            Self::MissingQuantizationTable(component) => write!(
                formatter,
                "{quantization}No quantization table for component {component:?}"
            ),
            Self::RefineCode => write!(formatter, "{huffman}Bad Huffman code, corrupt JPEG?"),
            Self::MergedInterleavedScan => {
                write!(formatter, "{huffman}Can't merge dc and AC corrupt jpeg")
            }
            Self::LayoutOutOfBounds => formatter.write_str(
                "the sampling factors and scans of the image do not fit its component buffers",
            ),
            Self::Allocation => {
                formatter.write_str("the decoder's buffers for this image could not be allocated")
            }
            Self::RestartAtEnd => formatter
                .write_str("the image ends after a restart marker that follows a progressive scan"),
            Self::PixelBytes { bytes, max } => write!(
                formatter,
                "decoded pixels take {bytes} bytes, more than the {max} allowed"
            ),
            Self::Dimensions { width, height } => write!(
                formatter,
                "a {width}x{height} image exceeds the 65535 pixel limit of JPEG dimensions"
            ),
            Self::IccProfileTooLarge(bytes) => write!(
                formatter,
                "an ICC profile of {bytes} bytes exceeds the 255 APP2 segments JPEG can hold"
            ),
            Self::PixelLength { expected, actual } => write!(
                formatter,
                "pixel buffer holds {actual} bytes where the dimensions need {expected}"
            ),
            Self::Quality(quality) => {
                write!(formatter, "JPEG quality {quality} is outside 1 to 100")
            }
            _ => unreachable!("quoted errors return above"),
        }
    }
}

const fn class_name(class: TableClass) -> &'static str {
    match class {
        TableClass::Dc => "DC",
        TableClass::Ac => "AC",
    }
}

impl core::error::Error for JpegError {}
