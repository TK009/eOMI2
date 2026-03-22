// Minimal SHA-256 implementation (FIPS 180-4).
//
// Zero dependencies — works on both host and ESP targets.
// Used for hashing API keys before storage (FR-005).

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const H_INIT: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

fn ch(x: u32, y: u32, z: u32) -> u32 { (x & y) ^ (!x & z) }
fn maj(x: u32, y: u32, z: u32) -> u32 { (x & y) ^ (x & z) ^ (y & z) }
fn sigma0(x: u32) -> u32 { x.rotate_right(2) ^ x.rotate_right(13) ^ x.rotate_right(22) }
fn sigma1(x: u32) -> u32 { x.rotate_right(6) ^ x.rotate_right(11) ^ x.rotate_right(25) }
fn little_sigma0(x: u32) -> u32 { x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3) }
fn little_sigma1(x: u32) -> u32 { x.rotate_right(17) ^ x.rotate_right(19) ^ (x >> 10) }

/// Convert a byte slice to a 64-byte block reference.
///
/// # Panics
///
/// Panics if `slice.len() != 64`. All call-sites maintain this invariant
/// via construction (loop stride or fixed-size buffer arithmetic), so a
/// panic here signals a logic bug introduced by refactoring.
fn as_block(slice: &[u8]) -> &[u8; 64] {
    slice
        .try_into()
        .expect("SHA-256 block must be exactly 64 bytes")
}

fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[4 * i],
            block[4 * i + 1],
            block[4 * i + 2],
            block[4 * i + 3],
        ]);
    }
    for i in 16..64 {
        w[i] = little_sigma1(w[i - 2])
            .wrapping_add(w[i - 7])
            .wrapping_add(little_sigma0(w[i - 15]))
            .wrapping_add(w[i - 16]);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;

    for i in 0..64 {
        let t1 = h
            .wrapping_add(sigma1(e))
            .wrapping_add(ch(e, f, g))
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let t2 = sigma0(a).wrapping_add(maj(a, b, c));
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

/// Compute SHA-256 digest of `data`, returning 32 bytes.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state = H_INIT;
    let bit_len = (data.len() as u64) * 8;

    // Process complete 64-byte blocks.
    let mut offset = 0;
    while offset + 64 <= data.len() {
        let block = as_block(&data[offset..offset + 64]);
        compress(&mut state, block);
        offset += 64;
    }

    // Pad the final block(s).
    let remaining = data.len() - offset;
    let mut buf = [0u8; 128]; // at most 2 blocks for padding
    buf[..remaining].copy_from_slice(&data[offset..]);
    buf[remaining] = 0x80;

    let padded_len = if remaining < 56 { 64 } else { 128 };
    buf[padded_len - 8..padded_len].copy_from_slice(&bit_len.to_be_bytes());

    compress(&mut state, as_block(&buf[..64]));
    if padded_len == 128 {
        compress(&mut state, as_block(&buf[64..128]));
    }

    let mut out = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

// --- BLAKE2b (RFC 7693) ---
//
// Minimal implementation for WSOP verification code derivation.
// Zero dependencies — works on both host and ESP targets.

const BLAKE2B_IV: [u64; 8] = [
    0x6a09e667f3bcc908, 0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
    0x510e527fade682d1, 0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
];

const BLAKE2B_SIGMA: [[usize; 16]; 10] = [
    [ 0, 1, 2, 3, 4, 5, 6, 7, 8, 9,10,11,12,13,14,15],
    [14,10, 4, 8, 9,15,13, 6, 1,12, 0, 2,11, 7, 5, 3],
    [11, 8,12, 0, 5, 2,15,13,10,14, 3, 6, 7, 1, 9, 4],
    [ 7, 9, 3, 1,13,12,11,14, 2, 6, 5,10, 4, 0,15, 8],
    [ 9, 0, 5, 7, 2, 4,10,15,14, 1,11,12, 6, 8, 3,13],
    [ 2,12, 6,10, 0,11, 8, 3, 4,13, 7, 5,15,14, 1, 9],
    [12, 5, 1,15,14,13, 4,10, 0, 7, 6, 3, 9, 2, 8,11],
    [13,11, 7,14,12, 1, 3, 9, 5, 0,15, 4, 8, 6, 2,10],
    [ 6,15,14, 9,11, 3, 0, 8,12, 2,13, 7, 1, 4,10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5,15,11, 9,14, 3,12,13, 0],
];

#[inline]
fn blake2b_g(v: &mut [u64; 16], a: usize, b: usize, c: usize, d: usize, x: u64, y: u64) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
    v[d] = (v[d] ^ v[a]).rotate_right(32);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(24);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(63);
}

fn blake2b_compress(h: &mut [u64; 8], block: &[u8; 128], t: u128, last: bool) {
    let mut m = [0u64; 16];
    for i in 0..16 {
        m[i] = u64::from_le_bytes([
            block[8*i], block[8*i+1], block[8*i+2], block[8*i+3],
            block[8*i+4], block[8*i+5], block[8*i+6], block[8*i+7],
        ]);
    }

    let mut v = [0u64; 16];
    v[..8].copy_from_slice(h);
    v[8..16].copy_from_slice(&BLAKE2B_IV);
    v[12] ^= t as u64;
    v[13] ^= (t >> 64) as u64;
    if last {
        v[14] ^= 0xFFFFFFFFFFFFFFFF;
    }

    for round in 0..12 {
        let s = &BLAKE2B_SIGMA[round % 10];
        blake2b_g(&mut v, 0, 4,  8, 12, m[s[ 0]], m[s[ 1]]);
        blake2b_g(&mut v, 1, 5,  9, 13, m[s[ 2]], m[s[ 3]]);
        blake2b_g(&mut v, 2, 6, 10, 14, m[s[ 4]], m[s[ 5]]);
        blake2b_g(&mut v, 3, 7, 11, 15, m[s[ 6]], m[s[ 7]]);
        blake2b_g(&mut v, 0, 5, 10, 15, m[s[ 8]], m[s[ 9]]);
        blake2b_g(&mut v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
        blake2b_g(&mut v, 2, 7,  8, 13, m[s[12]], m[s[13]]);
        blake2b_g(&mut v, 3, 4,  9, 14, m[s[14]], m[s[15]]);
    }

    for i in 0..8 {
        h[i] ^= v[i] ^ v[i + 8];
    }
}

/// Compute BLAKE2b hash of `data` into `out` (1..=64 bytes, no key).
///
/// Implements RFC 7693 with output length 1-64 bytes.
/// Used by WSOP for verification code derivation.
pub fn blake2b(data: &[u8], out: &mut [u8]) {
    assert!(!out.is_empty() && out.len() <= 64, "BLAKE2b output must be 1-64 bytes");
    let nn = out.len();

    // Initialize state: h[0] = IV[0] XOR (0x01010000 | nn)
    let mut h = BLAKE2B_IV;
    h[0] ^= 0x01010000 ^ (nn as u64);

    // Process full 128-byte blocks
    let mut offset = 0;
    let full_blocks = if data.len() > 128 { (data.len() - 1) / 128 } else { 0 };
    for _ in 0..full_blocks {
        let block: &[u8; 128] = data[offset..offset+128].try_into().unwrap();
        offset += 128;
        blake2b_compress(&mut h, block, offset as u128, false);
    }

    // Final block (padded with zeros)
    let mut last_block = [0u8; 128];
    let remaining = data.len() - offset;
    last_block[..remaining].copy_from_slice(&data[offset..]);
    blake2b_compress(&mut h, &last_block, data.len() as u128, true);

    // Extract output bytes (little-endian)
    let mut hash_bytes = [0u8; 64];
    for i in 0..8 {
        hash_bytes[8*i..8*i+8].copy_from_slice(&h[i].to_le_bytes());
    }
    out.copy_from_slice(&hash_bytes[..nn]);
}

/// Compute a single-byte BLAKE2b hash of `data`.
///
/// Convenience wrapper for WSOP verification code derivation:
/// `code_byte = BLAKE2b(pubkey, output_length=1)`.
pub fn blake2b_1(data: &[u8]) -> u8 {
    let mut out = [0u8; 1];
    blake2b(data, &mut out);
    out[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    // NIST FIPS 180-4 test vectors.

    #[test]
    fn sha256_empty() {
        let digest = sha256(b"");
        assert_eq!(
            digest,
            [
                0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14,
                0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9, 0x24,
                0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c,
                0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52, 0xb8, 0x55,
            ]
        );
    }

    #[test]
    fn sha256_abc() {
        let digest = sha256(b"abc");
        assert_eq!(
            digest,
            [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea,
                0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
                0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c,
                0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
            ]
        );
    }

    #[test]
    fn sha256_two_block() {
        let digest = sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        assert_eq!(
            digest,
            [
                0x24, 0x8d, 0x6a, 0x61, 0xd2, 0x06, 0x38, 0xb8,
                0xe5, 0xc0, 0x26, 0x93, 0x0c, 0x3e, 0x60, 0x39,
                0xa3, 0x3c, 0xe4, 0x59, 0x64, 0xff, 0x21, 0x67,
                0xf6, 0xec, 0xed, 0xd4, 0x19, 0xdb, 0x06, 0xc1,
            ]
        );
    }

    #[test]
    fn sha256_deterministic() {
        let a = sha256(b"api-key-12345");
        let b = sha256(b"api-key-12345");
        assert_eq!(a, b);
    }

    #[test]
    fn sha256_different_inputs_differ() {
        let a = sha256(b"key-alpha");
        let b = sha256(b"key-beta");
        assert_ne!(a, b);
    }

    // --- BLAKE2b tests (RFC 7693 Appendix A) ---

    #[test]
    fn blake2b_empty_32() {
        // BLAKE2b-256("") from reference implementation
        let mut out = [0u8; 32];
        blake2b(b"", &mut out);
        assert_eq!(
            out,
            [
                0x0e, 0x57, 0x51, 0xc0, 0x26, 0xe5, 0x43, 0xb2,
                0xe8, 0xab, 0x2e, 0xb0, 0x60, 0x99, 0xda, 0xa1,
                0xd1, 0xe5, 0xdf, 0x47, 0x77, 0x8f, 0x77, 0x87,
                0xfa, 0xab, 0x45, 0xcd, 0xf1, 0x2f, 0xe3, 0xa8,
            ]
        );
    }

    #[test]
    fn blake2b_abc_64() {
        // BLAKE2b-512("abc") from reference implementation
        let mut out = [0u8; 64];
        blake2b(b"abc", &mut out);
        assert_eq!(
            out,
            [
                0xba, 0x80, 0xa5, 0x3f, 0x98, 0x1c, 0x4d, 0x0d,
                0x6a, 0x27, 0x97, 0xb6, 0x9f, 0x12, 0xf6, 0xe9,
                0x4c, 0x21, 0x2f, 0x14, 0x68, 0x5a, 0xc4, 0xb7,
                0x4b, 0x12, 0xbb, 0x6f, 0xdb, 0xff, 0xa2, 0xd1,
                0x7d, 0x87, 0xc5, 0x39, 0x2a, 0xab, 0x79, 0x2d,
                0xc2, 0x52, 0xd5, 0xde, 0x45, 0x33, 0xcc, 0x95,
                0x18, 0xd3, 0x8a, 0xa8, 0xdb, 0xf1, 0x92, 0x5a,
                0xb9, 0x23, 0x86, 0xed, 0xd4, 0x00, 0x99, 0x23,
            ]
        );
    }

    #[test]
    fn blake2b_1byte_output() {
        // Verify blake2b_1 returns the first byte of BLAKE2b(data, 1)
        let b = blake2b_1(b"test-pubkey");
        let mut out = [0u8; 1];
        blake2b(b"test-pubkey", &mut out);
        assert_eq!(b, out[0]);
    }

    #[test]
    fn blake2b_1_deterministic() {
        let a = blake2b_1(b"pubkey-data");
        let b = blake2b_1(b"pubkey-data");
        assert_eq!(a, b);
    }

    #[test]
    fn blake2b_1_different_inputs_differ() {
        let a = blake2b_1(b"pubkey-alpha");
        let b = blake2b_1(b"pubkey-beta");
        assert_ne!(a, b);
    }
}
