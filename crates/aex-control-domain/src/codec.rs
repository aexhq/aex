//! Canonical unpadded base64url.
//!
//! One encoder, used by the cursor MAC, the credential codec and the assertion
//! envelope alike. Canonical means exactly one spelling per byte string: no
//! padding, no impossible length, and a final character whose unused low bits
//! are zero. A lenient decoder would let one credential have several textual
//! forms, which is how a key-addressed lookup turns into a lookup miss.

/// The unpadded base64url alphabet.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// The characters a final 4-bit remainder can encode, for a 32-byte secret.
///
/// 32 bytes render as 43 characters, the last of which carries only four
/// significant bits. Constraining it is what rejects alternate spellings of the
/// same 256-bit secret.
pub const FINAL_QUARTET_ALPHABET: &str = "AEIMQUYcgkosw048";

/// Encodes unpadded base64url.
#[must_use]
pub fn base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let block = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((block >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((block >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((block >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(block & 0x3f) as usize] as char);
        }
    }
    out
}

/// Decodes canonical unpadded base64url.
///
/// Rejects padding, an impossible length, a character outside the alphabet, and
/// a final character whose unused low bits are non-zero.
#[must_use]
pub fn unbase64url(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let mut block = 0_u32;
        for (index, byte) in chunk.iter().enumerate() {
            let value = u32::try_from(ALPHABET.iter().position(|it| it == byte)?).ok()?;
            block |= value << (18 - 6 * index);
        }
        match chunk.len() {
            4 => {
                out.push(((block >> 16) & 0xff) as u8);
                out.push(((block >> 8) & 0xff) as u8);
                out.push((block & 0xff) as u8);
            }
            3 => {
                if block & 0xff != 0 {
                    return None;
                }
                out.push(((block >> 16) & 0xff) as u8);
                out.push(((block >> 8) & 0xff) as u8);
            }
            2 => {
                if block & 0xffff != 0 {
                    return None;
                }
                out.push(((block >> 16) & 0xff) as u8);
            }
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{FINAL_QUARTET_ALPHABET, base64url, unbase64url};

    #[test]
    fn the_round_trip_is_exact_at_every_remainder() {
        for length in 0_u8..=64 {
            let bytes: Vec<u8> = (0..length)
                .map(|index| index.wrapping_mul(7).wrapping_add(3))
                .collect();
            let text = base64url(&bytes);
            assert!(!text.contains('='), "{text} carries padding");
            assert_eq!(unbase64url(&text), Some(bytes), "length {length}");
        }
    }

    #[test]
    fn a_non_canonical_spelling_is_refused() {
        assert_eq!(
            unbase64url("A"),
            None,
            "a one-character block is impossible"
        );
        assert_eq!(unbase64url("AB=="), None, "padding is not canonical");
        assert_eq!(
            unbase64url("A+/B"),
            None,
            "the standard alphabet is not base64url"
        );
        assert_eq!(unbase64url("AA"), Some(vec![0]));
        assert_eq!(unbase64url("AB"), None, "unused low bits must be zero");
        assert_eq!(unbase64url("AAB"), None, "unused low bits must be zero");
    }

    #[test]
    fn the_final_quartet_alphabet_is_exactly_the_reachable_characters() {
        let mut reachable: Vec<char> = Vec::new();
        for value in 0..16_u8 {
            let bytes = {
                let mut buffer = [0_u8; 32];
                // 32 bytes render as 43 characters; the final character encodes
                // only the low nibble of the last byte.
                buffer[31] = value;
                buffer
            };
            let text = base64url(&bytes);
            assert_eq!(text.len(), 43);
            reachable.push(text.chars().next_back().expect("43 characters"));
        }
        reachable.sort_unstable();
        reachable.dedup();
        let mut declared: Vec<char> = FINAL_QUARTET_ALPHABET.chars().collect();
        declared.sort_unstable();
        assert_eq!(reachable, declared);
    }
}
