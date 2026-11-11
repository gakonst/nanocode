use super::*;
use alloc::vec::Vec;

fn rng(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

/// Inputs with every kind of structure: empty, runs, random bytes, repeats at small and
/// large distances, 8-byte records like a heap image's.
fn inputs() -> Vec<Vec<u8>> {
    let mut seed = 0x1234_5678_9abc_def1u64;
    let mut v: Vec<Vec<u8>> = vec![
        Vec::new(),
        vec![7],
        vec![0; 1000],
        b"abcabcabcabcabcabcabcabd".to_vec(),
    ];
    let random: Vec<u8> = (0..5000).map(|_| rng(&mut seed) as u8).collect();
    v.push(random.clone());
    let mut rec = Vec::new();
    for i in 0..3000u32 {
        rec.extend_from_slice(&(i * 24).to_le_bytes());
        rec.extend_from_slice(&[0xF8, 0xFF, 0x09, 0x00]);
        if rng(&mut seed).is_multiple_of(5) {
            rec.extend_from_slice(&random[(i as usize * 7) % 4000..][..(i as usize % 40)]);
        }
    }
    v.push(rec);
    let mut far = random[..3000].to_vec();
    far.extend((0..70000).map(|_| (rng(&mut seed) % 3) as u8));
    far.extend_from_slice(&random[..3000]);
    v.push(far);
    v
}

#[test]
fn round_trips() {
    for data in inputs() {
        let packed = encode(&data);
        assert_eq!(decoded_len(&packed), Some(data.len()));
        assert_eq!(decode(&packed, usize::MAX).as_deref(), Ok(&data[..]));
    }
}

#[test]
fn deterministic_and_compressing() {
    let data = &inputs()[5];
    assert_eq!(encode(data), encode(data));
    assert!(
        encode(data).len() < data.len() / 3,
        "{}",
        encode(data).len()
    );
}

#[test]
fn limit_and_header() {
    let packed = encode(b"hello hello hello");
    assert_eq!(decode(&packed, 5), Err(Error::Header));
    assert_eq!(decode(b"LZR0", 100), Err(Error::Header));
    assert_eq!(decode(&packed[..10], 100), Err(Error::Header));
}

/// Corrupted streams (flipped bits, truncation, garbage) give an error or the original
/// bytes (a flip the coder absorbs), never a panic or other output.
#[test]
fn corruption_is_detected() {
    let mut seed = 0x0bad_5eed_u64;
    for data in inputs().into_iter().filter(|d| d.len() > 100) {
        let packed = encode(&data);
        for _ in 0..300 {
            let mut bad = packed.clone();
            match rng(&mut seed) % 3 {
                0 => {
                    let i = HEADER + rng(&mut seed) as usize % (bad.len() - HEADER);
                    bad[i] ^= 1 << (rng(&mut seed) % 8);
                }
                1 => {
                    let n = HEADER + rng(&mut seed) as usize % (bad.len() - HEADER);
                    bad.truncate(n);
                }
                _ => {
                    let i = HEADER + rng(&mut seed) as usize % (bad.len() - HEADER);
                    for b in &mut bad[i..] {
                        *b = rng(&mut seed) as u8;
                    }
                }
            }
            if let Ok(out) = decode(&bad, data.len()) {
                assert_eq!(out, data);
            }
        }
        // Pure garbage after a valid header.
        let mut g = packed[..HEADER].to_vec();
        g.extend((0..2000).map(|_| rng(&mut seed) as u8));
        assert!(decode(&g, data.len()).is_err());
    }
}
