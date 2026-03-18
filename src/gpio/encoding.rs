//! Data encoding for peripheral protocol TX/RX data (FR-009a).
//!
//! Supports hex (hand-rolled), base64 (minimal impl), and UTF-8 string
//! passthrough. No external dependencies.

use core::fmt;
use core::str::FromStr;

/// Data encoding for peripheral protocol TX/RX data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataEncoding {
    /// UTF-8 string (default).
    String,
    /// Hex-encoded binary data.
    Hex,
    /// Base64-encoded binary data.
    Base64,
}

impl DataEncoding {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Hex => "hex",
            Self::Base64 => "base64",
        }
    }

    /// Decode an encoded string into raw bytes.
    pub fn decode(&self, s: &str) -> Result<Vec<u8>, EncodingError> {
        match self {
            Self::String => Ok(s.as_bytes().to_vec()),
            Self::Hex => decode_hex(s),
            Self::Base64 => decode_base64(s),
        }
    }

    /// Encode raw bytes into a string.
    pub fn encode(&self, data: &[u8]) -> std::string::String {
        match self {
            Self::String => std::string::String::from_utf8_lossy(data).into_owned(),
            Self::Hex => encode_hex(data),
            Self::Base64 => encode_base64(data),
        }
    }
}

impl fmt::Display for DataEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DataEncoding {
    type Err = EncodingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "string" => Ok(Self::String),
            "hex" => Ok(Self::Hex),
            "base64" => Ok(Self::Base64),
            _ => Err(EncodingError(format!("unknown encoding: {}", s))),
        }
    }
}

/// Error from encoding/decoding operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodingError(pub std::string::String);

impl fmt::Display for EncodingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// --- Hex (hand-rolled) ---

fn decode_hex(s: &str) -> Result<Vec<u8>, EncodingError> {
    if s.len() % 2 != 0 {
        return Err(EncodingError("hex string must have even length".into()));
    }
    let mut bytes = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        let byte = u8::from_str_radix(&s[i..i + 2], 16)
            .map_err(|_| EncodingError(format!("invalid hex at position {}", i)))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

fn encode_hex(data: &[u8]) -> std::string::String {
    let mut s = std::string::String::with_capacity(data.len() * 2);
    for b in data {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
    }
    s
}

// --- Base64 (minimal impl) ---

const B64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn decode_base64(s: &str) -> Result<Vec<u8>, EncodingError> {
    let s = s.trim_end_matches('=');
    let mut bytes = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for c in s.bytes() {
        let val = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'\n' | b'\r' | b' ' => continue,
            _ => return Err(EncodingError(format!("invalid base64 character: {}", c as char))),
        };
        buf = (buf << 6) | val as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(bytes)
}

fn encode_base64(data: &[u8]) -> std::string::String {
    let mut s = std::string::String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        s.push(B64_CHARS[((triple >> 18) & 0x3F) as usize] as char);
        s.push(B64_CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            s.push(B64_CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            s.push('=');
        }
        if chunk.len() > 2 {
            s.push(B64_CHARS[(triple & 0x3F) as usize] as char);
        } else {
            s.push('=');
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- from_str ---

    #[test]
    fn from_str_string() {
        assert_eq!("string".parse::<DataEncoding>().unwrap(), DataEncoding::String);
    }

    #[test]
    fn from_str_hex() {
        assert_eq!("hex".parse::<DataEncoding>().unwrap(), DataEncoding::Hex);
    }

    #[test]
    fn from_str_base64() {
        assert_eq!("base64".parse::<DataEncoding>().unwrap(), DataEncoding::Base64);
    }

    #[test]
    fn from_str_unknown() {
        assert!("unknown".parse::<DataEncoding>().is_err());
    }

    #[test]
    fn from_str_empty() {
        assert!("".parse::<DataEncoding>().is_err());
    }

    // --- as_str round-trip with from_str ---

    #[test]
    fn as_str_from_str_roundtrip() {
        for enc in [DataEncoding::String, DataEncoding::Hex, DataEncoding::Base64] {
            assert_eq!(enc.as_str().parse::<DataEncoding>().unwrap(), enc);
        }
    }

    // --- display ---

    #[test]
    fn display() {
        assert_eq!(format!("{}", DataEncoding::Hex), "hex");
    }

    // --- String encoding ---

    #[test]
    fn string_decode() {
        assert_eq!(DataEncoding::String.decode("Hello").unwrap(), b"Hello");
    }

    #[test]
    fn string_encode() {
        assert_eq!(DataEncoding::String.encode(b"Hello"), "Hello");
    }

    #[test]
    fn string_encode_invalid_utf8() {
        let result = DataEncoding::String.encode(&[0xFF, 0xFE]);
        assert!(result.contains('\u{FFFD}'));
    }

    #[test]
    fn string_roundtrip() {
        let original = "Hello, World!";
        let bytes = DataEncoding::String.decode(original).unwrap();
        assert_eq!(DataEncoding::String.encode(&bytes), original);
    }

    // --- Hex encoding ---

    #[test]
    fn hex_decode_hello() {
        assert_eq!(DataEncoding::Hex.decode("48656C6C6F").unwrap(), b"Hello");
    }

    #[test]
    fn hex_decode_lowercase() {
        assert_eq!(DataEncoding::Hex.decode("48656c6c6f").unwrap(), b"Hello");
    }

    #[test]
    fn hex_decode_empty() {
        assert_eq!(DataEncoding::Hex.decode("").unwrap(), b"");
    }

    #[test]
    fn hex_decode_odd_length() {
        assert!(DataEncoding::Hex.decode("ABC").is_err());
    }

    #[test]
    fn hex_decode_invalid_chars() {
        assert!(DataEncoding::Hex.decode("ZZZZ").is_err());
    }

    #[test]
    fn hex_roundtrip() {
        let data = b"Hello, World!";
        let hex = DataEncoding::Hex.encode(data);
        assert_eq!(DataEncoding::Hex.decode(&hex).unwrap(), data);
    }

    // --- Base64 encoding ---

    #[test]
    fn base64_decode_hello() {
        assert_eq!(DataEncoding::Base64.decode("SGVsbG8=").unwrap(), b"Hello");
    }

    #[test]
    fn base64_decode_no_padding() {
        assert_eq!(DataEncoding::Base64.decode("SGVsbG8").unwrap(), b"Hello");
    }

    #[test]
    fn base64_decode_aqid() {
        assert_eq!(DataEncoding::Base64.decode("AQID").unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn base64_decode_empty() {
        assert_eq!(DataEncoding::Base64.decode("").unwrap(), b"");
    }

    #[test]
    fn base64_decode_invalid_char() {
        assert!(DataEncoding::Base64.decode("@@@").is_err());
    }

    #[test]
    fn base64_roundtrip() {
        let data = b"Hello, World!";
        let b64 = DataEncoding::Base64.encode(data);
        assert_eq!(DataEncoding::Base64.decode(&b64).unwrap(), data);
    }

    #[test]
    fn base64_single_byte() {
        let data = b"\x01";
        let b64 = DataEncoding::Base64.encode(data);
        assert_eq!(DataEncoding::Base64.decode(&b64).unwrap(), data);
    }

    #[test]
    fn base64_two_bytes() {
        let data = b"\x01\x02";
        let b64 = DataEncoding::Base64.encode(data);
        assert_eq!(DataEncoding::Base64.decode(&b64).unwrap(), data);
    }

    // --- Cross-encoding ---

    #[test]
    fn all_encodings_roundtrip_binary() {
        let data: Vec<u8> = (0..=255).collect();
        for enc in [DataEncoding::Hex, DataEncoding::Base64] {
            let encoded = enc.encode(&data);
            assert_eq!(enc.decode(&encoded).unwrap(), data, "failed for {:?}", enc);
        }
    }
}
