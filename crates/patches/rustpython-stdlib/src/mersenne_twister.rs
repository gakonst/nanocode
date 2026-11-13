pub(crate) const N: usize = 624;
const M: usize = 397;
const MATRIX_A: u32 = 0x9908_b0df;
const UPPER_MASK: u32 = 0x8000_0000;
const LOWER_MASK: u32 = 0x7fff_ffff;
const INITIALIZATION_MULTIPLIER: u32 = 1_812_433_253;
const ARRAY_SEED: u32 = 19_650_218;
const ARRAY_MULTIPLIER_FIRST: u32 = 1_664_525;
const ARRAY_MULTIPLIER_SECOND: u32 = 1_566_083_941;
const DEFAULT_SEED: u32 = 5489;
const TEMPERING_MASK_B: u32 = 0x9d2c_5680;
const TEMPERING_MASK_C: u32 = 0xefc6_0000;
const RES53_HIGH_SHIFT: u32 = 5;
const RES53_LOW_SHIFT: u32 = 6;
const RES53_HIGH_SCALE: f64 = 67_108_864.0;
const RES53_DIVISOR: f64 = 9_007_199_254_740_992.0;
const WORD_BYTES: usize = 4;

pub(crate) struct MT19937 {
    state: [u32; N],
    index: usize,
}

impl Default for MT19937 {
    fn default() -> Self {
        Self {
            state: [0; N],
            index: N + 1,
        }
    }
}

impl core::fmt::Debug for MT19937 {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("MT19937").finish_non_exhaustive()
    }
}

impl MT19937 {
    pub(crate) fn new_with_slice_seed(key: &[u32]) -> Self {
        let mut generator = Self::default();
        generator.seed_slice(key);
        generator
    }

    pub(crate) fn from_os_random() -> Self {
        let mut bytes = [0u8; N * WORD_BYTES];
        nanocode_platform::fill_random(&mut bytes).expect("the operating system supplies random bytes");
        let mut key = [0u32; N];
        for (word, chunk) in key.iter_mut().zip(bytes.chunks_exact(WORD_BYTES)) {
            *word = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        Self::new_with_slice_seed(&key)
    }

    fn seed(&mut self, seed: u32) {
        self.state[0] = seed;
        self.index = 1;
        while self.index < N {
            let previous = self.state[self.index - 1];
            self.state[self.index] = INITIALIZATION_MULTIPLIER
                .wrapping_mul(previous ^ (previous >> 30))
                .wrapping_add(self.index as u32);
            self.index += 1;
        }
    }

    fn seed_slice(&mut self, key: &[u32]) {
        assert!(!key.is_empty());
        self.seed(ARRAY_SEED);
        let mut i = 1;
        let mut j = 0;
        for _ in 0..N.max(key.len()) {
            let previous = self.state[i - 1];
            self.state[i] = (self.state[i]
                ^ (previous ^ (previous >> 30)).wrapping_mul(ARRAY_MULTIPLIER_FIRST))
            .wrapping_add(key[j])
            .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                self.state[0] = self.state[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..N - 1 {
            let previous = self.state[i - 1];
            self.state[i] = (self.state[i]
                ^ (previous ^ (previous >> 30)).wrapping_mul(ARRAY_MULTIPLIER_SECOND))
            .wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                self.state[0] = self.state[N - 1];
                i = 1;
            }
        }
        self.state[0] = UPPER_MASK;
    }

    const fn twist(word: u32) -> u32 {
        (word >> 1) ^ if word & 1 == 1 { MATRIX_A } else { 0 }
    }

    pub(crate) fn next_u32(&mut self) -> u32 {
        if self.index >= N {
            if self.index == N + 1 {
                self.seed(DEFAULT_SEED);
            }
            for k in 0..N - M {
                let word = (self.state[k] & UPPER_MASK) | (self.state[k + 1] & LOWER_MASK);
                self.state[k] = self.state[k + M] ^ Self::twist(word);
            }
            for k in N - M..N - 1 {
                let word = (self.state[k] & UPPER_MASK) | (self.state[k + 1] & LOWER_MASK);
                self.state[k] = self.state[k + M - N] ^ Self::twist(word);
            }
            let word = (self.state[N - 1] & UPPER_MASK) | (self.state[0] & LOWER_MASK);
            self.state[N - 1] = self.state[M - 1] ^ Self::twist(word);
            self.index = 0;
        }
        let mut word = self.state[self.index];
        self.index += 1;
        word ^= word >> 11;
        word ^= (word << 7) & TEMPERING_MASK_B;
        word ^= (word << 15) & TEMPERING_MASK_C;
        word ^= word >> 18;
        word
    }

    pub(crate) const fn get_state(&self) -> &[u32; N] {
        &self.state
    }

    pub(crate) const fn get_index(&self) -> usize {
        self.index
    }

    pub(crate) const fn set_state(&mut self, state: &[u32; N]) {
        self.state = *state;
    }

    pub(crate) fn set_index(&mut self, index: usize) {
        assert!(index <= N);
        self.index = index;
    }
}

pub(crate) fn gen_res53(generator: &mut MT19937) -> f64 {
    let high = generator.next_u32() >> RES53_HIGH_SHIFT;
    let low = generator.next_u32() >> RES53_LOW_SHIFT;
    (f64::from(high) * RES53_HIGH_SCALE + f64::from(low)) * (1.0 / RES53_DIVISOR)
}
