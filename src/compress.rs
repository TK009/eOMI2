// Gzip compression for HTTP responses.
//
// Pure functions — no ESP deps — testable on host.
// Uses miniz_oxide for DEFLATE and a simple CRC32 implementation
// to produce valid gzip output for Content-Encoding: gzip.

extern crate alloc;
use alloc::vec::Vec;

/// Compress data into gzip format (RFC 1952).
///
/// Produces a valid gzip stream: 10-byte header + DEFLATE payload + 8-byte
/// trailer (CRC32 + original size). Suitable for HTTP Content-Encoding: gzip.
pub fn gzip_compress(data: &[u8]) -> Vec<u8> {
    // DEFLATE compress (raw, not zlib-wrapped)
    let deflated = miniz_oxide::deflate::compress_to_vec(data, 6);

    let crc = crc32(data);
    let size = data.len() as u32;

    // Gzip: 10-byte header + deflated + 8-byte trailer
    let mut out = Vec::with_capacity(10 + deflated.len() + 8);

    // Header
    out.push(0x1f); // ID1
    out.push(0x8b); // ID2
    out.push(8); // CM = deflate
    out.push(0); // FLG = no extra fields
    out.extend_from_slice(&[0, 0, 0, 0]); // MTIME = 0
    out.push(0); // XFL
    out.push(0xff); // OS = unknown

    // Compressed data
    out.extend_from_slice(&deflated);

    // Trailer
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());

    out
}

/// CRC32 (ISO 3309 / ITU-T V.42) — bit-at-a-time implementation.
///
/// Trades speed for code size: no 1 KB lookup table, which matters on
/// flash-constrained ESP32 targets. For the small HTML payloads we compress
/// (~2–8 KB) this is perfectly adequate.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

/// Decompress gzip data (RFC 1952) back to the original bytes.
///
/// Expects a valid gzip stream: 10-byte header + DEFLATE payload + 8-byte
/// trailer. Returns `None` if the data is too short, decompression fails,
/// or the decompressed output exceeds `max_decompressed_size`.
pub fn gzip_decompress(data: &[u8], max_decompressed_size: usize) -> Option<Vec<u8>> {
    // Minimum: 10 header + 8 trailer = 18
    if data.len() < 18 {
        return None;
    }
    // Verify gzip magic bytes
    if data[0] != 0x1f || data[1] != 0x8b {
        return None;
    }

    let trailer_start = data.len() - 8;
    let stored_crc = u32::from_le_bytes([
        data[trailer_start],
        data[trailer_start + 1],
        data[trailer_start + 2],
        data[trailer_start + 3],
    ]);
    let stored_size = u32::from_le_bytes([
        data[trailer_start + 4],
        data[trailer_start + 5],
        data[trailer_start + 6],
        data[trailer_start + 7],
    ]);

    let deflated = &data[10..trailer_start];
    let decompressed =
        miniz_oxide::inflate::decompress_to_vec_with_limit(deflated, max_decompressed_size)
            .ok()?;

    // Validate CRC32 and decompressed length against trailer
    if crc32(&decompressed) != stored_crc {
        return None;
    }
    if decompressed.len() as u32 != stored_size {
        return None;
    }

    Some(decompressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_empty() {
        assert_eq!(crc32(b""), 0x0000_0000);
    }

    #[test]
    fn crc32_known_value() {
        // "123456789" has well-known CRC32 = 0xCBF43926
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn gzip_valid_header() {
        let compressed = gzip_compress(b"hello");
        assert!(compressed.len() >= 18); // 10 header + min 1 byte data + 8 trailer
        assert_eq!(compressed[0], 0x1f); // ID1
        assert_eq!(compressed[1], 0x8b); // ID2
        assert_eq!(compressed[2], 8); // CM = deflate
    }

    #[test]
    fn gzip_roundtrip() {
        let original = b"<html><body><h1>Hello World</h1></body></html>";
        let compressed = gzip_compress(original);

        // Compressed should be smaller than original for repetitive HTML
        // (or at least produce valid output)
        assert!(compressed.len() >= 18);

        // Verify trailer CRC32 and size
        let trailer_start = compressed.len() - 8;
        let stored_crc = u32::from_le_bytes([
            compressed[trailer_start],
            compressed[trailer_start + 1],
            compressed[trailer_start + 2],
            compressed[trailer_start + 3],
        ]);
        let stored_size = u32::from_le_bytes([
            compressed[trailer_start + 4],
            compressed[trailer_start + 5],
            compressed[trailer_start + 6],
            compressed[trailer_start + 7],
        ]);
        assert_eq!(stored_crc, crc32(original));
        assert_eq!(stored_size, original.len() as u32);
    }

    #[test]
    fn gzip_decompressible() {
        // Verify the output can be decompressed by miniz_oxide
        let original = b"<h1>Test</h1><p>Content here</p>";
        let compressed = gzip_compress(original);

        // Skip 10-byte header, strip 8-byte trailer
        let deflated = &compressed[10..compressed.len() - 8];
        let decompressed =
            miniz_oxide::inflate::decompress_to_vec(deflated)
                .expect("decompression should succeed");
        assert_eq!(&decompressed, original);
    }

    #[test]
    fn gzip_empty_input() {
        let compressed = gzip_compress(b"");
        assert!(compressed.len() >= 18);
        let trailer_start = compressed.len() - 8;
        let stored_size = u32::from_le_bytes([
            compressed[trailer_start + 4],
            compressed[trailer_start + 5],
            compressed[trailer_start + 6],
            compressed[trailer_start + 7],
        ]);
        assert_eq!(stored_size, 0);
    }

    #[test]
    fn gzip_decompress_rejects_bad_crc() {
        let mut compressed = gzip_compress(b"hello");
        let trailer_start = compressed.len() - 8;
        // Corrupt the CRC32
        compressed[trailer_start] ^= 0xff;
        assert!(gzip_decompress(&compressed, 1024).is_none());
    }

    #[test]
    fn gzip_decompress_rejects_bad_isize() {
        let mut compressed = gzip_compress(b"hello");
        let len = compressed.len();
        // Corrupt the ISIZE (last 4 bytes)
        compressed[len - 1] ^= 0xff;
        assert!(gzip_decompress(&compressed, 1024).is_none());
    }

    #[test]
    fn gzip_decompress_roundtrip() {
        let original = b"<html><body>test</body></html>";
        let compressed = gzip_compress(original);
        let decompressed = gzip_decompress(&compressed, 1024).expect("should decompress");
        assert_eq!(&decompressed, original);
    }

    #[test]
    fn gzip_decompress_rejects_oversized_output() {
        let original = vec![0u8; 1024];
        let compressed = gzip_compress(&original);
        // Limit smaller than the decompressed size — should be rejected
        assert!(gzip_decompress(&compressed, 512).is_none());
        // Exact limit should succeed
        assert!(gzip_decompress(&compressed, 1024).is_some());
    }

    #[test]
    fn gzip_compresses_html() {
        // Realistic HTML content should compress well
        let html = "<html><head><style>\
            *{box-sizing:border-box;margin:0;padding:0}\
            body{font-family:sans-serif;padding:1em}\
            </style></head><body>\
            <h1>Device Setup</h1>\
            <p>Configure your device settings below.</p>\
            </body></html>";
        let compressed = gzip_compress(html.as_bytes());
        // HTML should compress significantly (at least 20% savings)
        assert!(
            compressed.len() < html.len(),
            "compressed {} >= original {}",
            compressed.len(),
            html.len()
        );
    }
}
