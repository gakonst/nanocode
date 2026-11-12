use super::sha256::Sha256;
use super::{crc32, crc64};

pub(crate) const CHECK_ID_MAX: u32 = 15;
pub(crate) const CHECK_ID_NONE: u32 = 0;
const CHECK_ID_CRC32: u32 = 1;
const CHECK_ID_CRC64: u32 = 4;
const CHECK_ID_SHA256: u32 = 10;
pub(crate) const CHECK_BYTES_MAX: usize = 64;
const CHECK_BYTES: [usize; 16] = [0, 4, 4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 32, 64, 64, 64];
const CRC32_BYTES: usize = 4;
const CRC64_BYTES: usize = 8;
const SHA256_BYTES: usize = 32;

pub(crate) fn check_bytes(check_id: u32) -> usize {
    assert!(check_id <= CHECK_ID_MAX);
    CHECK_BYTES[check_id as usize]
}

pub(crate) enum CheckState {
    None,
    Crc32(u32),
    Crc64(u64),
    Sha256(Sha256),
}

impl CheckState {
    pub(crate) fn new(check_id: u32) -> Self {
        assert!(check_id <= CHECK_ID_MAX);
        match check_id {
            CHECK_ID_CRC32 => Self::Crc32(0),
            CHECK_ID_CRC64 => Self::Crc64(0),
            CHECK_ID_SHA256 => Self::Sha256(Sha256::new()),
            _ => Self::None,
        }
    }

    pub(crate) fn update(&mut self, input: &[u8]) {
        match self {
            Self::None => {}
            Self::Crc32(crc) => *crc = crc32::update(*crc, input),
            Self::Crc64(crc) => *crc = crc64::update(*crc, input),
            Self::Sha256(sha256) => sha256.update(input),
        }
    }

    pub(crate) fn finish(self) -> [u8; CHECK_BYTES_MAX] {
        let mut buffer = [0; CHECK_BYTES_MAX];
        match self {
            Self::None => {}
            Self::Crc32(crc) => buffer[..CRC32_BYTES].copy_from_slice(&crc.to_le_bytes()),
            Self::Crc64(crc) => buffer[..CRC64_BYTES].copy_from_slice(&crc.to_le_bytes()),
            Self::Sha256(sha256) => buffer[..SHA256_BYTES].copy_from_slice(&sha256.finish()),
        }
        buffer
    }
}
