//! A small SHA-256, shared with `build.rs` (which hashes the committed schema snapshots).
//! Carriage returns are skipped, so a CRLF checkout hashes like an LF one.
#![allow(dead_code)]

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
    0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
    0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
    0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
    0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
    0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2,
];

const H0: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];

const fn compress(mut h: [u32; 8], block: &[u8; 64]) -> [u32; 8] {
    let mut w = [0u32; 64];
    let mut i = 0;
    while i < 16 {
        w[i] = u32::from_be_bytes([block[4 * i], block[4 * i + 1], block[4 * i + 2], block[4 * i + 3]]);
        i += 1;
    }
    while i < 64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        i += 1;
    }
    let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) = (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
    i = 0;
    while i < 64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
        i += 1;
    }
    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
    h[5] = h[5].wrapping_add(f);
    h[6] = h[6].wrapping_add(g);
    h[7] = h[7].wrapping_add(hh);
    h
}

/// SHA-256 of `data` with every `\r` byte left out.
pub(super) const fn sha256_lf(data: &[u8]) -> [u8; 32] {
    let mut h = H0;
    let mut block = [0u8; 64];
    let mut fill = 0usize;
    let mut len: u64 = 0;
    let mut i = 0;
    while i < data.len() {
        let byte = data[i];
        i += 1;
        if byte == b'\r' {
            continue;
        }
        block[fill] = byte;
        fill += 1;
        len += 1;
        if fill == 64 {
            h = compress(h, &block);
            fill = 0;
        }
    }
    // Padding: 0x80, zeros, then the bit length as u64 big-endian.
    block[fill] = 0x80;
    fill += 1;
    if fill > 56 {
        while fill < 64 {
            block[fill] = 0;
            fill += 1;
        }
        h = compress(h, &block);
        fill = 0;
    }
    while fill < 56 {
        block[fill] = 0;
        fill += 1;
    }
    let bits = (len * 8).to_be_bytes();
    let mut j = 0;
    while j < 8 {
        block[56 + j] = bits[j];
        j += 1;
    }
    h = compress(h, &block);
    let mut out = [0u8; 32];
    let mut k = 0;
    while k < 8 {
        let word = h[k].to_be_bytes();
        out[4 * k] = word[0];
        out[4 * k + 1] = word[1];
        out[4 * k + 2] = word[2];
        out[4 * k + 3] = word[3];
        k += 1;
    }
    out
}

/// The first 8 bytes of a digest as 16 lowercase hex characters.
pub(super) const fn short_hex(digest: &[u8; 32]) -> [u8; 16] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 16];
    let mut i = 0;
    while i < 8 {
        out[2 * i] = HEX[(digest[i] >> 4) as usize];
        out[2 * i + 1] = HEX[(digest[i] & 0x0f) as usize];
        i += 1;
    }
    out
}

/// Full lowercase hex of a digest.
#[cfg(test)]
fn full_hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(
            full_hex(&sha256_lf(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            full_hex(&sha256_lf(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(
            full_hex(&sha256_lf(long)),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        let million_a = vec![b'a'; 1_000_000];
        assert_eq!(
            full_hex(&sha256_lf(&million_a)),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn carriage_returns_are_ignored() {
        assert_eq!(sha256_lf(b"a\r\nb\r\n"), sha256_lf(b"a\nb\n"));
    }
}
