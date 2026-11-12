use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use crate::checksum::{adler32, crc32};

mod bits;
mod block;
mod decode;
mod fast;
mod header;
mod output;
mod table;
mod window;

use bits::Input;
use output::Output;
use table::{
    CODE_LENGTH_TABLE_ENTRIES, CODE_LENGTHS_CAPACITY, Code, DISTANCE_TABLE_ENTRIES,
    FIXED_DISTANCES, FIXED_LENGTHS, FIXED_LITERALS, LENGTH_TABLE_ENTRIES, LITERAL_ENTRIES,
    Literals, WORK_ENTRIES,
};
use window::Window;

const WINDOW_BITS_MIN: i32 = 8;
const WINDOW_BITS_MAX: i32 = 15;
const WINDOW_BITS_MASK: i32 = 15;
const GZIP_WINDOW_BITS_OFFSET: i32 = 16;
const AUTO_WINDOW_BITS_OFFSET: i32 = 32;
const WINDOW_BITS_FORMAT_END: i32 = 48;
const ADLER32_INITIAL: u32 = 1;
const CRC32_INITIAL: u32 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InflateFlush {
    NoFlush,
    SyncFlush,
    Finish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InflateStatus {
    Ok,
    StreamEnd,
    BufError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InflateError {
    NeedDict { dictionary_id: u32 },
    Stream,
    Data,
    Memory,
}

impl InflateError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NeedDict { .. } => "need dictionary",
            Self::Stream => "stream error",
            Self::Data => "data error",
            Self::Memory => "insufficient memory",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Raw,
    Zlib,
    Gzip,
    Auto,
}

impl Format {
    const fn accepts_zlib(self) -> bool {
        matches!(self, Self::Zlib | Self::Auto)
    }

    const fn accepts_gzip(self) -> bool {
        matches!(self, Self::Gzip | Self::Auto)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Wrapper {
    Unknown,
    Zlib,
    Gzip { flags: u16 },
}

#[derive(Clone, Copy)]
struct TableSizes {
    lengths: usize,
    distances: usize,
    code_lengths: usize,
}

#[derive(Clone, Copy)]
enum Mode {
    Header,
    GzipFlags,
    GzipTime,
    GzipOs,
    GzipExtraLength,
    GzipExtra {
        remaining: usize,
    },
    GzipName,
    GzipComment,
    GzipHeaderCrc,
    DictionaryId,
    Dictionary,
    BlockHeader,
    StoredLengths,
    StoredCopy {
        remaining: usize,
    },
    TableSizes,
    CodeLengthCodes {
        sizes: TableSizes,
        have: usize,
    },
    CodeLengths {
        sizes: TableSizes,
        have: usize,
    },
    Length,
    Literal {
        byte: u8,
    },
    LengthExtra {
        length: usize,
        extra: u32,
    },
    Distance {
        length: usize,
    },
    DistanceExtra {
        length: usize,
        distance: usize,
        extra: u32,
    },
    Match {
        length: usize,
        distance: usize,
    },
    Check,
    TotalLength,
    Done,
    Bad,
}

#[derive(Clone, Copy)]
enum LengthSource {
    Fixed,
    CodeLengths,
    Dynamic,
}

#[derive(Clone, Copy)]
enum DistanceSource {
    Fixed,
    Dynamic,
}

enum Stop {
    Suspend,
    StreamEnd,
    NeedDictionary,
    Data,
}

type Step = Result<(), Stop>;

#[derive(Clone)]
struct Tables {
    code_lengths: [Code; CODE_LENGTH_TABLE_ENTRIES],
    lengths: [Code; LENGTH_TABLE_ENTRIES],
    distances: [Code; DISTANCE_TABLE_ENTRIES],
    literals: [Option<Literals>; LITERAL_ENTRIES],
    lens: [u16; CODE_LENGTHS_CAPACITY],
    work: [u16; WORK_ENTRIES],
}

#[derive(Clone)]
pub struct Inflate {
    mode: Mode,
    format: Format,
    window_bits: Option<u32>,
    wrapper: Wrapper,
    last_block: bool,
    have_dictionary: bool,
    hold: u64,
    bits: u32,
    checksum: u32,
    crc: u32,
    total: u64,
    total_in: u64,
    message: Option<&'static str>,
    length_source: LengthSource,
    length_bits: u32,
    distance_source: DistanceSource,
    distance_bits: u32,
    tables: Box<Tables>,
    window: Window,
}

impl Inflate {
    pub fn new(window_bits: i32) -> Result<Self, InflateError> {
        let (format, bits) = if window_bits < 0 {
            if window_bits < -WINDOW_BITS_MAX {
                return Err(InflateError::Stream);
            }
            (Format::Raw, -window_bits)
        } else {
            let format = match window_bits {
                0..GZIP_WINDOW_BITS_OFFSET => Format::Zlib,
                GZIP_WINDOW_BITS_OFFSET..AUTO_WINDOW_BITS_OFFSET => Format::Gzip,
                AUTO_WINDOW_BITS_OFFSET..WINDOW_BITS_FORMAT_END => Format::Auto,
                _ => return Err(InflateError::Stream),
            };
            (format, window_bits & WINDOW_BITS_MASK)
        };
        if bits != 0 && !(WINDOW_BITS_MIN..=WINDOW_BITS_MAX).contains(&bits) {
            return Err(InflateError::Stream);
        }
        let window_bits =
            (bits != 0).then(|| u32::try_from(bits).expect("a checked window size is positive"));
        Ok(Self {
            mode: Mode::Header,
            format,
            window_bits,
            wrapper: Wrapper::Unknown,
            last_block: false,
            have_dictionary: false,
            hold: 0,
            bits: 0,
            checksum: ADLER32_INITIAL,
            crc: CRC32_INITIAL,
            total: 0,
            total_in: 0,
            message: None,
            length_source: LengthSource::Fixed,
            length_bits: 0,
            distance_source: DistanceSource::Fixed,
            distance_bits: 0,
            tables: Box::new(Tables {
                code_lengths: [Code::EMPTY; CODE_LENGTH_TABLE_ENTRIES],
                lengths: [Code::EMPTY; LENGTH_TABLE_ENTRIES],
                distances: [Code::EMPTY; DISTANCE_TABLE_ENTRIES],
                literals: [None; LITERAL_ENTRIES],
                lens: [0; CODE_LENGTHS_CAPACITY],
                work: [0; WORK_ENTRIES],
            }),
            window: Window::new(),
        })
    }

    pub fn inflate(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        flush: InflateFlush,
    ) -> Result<InflateStatus, InflateError> {
        let mut input = Input::new(input, self.hold, self.bits);
        let mut output = Output::new(output);
        let stop = self.run(&mut input, &mut output);
        let read = input.position();
        (self.hold, self.bits) = input.buffer();
        let counted = output.counted();
        let written = counted.len();
        self.total_in += u64::try_from(read).expect("a slice length fits in 64 bits");
        self.total = self
            .total
            .wrapping_add(u64::try_from(written).expect("a slice length fits in 64 bits"));
        let dictionary_id = self.checksum;
        self.record_output(counted);
        match stop {
            Stop::Suspend if (read == 0 && written == 0) || flush == InflateFlush::Finish => {
                Ok(InflateStatus::BufError)
            }
            Stop::Suspend => Ok(InflateStatus::Ok),
            Stop::StreamEnd => Ok(InflateStatus::StreamEnd),
            Stop::NeedDictionary => Err(InflateError::NeedDict { dictionary_id }),
            Stop::Data => Err(InflateError::Data),
        }
    }

    pub fn set_dictionary(&mut self, dictionary: &[u8]) -> Result<(), InflateError> {
        let waiting = matches!(self.mode, Mode::Dictionary);
        if self.format != Format::Raw && !waiting {
            return Err(InflateError::Stream);
        }
        if waiting && adler32(ADLER32_INITIAL, dictionary) != self.checksum {
            return Err(InflateError::Data);
        }
        self.window.extend(dictionary);
        self.have_dictionary = true;
        Ok(())
    }

    pub const fn total_in(&self) -> u64 {
        self.total_in
    }

    pub const fn total_out(&self) -> u64 {
        self.total
    }

    pub const fn message(&self) -> Option<&'static str> {
        self.message
    }

    fn record_output(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if self.format != Format::Raw {
            match self.wrapper {
                Wrapper::Zlib => self.checksum = adler32(self.checksum, data),
                Wrapper::Unknown | Wrapper::Gzip { .. } => self.crc = crc32(self.crc, data),
            }
        }
        self.window.extend(data);
    }

    const fn fail(&mut self, message: &'static str) -> Stop {
        self.mode = Mode::Bad;
        self.message = Some(message);
        Stop::Data
    }

    fn run(&mut self, input: &mut Input<'_>, output: &mut Output<'_>) -> Stop {
        loop {
            let step = match self.mode {
                Mode::Header => self.header(input),
                Mode::GzipFlags => self.gzip_flags(input),
                Mode::GzipTime => self.gzip_time(input),
                Mode::GzipOs => self.gzip_os(input),
                Mode::GzipExtraLength => self.gzip_extra_length(input),
                Mode::GzipExtra { remaining } => self.gzip_extra(input, remaining),
                Mode::GzipName => self.gzip_name(input),
                Mode::GzipComment => self.gzip_comment(input),
                Mode::GzipHeaderCrc => self.gzip_header_crc(input),
                Mode::DictionaryId => self.dictionary_id(input),
                Mode::Dictionary => self.dictionary(),
                Mode::BlockHeader => self.block_header(input),
                Mode::StoredLengths => self.stored_lengths(input),
                Mode::StoredCopy { remaining } => self.stored_copy(input, output, remaining),
                Mode::TableSizes => self.table_sizes(input),
                Mode::CodeLengthCodes { sizes, have } => self.code_length_codes(input, sizes, have),
                Mode::CodeLengths { sizes, have } => self.code_lengths(input, sizes, have),
                Mode::Length => self.length(input, output),
                Mode::Literal { byte } => self.literal(output, byte),
                Mode::LengthExtra { length, extra } => self.length_extra(input, length, extra),
                Mode::Distance { length } => self.distance(input, length),
                Mode::DistanceExtra {
                    length,
                    distance,
                    extra,
                } => self.distance_extra(input, length, distance, extra),
                Mode::Match { length, distance } => self.copy(output, length, distance),
                Mode::Check => self.check(input, output),
                Mode::TotalLength => self.total_length(input),
                Mode::Done => Err(Stop::StreamEnd),
                Mode::Bad => {
                    self.message = Some("repeated call with bad state");
                    Err(Stop::Data)
                }
            };
            if let Err(stop) = step {
                return stop;
            }
        }
    }

    fn length_codes(&self) -> &[Code] {
        match self.length_source {
            LengthSource::Fixed => &FIXED_LENGTHS,
            LengthSource::CodeLengths => &self.tables.code_lengths,
            LengthSource::Dynamic => &self.tables.lengths,
        }
    }

    fn literals(&self) -> &[Option<Literals>; LITERAL_ENTRIES] {
        match self.length_source {
            LengthSource::Fixed => &FIXED_LITERALS,
            LengthSource::CodeLengths => {
                panic!("literal entries exist only for literal/length codes")
            }
            LengthSource::Dynamic => &self.tables.literals,
        }
    }

    fn distance_codes(&self) -> &[Code] {
        match self.distance_source {
            DistanceSource::Fixed => &FIXED_DISTANCES,
            DistanceSource::Dynamic => &self.tables.distances,
        }
    }
}

pub fn inflate_all(
    window_bits: i32,
    input: &[u8],
    output_bytes: usize,
) -> Result<(InflateStatus, Vec<u8>), InflateError> {
    let mut inflate = Inflate::new(window_bits)?;
    let mut output = vec![0; output_bytes];
    let status = inflate.inflate(input, &mut output, InflateFlush::NoFlush)?;
    let produced =
        usize::try_from(inflate.total_out()).expect("one call produces at most its output slice");
    output.truncate(produced);
    Ok((status, output))
}
