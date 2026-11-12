use super::stream::Error;

pub const FILTER_ID_LZMA1: u64 = 0x4000_0000_0000_0001;
pub const FILTER_ID_LZMA2: u64 = 0x21;
pub const FILTER_ID_DELTA: u64 = 0x03;
pub const FILTER_ID_X86: u64 = 0x04;
pub const FILTER_ID_POWERPC: u64 = 0x05;
pub const FILTER_ID_IA64: u64 = 0x06;
pub const FILTER_ID_ARM: u64 = 0x07;
pub const FILTER_ID_ARMTHUMB: u64 = 0x08;
pub const FILTER_ID_SPARC: u64 = 0x09;
pub const FILTER_ID_ARM64: u64 = 0x0A;
pub const FILTER_ID_RISCV: u64 = 0x0B;
pub const PRESET_DEFAULT: u32 = 6;
pub const PRESET_LEVEL_MASK: u32 = 0x1F;
pub const PRESET_EXTREME: u32 = 1 << 31;
const PRESET_LEVEL_MAX: u32 = 9;
const PRESET_DICT_SIZE_POWERS: [u32; 10] = [18, 20, 21, 22, 22, 23, 23, 24, 25, 26];
const PRESET_FAST_LEVEL_MAX: u32 = 3;
const PRESET_FAST_DEPTHS: [u32; 4] = [4, 8, 24, 48];
const PRESET_FAST_SHORT_NICE_LEVEL_MAX: u32 = 1;
const PRESET_FAST_SHORT_NICE_LEN: u32 = 128;
const PRESET_NICE_LEN_MAX: u32 = 273;
const PRESET_LEVEL_4: u32 = 4;
const PRESET_LEVEL_4_NICE_LEN: u32 = 16;
const PRESET_LEVEL_5: u32 = 5;
const PRESET_LEVEL_5_NICE_LEN: u32 = 32;
const PRESET_NORMAL_NICE_LEN: u32 = 64;
const PRESET_EXTREME_LEVEL_3: u32 = 3;
const PRESET_EXTREME_SHORT_NICE_LEN: u32 = 192;
const PRESET_EXTREME_DEPTH: u32 = 512;
pub const LC_DEFAULT: u32 = 3;
pub const LP_DEFAULT: u32 = 0;
pub const PB_DEFAULT: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Fast,
    Normal,
}

impl Mode {
    pub const fn id(self) -> u32 {
        match self {
            Self::Fast => 1,
            Self::Normal => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchFinder {
    HashChain3,
    HashChain4,
    BinaryTree2,
    BinaryTree3,
    BinaryTree4,
}

impl MatchFinder {
    pub const fn id(self) -> u32 {
        match self {
            Self::HashChain3 => 0x03,
            Self::HashChain4 => 0x04,
            Self::BinaryTree2 => 0x12,
            Self::BinaryTree3 => 0x13,
            Self::BinaryTree4 => 0x14,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LzmaOptions {
    pub dict_size: u32,
    pub lc: u32,
    pub lp: u32,
    pub pb: u32,
    pub mode: Mode,
    pub nice_len: u32,
    pub match_finder: MatchFinder,
    pub depth: u32,
}

impl LzmaOptions {
    pub const fn preset(preset: u32) -> Result<Self, Error> {
        let level = preset & PRESET_LEVEL_MASK;
        let flags = preset & !PRESET_LEVEL_MASK;
        if level > PRESET_LEVEL_MAX || flags & !PRESET_EXTREME != 0 {
            return Err(Error::Options);
        }
        let mut options = if level <= PRESET_FAST_LEVEL_MAX {
            Self {
                dict_size: 1 << PRESET_DICT_SIZE_POWERS[level as usize],
                lc: LC_DEFAULT,
                lp: LP_DEFAULT,
                pb: PB_DEFAULT,
                mode: Mode::Fast,
                nice_len: if level <= PRESET_FAST_SHORT_NICE_LEVEL_MAX {
                    PRESET_FAST_SHORT_NICE_LEN
                } else {
                    PRESET_NICE_LEN_MAX
                },
                match_finder: if level == 0 {
                    MatchFinder::HashChain3
                } else {
                    MatchFinder::HashChain4
                },
                depth: PRESET_FAST_DEPTHS[level as usize],
            }
        } else {
            Self {
                dict_size: 1 << PRESET_DICT_SIZE_POWERS[level as usize],
                lc: LC_DEFAULT,
                lp: LP_DEFAULT,
                pb: PB_DEFAULT,
                mode: Mode::Normal,
                nice_len: match level {
                    PRESET_LEVEL_4 => PRESET_LEVEL_4_NICE_LEN,
                    PRESET_LEVEL_5 => PRESET_LEVEL_5_NICE_LEN,
                    _ => PRESET_NORMAL_NICE_LEN,
                },
                match_finder: MatchFinder::BinaryTree4,
                depth: 0,
            }
        };
        if flags & PRESET_EXTREME != 0 {
            options.mode = Mode::Normal;
            options.match_finder = MatchFinder::BinaryTree4;
            if level == PRESET_EXTREME_LEVEL_3 || level == PRESET_LEVEL_5 {
                options.nice_len = PRESET_EXTREME_SHORT_NICE_LEN;
                options.depth = 0;
            } else {
                options.nice_len = PRESET_NICE_LEN_MAX;
                options.depth = PRESET_EXTREME_DEPTH;
            }
        }
        Ok(options)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    None,
    Crc32,
    Crc64,
    Sha256,
}

impl Check {
    pub const fn id(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Crc32 => 1,
            Self::Crc64 => 4,
            Self::Sha256 => 10,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BcjArchitecture {
    X86,
    PowerPc,
    Ia64,
    Arm,
    ArmThumb,
    Sparc,
    Arm64,
    RiscV,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Lzma1(LzmaOptions),
    Lzma2(LzmaOptions),
    Delta {
        distance: u32,
    },
    Bcj {
        architecture: BcjArchitecture,
        start_offset: u32,
    },
}

impl Filter {
    pub(crate) const fn is_lzma(&self) -> bool {
        matches!(self, Self::Lzma1(_) | Self::Lzma2(_))
    }
}
