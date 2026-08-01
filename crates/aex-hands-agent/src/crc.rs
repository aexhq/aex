//! CRC-32C (Castagnoli), used by the frame preamble header check.
//!
//! Written out rather than taken as a dependency: it is eleven lines of table
//! generation, it must be byte-identical on the guest's `aarch64-unknown-linux-musl`
//! target and on every host that runs the corpus, and a hardware-accelerated crate
//! would add an `unsafe` closure to a crate that forbids `unsafe_code`.

/// The reflected Castagnoli polynomial.
const POLYNOMIAL: u32 = 0x82f6_3b78;

/// The lookup table, built once at compile time.
const TABLE: [u32; 256] = build_table();

/// Builds the byte-wise lookup table.
const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0usize;
    while index < 256 {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "index is bounded by 256 and the cast is the operation"
        )]
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                (value >> 1) ^ POLYNOMIAL
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

/// The CRC-32C of `bytes`.
#[must_use]
pub fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        let slot = usize::from(u8::try_from(crc & 0xff).unwrap_or(0) ^ byte);
        crc = (crc >> 8) ^ TABLE[slot];
    }
    crc ^ u32::MAX
}

#[cfg(test)]
mod tests {
    use super::crc32c;

    #[test]
    fn the_published_castagnoli_vectors_hold() {
        // RFC 3720 appendix B.4 and the standard "check" vector.
        assert_eq!(crc32c(b""), 0x0000_0000);
        assert_eq!(crc32c(b"123456789"), 0xe306_9283);
        assert_eq!(crc32c(&[0u8; 32]), 0x8a91_36aa);
        assert_eq!(crc32c(&[0xffu8; 32]), 0x62a8_ab43);
    }

    #[test]
    fn a_single_flipped_bit_changes_the_check() {
        let mut bytes = *b"AEXH\x00\x01\x01\x00";
        let before = crc32c(&bytes);
        bytes[6] ^= 0x01;
        assert_ne!(crc32c(&bytes), before);
    }
}
