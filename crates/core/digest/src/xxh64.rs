const PRIME64_1: u64 = 0x9E37_79B1_85EB_CA87;
const PRIME64_2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const PRIME64_3: u64 = 0x1656_67B1_9E37_79F9;
const PRIME64_4: u64 = 0x85EB_CA77_C2B2_AE63;
const PRIME64_5: u64 = 0x27D4_EB2F_1656_67C5;
const STRIPE_BYTES: usize = 32;
const LANE_BYTES: usize = 8;
const HALF_LANE_BYTES: usize = 4;

pub fn xxh64(bytes: &[u8], seed: u64) -> u64 {
    let (stripes, tail) = bytes.as_chunks::<STRIPE_BYTES>();
    let mut hash = if bytes.len() >= STRIPE_BYTES {
        let mut accumulators = [
            seed.wrapping_add(PRIME64_1).wrapping_add(PRIME64_2),
            seed.wrapping_add(PRIME64_2),
            seed,
            seed.wrapping_sub(PRIME64_1),
        ];
        for stripe in stripes {
            for (accumulator, lane) in accumulators
                .iter_mut()
                .zip(stripe.as_chunks::<LANE_BYTES>().0)
            {
                *accumulator = round(*accumulator, u64::from_le_bytes(*lane));
            }
        }
        let [first, second, third, fourth] = accumulators;
        let mut converged = first
            .rotate_left(1)
            .wrapping_add(second.rotate_left(7))
            .wrapping_add(third.rotate_left(12))
            .wrapping_add(fourth.rotate_left(18));
        for accumulator in accumulators {
            converged = merge_round(converged, accumulator);
        }
        converged
    } else {
        seed.wrapping_add(PRIME64_5)
    };
    hash = hash.wrapping_add(bytes.len() as u64);
    let (lanes, mut rest) = tail.as_chunks::<LANE_BYTES>();
    for lane in lanes {
        hash ^= round(0, u64::from_le_bytes(*lane));
        hash = hash
            .rotate_left(27)
            .wrapping_mul(PRIME64_1)
            .wrapping_add(PRIME64_4);
    }
    if let Some((half, after)) = rest.split_first_chunk::<HALF_LANE_BYTES>() {
        hash ^= u64::from(u32::from_le_bytes(*half)).wrapping_mul(PRIME64_1);
        hash = hash
            .rotate_left(23)
            .wrapping_mul(PRIME64_2)
            .wrapping_add(PRIME64_3);
        rest = after;
    }
    for &byte in rest {
        hash ^= u64::from(byte).wrapping_mul(PRIME64_5);
        hash = hash.rotate_left(11).wrapping_mul(PRIME64_1);
    }
    avalanche(hash)
}

#[inline(always)]
const fn round(accumulator: u64, lane: u64) -> u64 {
    accumulator
        .wrapping_add(lane.wrapping_mul(PRIME64_2))
        .rotate_left(31)
        .wrapping_mul(PRIME64_1)
}

const fn merge_round(accumulator: u64, value: u64) -> u64 {
    (accumulator ^ round(0, value))
        .wrapping_mul(PRIME64_1)
        .wrapping_add(PRIME64_4)
}

const fn avalanche(mut hash: u64) -> u64 {
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(PRIME64_2);
    hash ^= hash >> 29;
    hash = hash.wrapping_mul(PRIME64_3);
    hash ^= hash >> 32;
    hash
}
