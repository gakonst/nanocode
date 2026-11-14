use core::array;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Probe {
    pub offset: usize,
    pub byte: u8,
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
const VECTOR_MIN: usize = 16;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const VECTOR_MIN: usize = swar::LANES;

#[inline]
pub fn find<const N: usize>(bytes: [u8; N], haystack: &[u8]) -> Option<usize> {
    const {
        assert!(
            N >= 1 && N <= 3,
            "a byte search looks for one to three bytes"
        )
    };
    if haystack.len() < VECTOR_MIN {
        return swar::forward_short(bytes, haystack);
    }
    #[cfg(target_arch = "x86_64")]
    if haystack.len() < x86::AVX_LANES {
        return unsafe { x86::forward_sse2(bytes, haystack) };
    }
    vector_forward(bytes, haystack)
}

#[inline]
pub fn rfind<const N: usize>(bytes: [u8; N], haystack: &[u8]) -> Option<usize> {
    const {
        assert!(
            N >= 1 && N <= 3,
            "a byte search looks for one to three bytes"
        )
    };
    if haystack.len() < VECTOR_MIN {
        return swar::reverse_short(bytes, haystack);
    }
    #[cfg(target_arch = "x86_64")]
    if haystack.len() < x86::AVX_LANES {
        return unsafe { x86::reverse_sse2(bytes, haystack) };
    }
    vector_reverse(bytes, haystack)
}

#[inline(never)]
fn vector_forward<const N: usize>(bytes: [u8; N], haystack: &[u8]) -> Option<usize> {
    #[cfg(target_arch = "x86_64")]
    {
        if haystack.len() >= x86::AVX_LANES && x86::avx2() {
            return unsafe { x86::forward_avx2(bytes, haystack) };
        }
        unsafe { x86::forward_sse2(bytes, haystack) }
    }
    #[cfg(target_arch = "aarch64")]
    {
        unsafe { arm::forward_neon(bytes, haystack) }
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        unsafe { forward::<u64, N>(bytes, haystack) }
    }
}

#[inline(never)]
fn vector_reverse<const N: usize>(bytes: [u8; N], haystack: &[u8]) -> Option<usize> {
    #[cfg(target_arch = "x86_64")]
    {
        if haystack.len() >= x86::AVX_LANES && x86::avx2() {
            return unsafe { x86::reverse_avx2(bytes, haystack) };
        }
        unsafe { x86::reverse_sse2(bytes, haystack) }
    }
    #[cfg(target_arch = "aarch64")]
    {
        unsafe { arm::reverse_neon(bytes, haystack) }
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        unsafe { reverse::<u64, N>(bytes, haystack) }
    }
}

#[inline]
pub fn count(byte: u8, haystack: &[u8]) -> usize {
    if haystack.len() < VECTOR_MIN {
        return haystack
            .iter()
            .filter(|&&candidate| candidate == byte)
            .count();
    }
    vector_count(byte, haystack)
}

#[inline(never)]
fn vector_count(byte: u8, haystack: &[u8]) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        if haystack.len() >= x86::AVX_LANES && x86::avx2_popcnt() {
            return unsafe { x86::count_avx2(byte, haystack) };
        }
        unsafe { x86::count_sse2(byte, haystack) }
    }
    #[cfg(target_arch = "aarch64")]
    {
        unsafe { arm::count_neon(byte, haystack) }
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        unsafe { tally::<u64>(byte, haystack) }
    }
}

pub fn find_pair(
    haystack: &[u8],
    start: usize,
    first: Probe,
    second: Probe,
    mut stop: impl FnMut(usize) -> bool,
) -> Option<usize> {
    let reach = first.offset.max(second.offset);
    if reach >= haystack.len() || start >= haystack.len() - reach {
        return None;
    }
    let kernel = PairKernel::detect();
    let lanes = kernel.lanes;
    let end = haystack.len() - reach;
    let mut forward_from = |at: usize, mut mask: u64| {
        while mask != 0 {
            let lane = mask.trailing_zeros() / kernel.lane_bits;
            if stop(at + lane as usize) {
                return Some(at + lane as usize);
            }
            mask &= !(kernel.lane_ones() << (lane * kernel.lane_bits));
        }
        None
    };
    let mut at = start;
    while at + lanes <= end {
        let (chunk, mask) = unsafe { (kernel.forward)(haystack, at, end, first, second) };
        if mask == 0 {
            at = chunk;
            break;
        }
        if let Some(position) = forward_from(chunk, mask) {
            return Some(position);
        }
        at = chunk + lanes;
    }
    if at >= end {
        return None;
    }
    if end >= lanes && end - lanes >= start {
        let last = end - lanes;
        let seen = (at - last) as u32 * kernel.lane_bits;
        let (_, mask) = unsafe { (kernel.forward)(haystack, last, end, first, second) };
        return forward_from(last, mask & (u64::MAX << seen));
    }
    (at..end).find(|&position| probes_match(haystack, position, first, second) && stop(position))
}

pub fn rfind_pair(
    haystack: &[u8],
    end: usize,
    first: Probe,
    second: Probe,
    mut stop: impl FnMut(usize) -> bool,
) -> Option<usize> {
    assert!(
        end <= haystack.len(),
        "a reverse pair search ends inside the haystack"
    );
    let reach = first.offset.max(second.offset);
    if end <= reach {
        return None;
    }
    let kernel = PairKernel::detect();
    let lanes = kernel.lanes;
    let limit = end - reach;
    let mut backward_from = |at: usize, mut mask: u64| {
        while mask != 0 {
            let lane = (u64::BITS - 1 - mask.leading_zeros()) / kernel.lane_bits;
            if stop(at + lane as usize) {
                return Some(at + lane as usize);
            }
            mask &= !(kernel.lane_ones() << (lane * kernel.lane_bits));
        }
        None
    };
    let mut at = limit;
    while at >= lanes {
        let (chunk, mask) = unsafe { (kernel.backward)(haystack, at, first, second) };
        if mask == 0 {
            at = chunk;
            break;
        }
        if let Some(position) = backward_from(chunk, mask) {
            return Some(position);
        }
        at = chunk;
    }
    if at == 0 {
        return None;
    }
    if limit >= lanes {
        let unseen = (1 << (at as u32 * kernel.lane_bits)) - 1;
        let (_, mask) = unsafe { (kernel.backward)(haystack, lanes, first, second) };
        return backward_from(0, mask & unseen);
    }
    (0..at)
        .rev()
        .find(|&position| probes_match(haystack, position, first, second) && stop(position))
}

type Scan = unsafe fn(&[u8], usize, usize, Probe, Probe) -> (usize, u64);
type ScanBack = unsafe fn(&[u8], usize, Probe, Probe) -> (usize, u64);

#[derive(Clone, Copy)]
struct PairKernel {
    lanes: usize,
    lane_bits: u32,
    forward: Scan,
    backward: ScanBack,
}

impl PairKernel {
    #[inline(always)]
    fn detect() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            if x86::avx2() {
                return Self::of::<core::arch::x86_64::__m256i>(
                    x86::scan_avx2,
                    x86::scan_back_avx2,
                );
            }
            Self::of::<core::arch::x86_64::__m128i>(x86::scan_sse2, x86::scan_back_sse2)
        }
        #[cfg(target_arch = "aarch64")]
        {
            Self::of::<core::arch::aarch64::uint8x16_t>(arm::scan_neon, arm::scan_back_neon)
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            Self::of::<u64>(scan::<u64>, scan_back::<u64>)
        }
    }

    #[inline(always)]
    const fn of<V: Vector>(forward: Scan, backward: ScanBack) -> Self {
        Self {
            lanes: V::LANES,
            lane_bits: V::LANE_BITS,
            forward,
            backward,
        }
    }

    #[inline(always)]
    const fn lane_ones(&self) -> u64 {
        u64::MAX >> (u64::BITS - self.lane_bits)
    }
}

trait Vector: Copy {
    const LANES: usize;
    const LANE_BITS: u32;
    const SET_BITS: u32;
    unsafe fn splat(byte: u8) -> Self;
    unsafe fn load(haystack: &[u8], at: usize) -> Self;
    unsafe fn equal(self, other: Self) -> Self;
    unsafe fn or(self, other: Self) -> Self;
    unsafe fn and(self, other: Self) -> Self;
    unsafe fn mask(self) -> u64;
}

const fn unrolled(needles: usize) -> usize {
    if needles == 1 { 4 } else { 2 }
}

#[inline(always)]
const fn first_lane<V: Vector>(mask: u64) -> usize {
    (mask.trailing_zeros() / V::LANE_BITS) as usize
}

#[inline(always)]
const fn last_lane<V: Vector>(mask: u64) -> usize {
    ((u64::BITS - 1 - mask.leading_zeros()) / V::LANE_BITS) as usize
}

#[inline(always)]
unsafe fn hits<V: Vector, const N: usize>(needles: &[V; N], haystack: &[u8], at: usize) -> V {
    debug_assert!(at + V::LANES <= haystack.len());
    let chunk = unsafe { V::load(haystack, at) };
    let mut hit = unsafe { chunk.equal(needles[0]) };
    for needle in &needles[1..] {
        hit = unsafe { hit.or(chunk.equal(*needle)) };
    }
    hit
}

#[inline(always)]
fn first_in_group<V: Vector, const G: usize>(group: &[V; G]) -> usize {
    for (index, hit) in group.iter().enumerate() {
        let mask = unsafe { hit.mask() };
        if mask != 0 {
            return index * V::LANES + first_lane::<V>(mask);
        }
    }
    unreachable!("a group whose union has a hit has a vector with a hit")
}

#[inline(always)]
fn last_in_group<V: Vector, const G: usize>(group: &[V; G]) -> usize {
    for (index, hit) in group.iter().enumerate().rev() {
        let mask = unsafe { hit.mask() };
        if mask != 0 {
            return index * V::LANES + last_lane::<V>(mask);
        }
    }
    unreachable!("a group whose union has a hit has a vector with a hit")
}

#[inline(always)]
unsafe fn union<V: Vector, const G: usize>(group: &[V; G]) -> u64 {
    let mut all = group[0];
    for hit in &group[1..] {
        all = unsafe { all.or(*hit) };
    }
    unsafe { all.mask() }
}

#[inline(always)]
unsafe fn forward<V: Vector, const N: usize>(bytes: [u8; N], haystack: &[u8]) -> Option<usize> {
    let lanes = V::LANES;
    let len = haystack.len();
    assert!(len >= lanes, "a vector search reads at least one vector");
    let needles: [V; N] = array::from_fn(|index| unsafe { V::splat(bytes[index]) });
    let mask = unsafe { hits(&needles, haystack, 0).mask() };
    if mask != 0 {
        return Some(first_lane::<V>(mask));
    }
    let mut at = lanes - (haystack.as_ptr() as usize & (lanes - 1));
    if N == 1 {
        if let Some(found) =
            unsafe { forward_groups::<V, N, { unrolled(1) }>(&needles, haystack, &mut at) }
        {
            return Some(found);
        }
    } else if let Some(found) =
        unsafe { forward_groups::<V, N, { unrolled(2) }>(&needles, haystack, &mut at) }
    {
        return Some(found);
    }
    while at + lanes <= len {
        let mask = unsafe { hits(&needles, haystack, at).mask() };
        if mask != 0 {
            return Some(at + first_lane::<V>(mask));
        }
        at += lanes;
    }
    if at < len {
        let start = len - lanes;
        let mask = unsafe { hits(&needles, haystack, start).mask() };
        if mask != 0 {
            return Some(start + first_lane::<V>(mask));
        }
    }
    None
}

#[inline(always)]
unsafe fn forward_groups<V: Vector, const N: usize, const G: usize>(
    needles: &[V; N],
    haystack: &[u8],
    at: &mut usize,
) -> Option<usize> {
    let step = V::LANES * G;
    while *at + step <= haystack.len() {
        let group: [V; G] =
            array::from_fn(|index| unsafe { hits(needles, haystack, *at + index * V::LANES) });
        if unsafe { union(&group) } != 0 {
            return Some(*at + first_in_group(&group));
        }
        *at += step;
    }
    None
}

#[inline(always)]
unsafe fn reverse<V: Vector, const N: usize>(bytes: [u8; N], haystack: &[u8]) -> Option<usize> {
    let lanes = V::LANES;
    let len = haystack.len();
    assert!(len >= lanes, "a vector search reads at least one vector");
    let needles: [V; N] = array::from_fn(|index| unsafe { V::splat(bytes[index]) });
    let start = len - lanes;
    let mask = unsafe { hits(&needles, haystack, start).mask() };
    if mask != 0 {
        return Some(start + last_lane::<V>(mask));
    }
    let mut at = len - ((haystack.as_ptr() as usize + len) & (lanes - 1));
    if N == 1 {
        if let Some(found) =
            unsafe { reverse_groups::<V, N, { unrolled(1) }>(&needles, haystack, &mut at) }
        {
            return Some(found);
        }
    } else if let Some(found) =
        unsafe { reverse_groups::<V, N, { unrolled(2) }>(&needles, haystack, &mut at) }
    {
        return Some(found);
    }
    while at >= lanes {
        at -= lanes;
        let mask = unsafe { hits(&needles, haystack, at).mask() };
        if mask != 0 {
            return Some(at + last_lane::<V>(mask));
        }
    }
    if at > 0 {
        let mask = unsafe { hits(&needles, haystack, 0).mask() };
        if mask != 0 {
            return Some(last_lane::<V>(mask));
        }
    }
    None
}

#[inline(always)]
unsafe fn reverse_groups<V: Vector, const N: usize, const G: usize>(
    needles: &[V; N],
    haystack: &[u8],
    at: &mut usize,
) -> Option<usize> {
    let step = V::LANES * G;
    while *at >= step {
        *at -= step;
        let group: [V; G] =
            array::from_fn(|index| unsafe { hits(needles, haystack, *at + index * V::LANES) });
        if unsafe { union(&group) } != 0 {
            return Some(*at + last_in_group(&group));
        }
    }
    None
}

#[inline(always)]
unsafe fn tally<V: Vector>(byte: u8, haystack: &[u8]) -> usize {
    let lanes = V::LANES;
    let needle = [unsafe { V::splat(byte) }];
    let lanes_set = |hit: V| (unsafe { hit.mask() }.count_ones() / V::SET_BITS) as usize;
    let mut total = 0;
    let mut at = 0;
    let step = lanes * unrolled(1);
    while at + step <= haystack.len() {
        for index in 0..unrolled(1) {
            total += lanes_set(unsafe { hits(&needle, haystack, at + index * lanes) });
        }
        at += step;
    }
    while at + lanes <= haystack.len() {
        total += lanes_set(unsafe { hits(&needle, haystack, at) });
        at += lanes;
    }
    total
        + haystack[at..]
            .iter()
            .filter(|&&candidate| candidate == byte)
            .count()
}

#[inline(always)]
unsafe fn candidates<V: Vector>(haystack: &[u8], at: usize, first: Probe, second: Probe) -> u64 {
    unsafe {
        V::load(haystack, at + first.offset)
            .equal(V::splat(first.byte))
            .and(V::load(haystack, at + second.offset).equal(V::splat(second.byte)))
            .mask()
    }
}

const fn probes_match(haystack: &[u8], position: usize, first: Probe, second: Probe) -> bool {
    haystack[position + first.offset] == first.byte
        && haystack[position + second.offset] == second.byte
}

#[inline(always)]
unsafe fn scan<V: Vector>(
    haystack: &[u8],
    mut at: usize,
    end: usize,
    first: Probe,
    second: Probe,
) -> (usize, u64) {
    while at + V::LANES <= end {
        let mask = unsafe { candidates::<V>(haystack, at, first, second) };
        if mask != 0 {
            return (at, mask);
        }
        at += V::LANES;
    }
    (at, 0)
}

#[inline(always)]
unsafe fn scan_back<V: Vector>(
    haystack: &[u8],
    mut at: usize,
    first: Probe,
    second: Probe,
) -> (usize, u64) {
    while at >= V::LANES {
        at -= V::LANES;
        let mask = unsafe { candidates::<V>(haystack, at, first, second) };
        if mask != 0 {
            return (at, mask);
        }
    }
    (at, 0)
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    use core::arch::x86_64::{
        __m128i, __m256i, _mm_and_si128, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8,
        _mm_or_si128, _mm_set1_epi8, _mm256_and_si256, _mm256_cmpeq_epi8, _mm256_loadu_si256,
        _mm256_movemask_epi8, _mm256_or_si256, _mm256_set1_epi8,
    };

    use super::{Probe, Vector, forward, reverse, scan, scan_back, tally};

    pub(super) const SSE_LANES: usize = 16;
    pub(super) const AVX_LANES: usize = 32;

    #[inline(always)]
    pub(super) fn avx2() -> bool {
        std::is_x86_feature_detected!("avx2")
    }

    #[inline(always)]
    pub(super) fn avx2_popcnt() -> bool {
        std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("popcnt")
    }

    impl Vector for __m128i {
        const LANES: usize = SSE_LANES;
        const LANE_BITS: u32 = 1;
        const SET_BITS: u32 = 1;
        #[inline(always)]
        unsafe fn splat(byte: u8) -> Self {
            unsafe { _mm_set1_epi8(byte as i8) }
        }
        #[inline(always)]
        unsafe fn load(haystack: &[u8], at: usize) -> Self {
            unsafe { _mm_loadu_si128(haystack.as_ptr().add(at).cast()) }
        }
        #[inline(always)]
        unsafe fn equal(self, other: Self) -> Self {
            unsafe { _mm_cmpeq_epi8(self, other) }
        }
        #[inline(always)]
        unsafe fn or(self, other: Self) -> Self {
            unsafe { _mm_or_si128(self, other) }
        }
        #[inline(always)]
        unsafe fn and(self, other: Self) -> Self {
            unsafe { _mm_and_si128(self, other) }
        }
        #[inline(always)]
        unsafe fn mask(self) -> u64 {
            u64::from(unsafe { _mm_movemask_epi8(self) } as u32)
        }
    }

    impl Vector for __m256i {
        const LANES: usize = AVX_LANES;
        const LANE_BITS: u32 = 1;
        const SET_BITS: u32 = 1;
        #[inline(always)]
        unsafe fn splat(byte: u8) -> Self {
            unsafe { _mm256_set1_epi8(byte as i8) }
        }
        #[inline(always)]
        unsafe fn load(haystack: &[u8], at: usize) -> Self {
            unsafe { _mm256_loadu_si256(haystack.as_ptr().add(at).cast()) }
        }
        #[inline(always)]
        unsafe fn equal(self, other: Self) -> Self {
            unsafe { _mm256_cmpeq_epi8(self, other) }
        }
        #[inline(always)]
        unsafe fn or(self, other: Self) -> Self {
            unsafe { _mm256_or_si256(self, other) }
        }
        #[inline(always)]
        unsafe fn and(self, other: Self) -> Self {
            unsafe { _mm256_and_si256(self, other) }
        }
        #[inline(always)]
        unsafe fn mask(self) -> u64 {
            u64::from(unsafe { _mm256_movemask_epi8(self) } as u32)
        }
    }

    #[target_feature(enable = "sse2")]
    #[inline(never)]
    pub(super) unsafe fn forward_sse2<const N: usize>(
        bytes: [u8; N],
        haystack: &[u8],
    ) -> Option<usize> {
        unsafe { forward::<__m128i, N>(bytes, haystack) }
    }

    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn forward_avx2<const N: usize>(
        bytes: [u8; N],
        haystack: &[u8],
    ) -> Option<usize> {
        unsafe { forward::<__m256i, N>(bytes, haystack) }
    }

    #[target_feature(enable = "sse2")]
    #[inline(never)]
    pub(super) unsafe fn reverse_sse2<const N: usize>(
        bytes: [u8; N],
        haystack: &[u8],
    ) -> Option<usize> {
        unsafe { reverse::<__m128i, N>(bytes, haystack) }
    }

    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn reverse_avx2<const N: usize>(
        bytes: [u8; N],
        haystack: &[u8],
    ) -> Option<usize> {
        unsafe { reverse::<__m256i, N>(bytes, haystack) }
    }

    #[target_feature(enable = "sse2")]
    pub(super) unsafe fn count_sse2(byte: u8, haystack: &[u8]) -> usize {
        unsafe { tally::<__m128i>(byte, haystack) }
    }

    #[target_feature(enable = "avx2,popcnt")]
    pub(super) unsafe fn count_avx2(byte: u8, haystack: &[u8]) -> usize {
        unsafe { tally::<__m256i>(byte, haystack) }
    }

    #[target_feature(enable = "sse2")]
    #[inline(never)]
    pub(super) unsafe fn scan_sse2(
        haystack: &[u8],
        at: usize,
        end: usize,
        first: Probe,
        second: Probe,
    ) -> (usize, u64) {
        unsafe { scan::<__m128i>(haystack, at, end, first, second) }
    }

    #[target_feature(enable = "sse2")]
    #[inline(never)]
    pub(super) unsafe fn scan_back_sse2(
        haystack: &[u8],
        at: usize,
        first: Probe,
        second: Probe,
    ) -> (usize, u64) {
        unsafe { scan_back::<__m128i>(haystack, at, first, second) }
    }

    #[target_feature(enable = "avx2")]
    #[inline(never)]
    pub(super) unsafe fn scan_avx2(
        haystack: &[u8],
        at: usize,
        end: usize,
        first: Probe,
        second: Probe,
    ) -> (usize, u64) {
        unsafe { scan::<__m256i>(haystack, at, end, first, second) }
    }

    #[target_feature(enable = "avx2")]
    #[inline(never)]
    pub(super) unsafe fn scan_back_avx2(
        haystack: &[u8],
        at: usize,
        first: Probe,
        second: Probe,
    ) -> (usize, u64) {
        unsafe { scan_back::<__m256i>(haystack, at, first, second) }
    }
}

#[cfg(target_arch = "aarch64")]
mod arm {
    use core::arch::aarch64::{
        uint8x16_t, vandq_u8, vceqq_u8, vdupq_n_u8, vget_lane_u64, vld1q_u8, vorrq_u8,
        vreinterpret_u64_u8, vreinterpretq_u16_u8, vshrn_n_u16,
    };

    use super::{Probe, Vector, forward, reverse, scan, scan_back, tally};

    const LANES: usize = 16;
    const NARROW_SHIFT: i32 = 4;

    impl Vector for uint8x16_t {
        const LANES: usize = LANES;
        const LANE_BITS: u32 = NARROW_SHIFT as u32;
        const SET_BITS: u32 = NARROW_SHIFT as u32;
        #[inline(always)]
        unsafe fn splat(byte: u8) -> Self {
            unsafe { vdupq_n_u8(byte) }
        }
        #[inline(always)]
        unsafe fn load(haystack: &[u8], at: usize) -> Self {
            unsafe { vld1q_u8(haystack.as_ptr().add(at)) }
        }
        #[inline(always)]
        unsafe fn equal(self, other: Self) -> Self {
            unsafe { vceqq_u8(self, other) }
        }
        #[inline(always)]
        unsafe fn or(self, other: Self) -> Self {
            unsafe { vorrq_u8(self, other) }
        }
        #[inline(always)]
        unsafe fn and(self, other: Self) -> Self {
            unsafe { vandq_u8(self, other) }
        }
        #[inline(always)]
        unsafe fn mask(self) -> u64 {
            unsafe {
                vget_lane_u64::<0>(vreinterpret_u64_u8(vshrn_n_u16::<NARROW_SHIFT>(
                    vreinterpretq_u16_u8(self),
                )))
            }
        }
    }

    #[target_feature(enable = "neon")]
    pub(super) unsafe fn forward_neon<const N: usize>(
        bytes: [u8; N],
        haystack: &[u8],
    ) -> Option<usize> {
        unsafe { forward::<uint8x16_t, N>(bytes, haystack) }
    }

    #[target_feature(enable = "neon")]
    pub(super) unsafe fn reverse_neon<const N: usize>(
        bytes: [u8; N],
        haystack: &[u8],
    ) -> Option<usize> {
        unsafe { reverse::<uint8x16_t, N>(bytes, haystack) }
    }

    #[target_feature(enable = "neon")]
    pub(super) unsafe fn count_neon(byte: u8, haystack: &[u8]) -> usize {
        unsafe { tally::<uint8x16_t>(byte, haystack) }
    }

    #[target_feature(enable = "neon")]
    #[inline(never)]
    pub(super) unsafe fn scan_neon(
        haystack: &[u8],
        at: usize,
        end: usize,
        first: Probe,
        second: Probe,
    ) -> (usize, u64) {
        unsafe { scan::<uint8x16_t>(haystack, at, end, first, second) }
    }

    #[target_feature(enable = "neon")]
    #[inline(never)]
    pub(super) unsafe fn scan_back_neon(
        haystack: &[u8],
        at: usize,
        first: Probe,
        second: Probe,
    ) -> (usize, u64) {
        unsafe { scan_back::<uint8x16_t>(haystack, at, first, second) }
    }
}

mod swar {
    pub(super) const LANES: usize = size_of::<u64>();
    const ONES: u64 = u64::MAX / 0xff;
    const LOW_SEVEN: u64 = ONES * 0x7f;

    #[inline(always)]
    const fn zero_bytes(word: u64) -> u64 {
        !(((word & LOW_SEVEN) + LOW_SEVEN) | word | LOW_SEVEN)
    }

    #[inline(always)]
    fn word(haystack: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(
            haystack[at..at + LANES]
                .try_into()
                .expect("a word holds eight bytes"),
        )
    }

    #[inline(always)]
    fn word_hits<const N: usize>(bytes: [u8; N], word: u64) -> u64 {
        bytes.iter().fold(0, |hits, &byte| {
            hits | zero_bytes(word ^ (ONES * u64::from(byte)))
        })
    }

    #[inline(always)]
    pub(super) fn forward_short<const N: usize>(bytes: [u8; N], haystack: &[u8]) -> Option<usize> {
        let len = haystack.len();
        if len < LANES {
            return haystack.iter().position(|byte| bytes.contains(byte));
        }
        let hits = word_hits(bytes, word(haystack, 0));
        if hits != 0 {
            return Some((hits.trailing_zeros() / u8::BITS) as usize);
        }
        let start = len - LANES;
        let hits = word_hits(bytes, word(haystack, start));
        (hits != 0).then(|| start + (hits.trailing_zeros() / u8::BITS) as usize)
    }

    #[inline(always)]
    pub(super) fn reverse_short<const N: usize>(bytes: [u8; N], haystack: &[u8]) -> Option<usize> {
        let len = haystack.len();
        if len < LANES {
            return haystack.iter().rposition(|byte| bytes.contains(byte));
        }
        let last = |hits: u64| ((u64::BITS - 1 - hits.leading_zeros()) / u8::BITS) as usize;
        let start = len - LANES;
        let hits = word_hits(bytes, word(haystack, start));
        if hits != 0 {
            return Some(start + last(hits));
        }
        let hits = word_hits(bytes, word(haystack, 0));
        (hits != 0).then(|| last(hits))
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    impl super::Vector for u64 {
        const LANES: usize = LANES;
        const LANE_BITS: u32 = u8::BITS;
        const SET_BITS: u32 = 1;
        #[inline(always)]
        unsafe fn splat(byte: u8) -> Self {
            ONES * Self::from(byte)
        }
        #[inline(always)]
        unsafe fn load(haystack: &[u8], at: usize) -> Self {
            Self::from_le_bytes(unsafe {
                haystack
                    .as_ptr()
                    .add(at)
                    .cast::<[u8; LANES]>()
                    .read_unaligned()
            })
        }
        #[inline(always)]
        unsafe fn equal(self, other: Self) -> Self {
            zero_bytes(self ^ other)
        }
        #[inline(always)]
        unsafe fn or(self, other: Self) -> Self {
            self | other
        }
        #[inline(always)]
        unsafe fn and(self, other: Self) -> Self {
            self & other
        }
        #[inline(always)]
        unsafe fn mask(self) -> u64 {
            self
        }
    }
}
