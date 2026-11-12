use super::lzma_common::{DICT_SIZE_MIN, LCLP_MAX, is_lclppb_valid};
use super::options::{
    BcjArchitecture, FILTER_ID_ARM, FILTER_ID_ARM64, FILTER_ID_ARMTHUMB, FILTER_ID_DELTA,
    FILTER_ID_IA64, FILTER_ID_LZMA1, FILTER_ID_LZMA2, FILTER_ID_POWERPC, FILTER_ID_RISCV,
    FILTER_ID_SPARC, FILTER_ID_X86, Filter, LC_DEFAULT, LP_DEFAULT, LzmaOptions, MatchFinder, Mode,
    PB_DEFAULT,
};
use super::stream::Error;
use super::vli;

const FILTER_ID_RESERVED_START: u64 = 1 << 62;
pub(crate) const PROPERTIES_BYTES_MAX: usize = 5;
const LZMA1_PROPERTIES_BYTES: usize = 5;
const LZMA2_PROPERTIES_BYTES: usize = 1;
const DELTA_PROPERTIES_BYTES: usize = 1;
const BCJ_PROPERTIES_BYTES: usize = 4;
const DICT_SIZE_BYTES: usize = 4;
const LZMA2_DICT_PROPERTY_MAX: u8 = 40;
const LZMA2_DICT_PROPERTY_RESERVED_MASK: u8 = 0xC0;
const LZMA2_DICT_SIZE_MANTISSA: u32 = 2;
const LZMA2_DICT_SIZE_SHIFT_BASE: u32 = 11;
const LZMA2_DICT_PROPERTY_SLOT_BASE: u32 = 24;
const DELTA_DISTANCE_MIN: u32 = 1;
const LCLPPB_BYTE_MAX: u8 = ((4 * 5 + 4) * 9 + 8) as u8;
const LC_VALUES: u32 = 9;
const LP_VALUES: u32 = 5;
const DECODER_NICE_LEN: u32 = 64;

pub(crate) const fn decoder_options(dict_size: u32, lc: u32, lp: u32, pb: u32) -> LzmaOptions {
    LzmaOptions {
        dict_size,
        lc,
        lp,
        pb,
        mode: Mode::Normal,
        nice_len: DECODER_NICE_LEN,
        match_finder: MatchFinder::BinaryTree4,
        depth: 0,
    }
}

pub(crate) fn decode_lclppb(byte: u8) -> Option<(u32, u32, u32)> {
    if byte > LCLPPB_BYTE_MAX {
        return None;
    }
    let byte = u32::from(byte);
    let pb = byte / (LC_VALUES * LP_VALUES);
    let rest = byte - pb * LC_VALUES * LP_VALUES;
    let lp = rest / LC_VALUES;
    let lc = rest - lp * LC_VALUES;
    if lc + lp > LCLP_MAX {
        return None;
    }
    Some((lc, lp, pb))
}

pub(crate) fn encode_lclppb(options: &LzmaOptions) -> Option<u8> {
    if !is_lclppb_valid(options.lc, options.lp, options.pb) {
        return None;
    }
    let byte = (options.pb * LP_VALUES + options.lp) * LC_VALUES + options.lc;
    Some(u8::try_from(byte).expect("valid lc, lp and pb fit in one byte"))
}

fn read_u32(bytes: &[u8]) -> u32 {
    let mut word = [0; DICT_SIZE_BYTES];
    word.copy_from_slice(&bytes[..DICT_SIZE_BYTES]);
    u32::from_le_bytes(word)
}

const fn bcj_architecture(id: u64) -> Option<BcjArchitecture> {
    match id {
        FILTER_ID_X86 => Some(BcjArchitecture::X86),
        FILTER_ID_POWERPC => Some(BcjArchitecture::PowerPc),
        FILTER_ID_IA64 => Some(BcjArchitecture::Ia64),
        FILTER_ID_ARM => Some(BcjArchitecture::Arm),
        FILTER_ID_ARMTHUMB => Some(BcjArchitecture::ArmThumb),
        FILTER_ID_SPARC => Some(BcjArchitecture::Sparc),
        FILTER_ID_ARM64 => Some(BcjArchitecture::Arm64),
        FILTER_ID_RISCV => Some(BcjArchitecture::RiscV),
        _ => None,
    }
}

const fn bcj_id(architecture: BcjArchitecture) -> u64 {
    match architecture {
        BcjArchitecture::X86 => FILTER_ID_X86,
        BcjArchitecture::PowerPc => FILTER_ID_POWERPC,
        BcjArchitecture::Ia64 => FILTER_ID_IA64,
        BcjArchitecture::Arm => FILTER_ID_ARM,
        BcjArchitecture::ArmThumb => FILTER_ID_ARMTHUMB,
        BcjArchitecture::Sparc => FILTER_ID_SPARC,
        BcjArchitecture::Arm64 => FILTER_ID_ARM64,
        BcjArchitecture::RiscV => FILTER_ID_RISCV,
    }
}

pub(crate) fn lzma2_dict_size(property: u8) -> u32 {
    assert!(property <= LZMA2_DICT_PROPERTY_MAX);
    if property == LZMA2_DICT_PROPERTY_MAX {
        return u32::MAX;
    }
    let property = u32::from(property);
    (LZMA2_DICT_SIZE_MANTISSA | (property & 1)) << (property / 2 + LZMA2_DICT_SIZE_SHIFT_BASE)
}

fn decode_properties(id: u64, properties: &[u8]) -> Result<Filter, Error> {
    if let Some(architecture) = bcj_architecture(id) {
        let start_offset = match properties.len() {
            0 => 0,
            BCJ_PROPERTIES_BYTES => read_u32(properties),
            _ => return Err(Error::Options),
        };
        return Ok(Filter::Bcj {
            architecture,
            start_offset,
        });
    }
    match id {
        FILTER_ID_LZMA1 => {
            if properties.len() != LZMA1_PROPERTIES_BYTES {
                return Err(Error::Options);
            }
            let (lc, lp, pb) = decode_lclppb(properties[0]).ok_or(Error::Options)?;
            let dict_size = read_u32(&properties[1..]);
            Ok(Filter::Lzma1(decoder_options(dict_size, lc, lp, pb)))
        }
        FILTER_ID_LZMA2 => {
            let [property] = properties else {
                return Err(Error::Options);
            };
            if property & LZMA2_DICT_PROPERTY_RESERVED_MASK != 0
                || *property > LZMA2_DICT_PROPERTY_MAX
            {
                return Err(Error::Options);
            }
            Ok(Filter::Lzma2(decoder_options(
                lzma2_dict_size(*property),
                LC_DEFAULT,
                LP_DEFAULT,
                PB_DEFAULT,
            )))
        }
        FILTER_ID_DELTA => {
            let [property] = properties else {
                return Err(Error::Options);
            };
            Ok(Filter::Delta {
                distance: u32::from(*property) + DELTA_DISTANCE_MIN,
            })
        }
        _ => Err(Error::Options),
    }
}

pub(crate) fn decode(input: &[u8], position: &mut usize) -> Result<Filter, Error> {
    let id = vli::decode_whole(input, position)?;
    if id >= FILTER_ID_RESERVED_START {
        return Err(Error::Data);
    }
    let properties_size = vli::decode_whole(input, position)?;
    let remaining = input.len() - *position;
    if (remaining as u64) < properties_size {
        return Err(Error::Data);
    }
    let properties_size = properties_size as usize;
    let properties = &input[*position..*position + properties_size];
    let result = decode_properties(id, properties);
    *position += properties_size;
    result
}

fn lzma2_dict_property(dict_size: u32) -> u8 {
    let mut rounded = dict_size.max(DICT_SIZE_MIN) - 1;
    rounded |= rounded >> 2;
    rounded |= rounded >> 3;
    rounded |= rounded >> 4;
    rounded |= rounded >> 8;
    rounded |= rounded >> 16;
    if rounded == u32::MAX {
        return LZMA2_DICT_PROPERTY_MAX;
    }
    let value = rounded + 1;
    let bits = u32::BITS - 1 - value.leading_zeros();
    let slot = bits * 2 + ((value >> (bits - 1)) & 1);
    u8::try_from(slot - LZMA2_DICT_PROPERTY_SLOT_BASE)
        .expect("an LZMA2 dictionary property fits in a byte")
}

pub(crate) fn properties(filter: &Filter) -> ([u8; PROPERTIES_BYTES_MAX], usize) {
    let mut bytes = [0; PROPERTIES_BYTES_MAX];
    let size = match filter {
        Filter::Lzma1(options) => {
            bytes[0] = encode_lclppb(options).expect("the encoder validated lc, lp and pb");
            bytes[1..].copy_from_slice(&options.dict_size.to_le_bytes());
            LZMA1_PROPERTIES_BYTES
        }
        Filter::Lzma2(options) => {
            bytes[0] = lzma2_dict_property(options.dict_size);
            LZMA2_PROPERTIES_BYTES
        }
        Filter::Delta { distance } => {
            bytes[0] = u8::try_from(distance - DELTA_DISTANCE_MIN)
                .expect("the encoder validated the delta distance");
            DELTA_PROPERTIES_BYTES
        }
        Filter::Bcj { start_offset, .. } => {
            if *start_offset == 0 {
                0
            } else {
                bytes[..BCJ_PROPERTIES_BYTES].copy_from_slice(&start_offset.to_le_bytes());
                BCJ_PROPERTIES_BYTES
            }
        }
    };
    (bytes, size)
}

const fn properties_size(filter: &Filter) -> usize {
    match filter {
        Filter::Lzma1(_) => LZMA1_PROPERTIES_BYTES,
        Filter::Lzma2(_) => LZMA2_PROPERTIES_BYTES,
        Filter::Delta { .. } => DELTA_PROPERTIES_BYTES,
        Filter::Bcj { start_offset, .. } => {
            if *start_offset == 0 {
                0
            } else {
                BCJ_PROPERTIES_BYTES
            }
        }
    }
}

const fn id(filter: &Filter) -> u64 {
    match filter {
        Filter::Lzma1(_) => FILTER_ID_LZMA1,
        Filter::Lzma2(_) => FILTER_ID_LZMA2,
        Filter::Delta { .. } => FILTER_ID_DELTA,
        Filter::Bcj { architecture, .. } => bcj_id(*architecture),
    }
}

pub(crate) fn size(filter: &Filter) -> Result<usize, Error> {
    let id = id(filter);
    if id >= FILTER_ID_RESERVED_START {
        return Err(Error::Program);
    }
    let properties_size = properties_size(filter);
    Ok(properties_size + vli::size(id) + vli::size(properties_size as u64))
}

pub(crate) fn encode(filter: &Filter, output: &mut [u8], position: &mut usize) {
    vli::encode_whole(id(filter), output, position);
    let (bytes, properties_size) = properties(filter);
    vli::encode_whole(properties_size as u64, output, position);
    output[*position..*position + properties_size].copy_from_slice(&bytes[..properties_size]);
    *position += properties_size;
}
