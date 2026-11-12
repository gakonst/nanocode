use alloc::vec::Vec;
use core::fmt;

mod bits;
mod block;
mod compressor;
mod dictionary;
mod huffman;
mod tables;

use compressor::{Compressor, Flush, Parsing, Settings, Sink};
use tables::{
    GREEDY_LEVEL_MAX, LEVEL_PROBES, ZLIB_BEST_PROBES, ZLIB_CHECK_DIVISOR, ZLIB_LEVEL_MASK,
    ZLIB_LEVEL_SHIFT, ZLIB_METHOD, ZLIB_WINDOW_INFO,
};

const LEVEL_STORED: u8 = 0;
const LEVEL_FAST: u8 = 1;
const LEVEL_DEFAULT: u8 = 6;
const LEVEL_BEST: u8 = 9;
pub const LEVEL_MAX: u8 = 10;
const WRITER_BUFFER_BYTES: usize = 32 * 1024;
const ZLIB_LEVEL_FASTEST: u32 = 0;
const ZLIB_LEVEL_FAST: u32 = 1;
const ZLIB_LEVEL_DEFAULT: u32 = 2;
const ZLIB_LEVEL_MAXIMUM: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Raw,
    Zlib,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level(u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelError {
    pub level: u32,
}

impl fmt::Display for LevelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "compression level {} is above the maximum {LEVEL_MAX}",
            self.level
        )
    }
}

impl core::error::Error for LevelError {}

impl Level {
    pub const STORED: Self = Self(LEVEL_STORED);
    pub const FAST: Self = Self(LEVEL_FAST);
    pub const DEFAULT: Self = Self(LEVEL_DEFAULT);
    pub const BEST: Self = Self(LEVEL_BEST);

    pub const fn new(level: u32) -> Result<Self, LevelError> {
        if level > LEVEL_MAX as u32 {
            return Err(LevelError { level });
        }
        Ok(Self(level as u8))
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    fn settings(self, format: Format) -> Settings {
        let probes = LEVEL_PROBES[usize::from(self.0)];
        let parsing = match self.0 {
            LEVEL_STORED => Parsing::Stored,
            LEVEL_FAST => Parsing::Fast,
            level if level <= GREEDY_LEVEL_MAX => Parsing::Greedy,
            _ => Parsing::Lazy,
        };
        Settings {
            format,
            parsing,
            probes: [1 + probes.div_ceil(3), 1 + (probes >> 2).div_ceil(3)],
            zlib_header: zlib_header(parsing, probes),
        }
    }
}

fn zlib_header(parsing: Parsing, probes: u32) -> [u8; 2] {
    let level = match parsing {
        Parsing::Stored | Parsing::Fast | Parsing::Greedy if probes <= 1 => ZLIB_LEVEL_FASTEST,
        Parsing::Stored | Parsing::Fast | Parsing::Greedy => ZLIB_LEVEL_FAST,
        Parsing::Lazy if probes >= ZLIB_BEST_PROBES => ZLIB_LEVEL_MAXIMUM,
        Parsing::Lazy => ZLIB_LEVEL_DEFAULT,
    };
    let method = match parsing {
        Parsing::Stored => ZLIB_METHOD,
        Parsing::Fast | Parsing::Greedy | Parsing::Lazy => ZLIB_METHOD | ZLIB_WINDOW_INFO,
    };
    let flags = level << ZLIB_LEVEL_SHIFT;
    let remainder = ((u32::from(method) << u8::BITS) + flags) % ZLIB_CHECK_DIVISOR;
    [
        method,
        ((flags & ZLIB_LEVEL_MASK) + (ZLIB_CHECK_DIVISOR - remainder)) as u8,
    ]
}

pub struct Writer {
    compressor: Compressor,
    output: Vec<u8>,
    buffered: usize,
}

impl Writer {
    pub fn new(level: Level, format: Format) -> Self {
        Self {
            compressor: Compressor::new(level.settings(format)),
            output: Vec::new(),
            buffered: 0,
        }
    }

    pub fn write(&mut self, data: &[u8]) {
        let mut source = 0;
        while source < data.len() {
            self.buffered = 0;
            let progress = self.run(&data[source..], WRITER_BUFFER_BYTES, Flush::None);
            assert!(progress.consumed != 0 || progress.written != 0);
            assert!(!progress.stream_end);
            source += progress.consumed;
        }
    }

    pub fn flush(&mut self) -> Vec<u8> {
        self.run(&[], WRITER_BUFFER_BYTES - self.buffered, Flush::Sync);
        self.drain(Flush::None);
        core::mem::take(&mut self.output)
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.drain(Flush::Finish);
        self.output
    }

    fn drain(&mut self, flush: Flush) {
        loop {
            self.buffered = 0;
            if self.run(&[], WRITER_BUFFER_BYTES, flush).written == 0 {
                return;
            }
        }
    }

    fn run(&mut self, input: &[u8], space: usize, flush: Flush) -> compressor::Progress {
        let mut sink = Sink {
            bytes: &mut self.output,
            space,
        };
        let progress = self.compressor.deflate(input, &mut sink, flush);
        self.buffered += progress.written;
        progress
    }
}

pub fn compress(data: &[u8], level: Level, format: Format) -> Vec<u8> {
    let mut writer = Writer::new(level, format);
    writer.write(data);
    writer.finish()
}

pub fn compress_sync_flush(data: &[u8], level: Level, format: Format) -> Vec<u8> {
    let mut writer = Writer::new(level, format);
    writer.write(data);
    writer.flush()
}
