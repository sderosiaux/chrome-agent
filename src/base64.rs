//! Minimal base64 decoder (RFC 4648). Shared by commands that pull binary
//! payloads out of CDP (screenshots, PDFs, downloads). Avoids the `base64`
//! crate to keep the dependency graph pure-Rust (static musl builds, issue #3).

/// Decode a standard base64 string into bytes.
///
/// Ignores padding (`=`) and ASCII whitespace. Errors on any other
/// out-of-alphabet character.
pub fn decode(input: &str) -> Result<Vec<u8>, crate::BoxError> {
    // Compile-time lookup table (Rust 2024 const block)
    const LOOKUP: [u8; 256] = const {
        let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut lut = [255u8; 256];
        let mut i = 0;
        while i < 64 {
            lut[table[i] as usize] = i as u8;
            i += 1;
        }
        lut
    };

    let input = input.as_bytes();
    let mut out = Vec::with_capacity(input.len() * 3 / 4);

    let mut buf: u32 = 0;
    let mut bits: u32 = 0;

    for &b in input {
        if matches!(b, b'=' | b'\n' | b'\r' | b' ' | b'\t') {
            continue;
        }
        let val = LOOKUP[b as usize];
        if val == 255 {
            return Err(format!("Invalid base64 character: {}", b as char).into());
        }
        buf = (buf << 6) | u32::from(val);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn decodes_ascii() {
        assert_eq!(decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode("Zm9vYmFy").unwrap(), b"foobar");
    }

    #[test]
    fn decodes_empty() {
        assert_eq!(decode("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn ignores_whitespace_and_padding() {
        // Chrome sometimes returns line-wrapped base64.
        assert_eq!(decode("aGVs\nbG8=").unwrap(), b"hello");
        assert_eq!(decode("  aGVsbG8=  ").unwrap(), b"hello");
        assert_eq!(decode("aGVs\tbG8=\r\n").unwrap(), b"hello");
    }

    #[test]
    fn roundtrip_all_byte_values() {
        // Encode 0..=255 with a reference encoder, decode, compare.
        let bytes: Vec<u8> = (0..=255).collect();
        let encoded = reference_encode(&bytes);
        assert_eq!(decode(&encoded).unwrap(), bytes);
    }

    #[test]
    fn rejects_invalid_char() {
        assert!(decode("aGVsbG8!").is_err());
    }

    // Reference encoder for the roundtrip test only (not shipped in the binary path).
    fn reference_encode(data: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
            out.push(if chunk.len() > 1 { ALPHABET[((n >> 6) & 63) as usize] as char } else { '=' });
            out.push(if chunk.len() > 2 { ALPHABET[(n & 63) as usize] as char } else { '=' });
        }
        out
    }
}
