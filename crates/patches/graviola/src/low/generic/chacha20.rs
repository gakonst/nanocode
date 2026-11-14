// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
// Originally from cifra

pub(crate) struct ChaCha20 {
    key0: [u32; 4],
    key1: [u32; 4],
    nonce: [u32; 4],
}

fn four(b: &[u8; 16]) -> [u32; 4] {
    [
        u32::from_le_bytes(b[0..4].try_into().unwrap()),
        u32::from_le_bytes(b[4..8].try_into().unwrap()),
        u32::from_le_bytes(b[8..12].try_into().unwrap()),
        u32::from_le_bytes(b[12..16].try_into().unwrap()),
    ]
}

impl ChaCha20 {
    pub(crate) fn new(key: &[u8; 32], nonce: &[u8; 16]) -> Self {
        Self {
            key0: four(key[0..16].try_into().unwrap()),
            key1: four(key[16..32].try_into().unwrap()),
            nonce: four(nonce),
        }
    }

    pub(crate) fn cipher(&mut self, buffer: &mut [u8]) {
        for block in buffer.chunks_mut(64) {
            let mut stream = [0u8; 64];
            core(&self.key0, &self.key1, &self.nonce, &mut stream);
            for (out, key) in block.iter_mut().zip(stream.iter()) {
                *out ^= *key;
            }

            self.nonce[0] = self.nonce[0].wrapping_add(1);
        }
    }
}

fn core(key0: &[u32; 4], key1: &[u32; 4], nonce: &[u32; 4], out: &mut [u8; 64]) {
    let [mut z0, mut z1, mut z2, mut z3] = SIGMA;
    let &[mut z4, mut z5, mut z6, mut z7] = key0;
    let &[mut z8, mut z9, mut za, mut zb] = key1;
    let &[mut zc, mut zd, mut ze, mut zf] = nonce;

    let (x0, x1, x2, x3, x4, x5, x6, x7, x8, x9, xa, xb, xc, xd, xe, xf) = (
        z0, z1, z2, z3, z4, z5, z6, z7, z8, z9, za, zb, zc, zd, ze, zf,
    );

    macro_rules! quarter {
        ($a:ident, $b:ident, $c:ident, $d:ident) => {
            $a = $a.wrapping_add($b);
            $d = ($d ^ $a).rotate_left(16);
            $c = $c.wrapping_add($d);
            $b = ($b ^ $c).rotate_left(12);
            $a = $a.wrapping_add($b);
            $d = ($d ^ $a).rotate_left(8);
            $c = $c.wrapping_add($d);
            $b = ($b ^ $c).rotate_left(7);
        };
    }

    for _ in 0..10 {
        quarter!(z0, z4, z8, zc);
        quarter!(z1, z5, z9, zd);
        quarter!(z2, z6, za, ze);
        quarter!(z3, z7, zb, zf);
        quarter!(z0, z5, za, zf);
        quarter!(z1, z6, zb, zc);
        quarter!(z2, z7, z8, zd);
        quarter!(z3, z4, z9, ze);
    }

    let x0 = x0.wrapping_add(z0);
    let x1 = x1.wrapping_add(z1);
    let x2 = x2.wrapping_add(z2);
    let x3 = x3.wrapping_add(z3);
    let x4 = x4.wrapping_add(z4);
    let x5 = x5.wrapping_add(z5);
    let x6 = x6.wrapping_add(z6);
    let x7 = x7.wrapping_add(z7);
    let x8 = x8.wrapping_add(z8);
    let x9 = x9.wrapping_add(z9);
    let xa = xa.wrapping_add(za);
    let xb = xb.wrapping_add(zb);
    let xc = xc.wrapping_add(zc);
    let xd = xd.wrapping_add(zd);
    let xe = xe.wrapping_add(ze);
    let xf = xf.wrapping_add(zf);

    out[0..4].copy_from_slice(&x0.to_le_bytes());
    out[4..8].copy_from_slice(&x1.to_le_bytes());
    out[8..12].copy_from_slice(&x2.to_le_bytes());
    out[12..16].copy_from_slice(&x3.to_le_bytes());
    out[16..20].copy_from_slice(&x4.to_le_bytes());
    out[20..24].copy_from_slice(&x5.to_le_bytes());
    out[24..28].copy_from_slice(&x6.to_le_bytes());
    out[28..32].copy_from_slice(&x7.to_le_bytes());
    out[32..36].copy_from_slice(&x8.to_le_bytes());
    out[36..40].copy_from_slice(&x9.to_le_bytes());
    out[40..44].copy_from_slice(&xa.to_le_bytes());
    out[44..48].copy_from_slice(&xb.to_le_bytes());
    out[48..52].copy_from_slice(&xc.to_le_bytes());
    out[52..56].copy_from_slice(&xd.to_le_bytes());
    out[56..60].copy_from_slice(&xe.to_le_bytes());
    out[60..64].copy_from_slice(&xf.to_le_bytes());
}

// b"expand 32-byte k" in little-endian
const SIGMA: [u32; 4] = [0x61707865, 0x3320646e, 0x79622d32, 0x6b206574];

