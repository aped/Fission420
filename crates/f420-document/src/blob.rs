//! Encoding helpers for embedded source files: base64 (RFC 4648, standard
//! alphabet, padded) and an FNV-1a 64-bit integrity hash.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> shift) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decode padded standard base64; `None` on any invalid character or length.
/// ASCII whitespace is ignored.
pub(crate) fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let clean: Vec<u8> = text
        .bytes()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    if clean.len() % 4 != 0 {
        return None;
    }
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    let quads = clean.len() / 4;
    for (qi, q) in clean.chunks(4).enumerate() {
        let pad = q.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && qi + 1 != quads) {
            return None;
        }
        let mut n = 0u32;
        for &c in &q[..4 - pad] {
            n = (n << 6) | val(c)?;
        }
        n <<= 6 * pad as u32;
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}

pub(crate) fn fnv64_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_rfc4648_vectors_and_round_trip() {
        for (plain, enc) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(plain.as_bytes()), enc);
            assert_eq!(base64_decode(enc).unwrap(), plain.as_bytes());
        }
        let all: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        assert_eq!(base64_decode(&base64_encode(&all)).unwrap(), all);
        assert!(base64_decode("Zm9").is_none());
        assert!(base64_decode("Zm=v").is_none());
        assert!(base64_decode("Zm9v$===").is_none());
    }

    #[test]
    fn fnv_known_values() {
        // FNV-1a 64 reference values.
        assert_eq!(fnv64_hex(b""), "cbf29ce484222325");
        assert_eq!(fnv64_hex(b"a"), "af63dc4c8601ec8c");
    }
}
