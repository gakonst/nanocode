pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;
const SPLITMIX_GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;
const SPLITMIX_MIX_FIRST: u64 = 0xbf58_476d_1ce4_e5b9;
const SPLITMIX_MIX_SECOND: u64 = 0x94d0_49bb_1331_11eb;
pub const PPM: u64 = 1_000_000;

pub fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn derive(seed: u64, label: &str) -> Self {
        let mut rng = Self(seed ^ fnv(FNV_OFFSET, label.as_bytes()));
        rng.draw();
        rng
    }

    pub const fn draw(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(SPLITMIX_GAMMA);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(SPLITMIX_MIX_FIRST);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(SPLITMIX_MIX_SECOND);
        mixed ^ (mixed >> 31)
    }

    pub fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            return 0;
        }
        let wide = u128::from(self.draw()) * u128::from(bound);
        u64::try_from(wide >> 64).expect("a product shifted by 64 bits fits in 64 bits")
    }

    pub fn range(&mut self, low: u64, high: u64) -> u64 {
        assert!(low <= high, "a range runs from its low end to its high end");
        low + self.below(high - low + 1)
    }

    pub fn chance(&mut self, ppm: u64) -> bool {
        ppm > 0 && self.below(PPM) < ppm
    }

    pub fn pick(&mut self, len: usize) -> usize {
        assert!(len > 0, "a pick is made among at least one choice");
        let bound = u64::try_from(len).expect("a choice count fits in 64 bits");
        usize::try_from(self.below(bound)).expect("a pick below a usize count fits in usize")
    }
}
