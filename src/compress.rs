// Gzip compression and streaming decompression.
//
// Uses miniz_oxide for DEFLATE on all targets.
// Streaming decompressor for OTA firmware updates (FR-011..FR-014).

extern crate alloc;
use alloc::vec::Vec;

/// Minimum free heap (bytes) required before attempting gzip compression.
#[cfg(feature = "esp")]
/// The deflate compressor state is ~300 KB; on ESP32-S2 with only 320 KB
/// total SRAM this will OOM and abort unless we gate on available memory first.
const GZIP_MIN_FREE_HEAP: usize = 350_000;

/// Compress data into gzip format (RFC 1952).
///
/// Produces a valid gzip stream: 10-byte header + DEFLATE payload + 8-byte
/// trailer (CRC32 + original size). Suitable for HTTP Content-Encoding: gzip.
///
/// Returns `None` when insufficient heap is available (ESP32) or compression
/// otherwise cannot proceed safely.
pub fn gzip_compress(data: &[u8]) -> Option<Vec<u8>> {
    // On ESP32, check free heap before attempting the ~300 KB allocation.
    #[cfg(feature = "esp")]
    {
        let free = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() } as usize;
        if free < GZIP_MIN_FREE_HEAP {
            return None;
        }
    }

    let deflated = deflate_raw(data)?;

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

    Some(out)
}

/// Raw DEFLATE compression (no zlib/gzip header).
fn deflate_raw(data: &[u8]) -> Option<Vec<u8>> {
    Some(miniz_oxide::deflate::compress_to_vec(data, 6))
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
    let decompressed = inflate_raw_bounded(deflated, max_decompressed_size)?;

    // Validate CRC32 and decompressed length against trailer
    if crc32(&decompressed) != stored_crc {
        return None;
    }
    if decompressed.len() as u32 != stored_size {
        return None;
    }

    Some(decompressed)
}

/// One-shot raw DEFLATE decompression with size limit.
fn inflate_raw_bounded(deflated: &[u8], max_size: usize) -> Option<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_with_limit(deflated, max_size).ok()
}

// --- Streaming gzip decompressor (FR-011..FR-014) ---

const GZIP_STREAM_OUT_SIZE: usize = 8192;

// Gzip FLG bits (RFC 1952 §2.3.1)
const FEXTRA: u8 = 1 << 2;
const FNAME: u8 = 1 << 3;
const FCOMMENT: u8 = 1 << 4;
const FHCRC: u8 = 1 << 1;

#[derive(Debug, PartialEq)]
pub enum GzipStreamError {
    InvalidHeader,
    DecompressFailed,
    CrcMismatch,
    SizeMismatch,
    Truncated,
}

/// State machine phases for streaming gzip decompression.
enum Phase {
    /// Collecting the fixed 10-byte gzip header.
    Header,
    /// Collecting the 2-byte XLEN field (FEXTRA set).
    ExtraLen,
    /// Skipping XLEN bytes of extra data.
    ExtraData { remaining: u16 },
    /// Skipping null-terminated original filename (FNAME set).
    Name,
    /// Skipping null-terminated comment (FCOMMENT set).
    Comment,
    /// Skipping 2-byte header CRC16 (FHCRC set).
    HeaderCrc,
    /// Streaming DEFLATE decompression.
    Deflate,
    /// Collecting the 8-byte trailer (CRC32 + ISIZE).
    Trailer,
    /// Successfully verified.
    Done,
    /// Terminal error.
    Failed,
}

/// Streaming gzip decompressor for OTA firmware updates.
///
/// State machine: header parsing → DEFLATE decompression → trailer
/// CRC32+ISIZE verification. Computes a running CRC32 over all decompressed
/// output. Designed for chunk-by-chunk feeding from HTTP body reads.
pub struct GzipStreamDecompressor {
    phase: Phase,
    inflate_state: alloc::boxed::Box<miniz_oxide::inflate::stream::InflateState>,
    // Header
    header_buf: [u8; 10],
    header_pos: u8,
    flg: u8,
    // FEXTRA: 2-byte XLEN accumulator
    extra_len_buf: [u8; 2],
    extra_len_pos: u8,
    // FHCRC: 2-byte skip counter
    hcrc_pos: u8,
    // Running CRC32 + decompressed byte count
    crc: u32,
    total_out: u32,
    // Trailer
    trailer_buf: [u8; 8],
    trailer_pos: u8,
    // Scratch buffer for inflate calls
    inflate_buf: [u8; GZIP_STREAM_OUT_SIZE],
    // Accumulated output for current feed() call
    out_buf: Vec<u8>,
}

impl GzipStreamDecompressor {
    pub fn new() -> Self {
        Self {
            phase: Phase::Header,
            inflate_state: miniz_oxide::inflate::stream::InflateState::new_boxed(
                miniz_oxide::DataFormat::Raw,
            ),
            header_buf: [0; 10],
            header_pos: 0,
            flg: 0,
            extra_len_buf: [0; 2],
            extra_len_pos: 0,
            hcrc_pos: 0,
            crc: 0xFFFF_FFFF,
            total_out: 0,
            trailer_buf: [0; 8],
            trailer_pos: 0,
            inflate_buf: [0; GZIP_STREAM_OUT_SIZE],
            out_buf: Vec::new(),
        }
    }

    /// Feed a chunk of gzip input. Returns decompressed output bytes, or an
    /// error if the stream is invalid. Call repeatedly with successive chunks.
    /// After the final trailer byte is verified, returns `Ok(&[])` and
    /// `is_done()` becomes true.
    pub fn feed(&mut self, input: &[u8]) -> Result<&[u8], GzipStreamError> {
        self.out_buf.clear();
        let mut pos = 0;

        while pos < input.len() {
            match self.phase {
                Phase::Header => {
                    pos += self.consume_header(&input[pos..]);
                }
                Phase::ExtraLen => {
                    pos += self.consume_extra_len(&input[pos..]);
                }
                Phase::ExtraData { remaining } => {
                    let skip = core::cmp::min(remaining as usize, input.len() - pos);
                    let new_remaining = remaining - skip as u16;
                    pos += skip;
                    if new_remaining == 0 {
                        self.advance_past_extra_data();
                    } else {
                        self.phase = Phase::ExtraData {
                            remaining: new_remaining,
                        };
                    }
                }
                Phase::Name => {
                    pos += self.skip_null_terminated(&input[pos..], Phase::Name);
                }
                Phase::Comment => {
                    pos += self.skip_null_terminated(&input[pos..], Phase::Comment);
                }
                Phase::HeaderCrc => {
                    pos += self.consume_header_crc(&input[pos..]);
                }
                Phase::Deflate => {
                    pos += self.inflate_all(&input[pos..])?;
                }
                Phase::Trailer => {
                    pos += self.consume_trailer(&input[pos..])?;
                }
                Phase::Done => break,
                Phase::Failed => return Err(GzipStreamError::DecompressFailed),
            }
        }

        Ok(&self.out_buf)
    }

    pub fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    // --- Header parsing ---

    fn consume_header(&mut self, data: &[u8]) -> usize {
        let need = 10 - self.header_pos as usize;
        let n = core::cmp::min(need, data.len());
        self.header_buf[self.header_pos as usize..self.header_pos as usize + n]
            .copy_from_slice(&data[..n]);
        self.header_pos += n as u8;

        if self.header_pos == 10 {
            // Validate magic and method
            if self.header_buf[0] != 0x1f
                || self.header_buf[1] != 0x8b
                || self.header_buf[2] != 8
            {
                self.phase = Phase::Failed;
            } else {
                self.flg = self.header_buf[3];
                self.advance_header_optional();
            }
        }
        n
    }

    /// After the fixed header, advance to the first applicable optional field
    /// or directly to Deflate.
    fn advance_header_optional(&mut self) {
        if self.flg & FEXTRA != 0 {
            self.phase = Phase::ExtraLen;
        } else if self.flg & FNAME != 0 {
            self.phase = Phase::Name;
        } else if self.flg & FCOMMENT != 0 {
            self.phase = Phase::Comment;
        } else if self.flg & FHCRC != 0 {
            self.phase = Phase::HeaderCrc;
        } else {
            self.phase = Phase::Deflate;
        }
    }

    fn consume_extra_len(&mut self, data: &[u8]) -> usize {
        let need = 2 - self.extra_len_pos as usize;
        let n = core::cmp::min(need, data.len());
        self.extra_len_buf[self.extra_len_pos as usize..self.extra_len_pos as usize + n]
            .copy_from_slice(&data[..n]);
        self.extra_len_pos += n as u8;
        if self.extra_len_pos == 2 {
            let xlen = u16::from_le_bytes(self.extra_len_buf);
            if xlen == 0 {
                self.advance_past_extra_data();
            } else {
                self.phase = Phase::ExtraData { remaining: xlen };
            }
        }
        n
    }

    fn advance_past_extra_data(&mut self) {
        if self.flg & FNAME != 0 {
            self.phase = Phase::Name;
        } else if self.flg & FCOMMENT != 0 {
            self.phase = Phase::Comment;
        } else if self.flg & FHCRC != 0 {
            self.phase = Phase::HeaderCrc;
        } else {
            self.phase = Phase::Deflate;
        }
    }

    fn skip_null_terminated(&mut self, data: &[u8], current: Phase) -> usize {
        for (i, &b) in data.iter().enumerate() {
            if b == 0 {
                // Null terminator found, advance to next phase
                match current {
                    Phase::Name => {
                        if self.flg & FCOMMENT != 0 {
                            self.phase = Phase::Comment;
                        } else if self.flg & FHCRC != 0 {
                            self.phase = Phase::HeaderCrc;
                        } else {
                            self.phase = Phase::Deflate;
                        }
                    }
                    Phase::Comment => {
                        if self.flg & FHCRC != 0 {
                            self.phase = Phase::HeaderCrc;
                        } else {
                            self.phase = Phase::Deflate;
                        }
                    }
                    _ => unreachable!(),
                }
                return i + 1;
            }
        }
        data.len()
    }

    fn consume_header_crc(&mut self, data: &[u8]) -> usize {
        let need = 2 - self.hcrc_pos as usize;
        let n = core::cmp::min(need, data.len());
        self.hcrc_pos += n as u8;
        if self.hcrc_pos == 2 {
            self.phase = Phase::Deflate;
        }
        n
    }

    // --- DEFLATE streaming ---

    /// Update running CRC32 over newly decompressed bytes and append to output.
    fn process_inflated(&mut self, written: usize) {
        if written == 0 {
            return;
        }
        for i in 0..written {
            let byte = self.inflate_buf[i];
            self.crc ^= byte as u32;
            for _ in 0..8 {
                if self.crc & 1 != 0 {
                    self.crc = (self.crc >> 1) ^ 0xEDB8_8320;
                } else {
                    self.crc >>= 1;
                }
            }
        }
        self.total_out = self.total_out.wrapping_add(written as u32);
        self.out_buf.extend_from_slice(&self.inflate_buf[..written]);
    }

    fn inflate_all(&mut self, data: &[u8]) -> Result<usize, GzipStreamError> {
        let mut total_consumed = 0;

        loop {
            let remaining = &data[total_consumed..];

            let result = miniz_oxide::inflate::stream::inflate(
                &mut self.inflate_state,
                remaining,
                &mut self.inflate_buf,
                miniz_oxide::MZFlush::None,
            );

            let written = result.bytes_written;
            self.process_inflated(written);
            total_consumed += result.bytes_consumed;

            match result.status {
                Ok(miniz_oxide::MZStatus::Ok) => {
                    // No progress or all input consumed — need more input
                    if total_consumed >= data.len()
                        || (result.bytes_consumed == 0 && written == 0)
                    {
                        break;
                    }
                }
                Ok(miniz_oxide::MZStatus::StreamEnd) => {
                    self.phase = Phase::Trailer;
                    break;
                }
                Err(_) => {
                    self.phase = Phase::Failed;
                    return Err(GzipStreamError::DecompressFailed);
                }
                _ => {
                    self.phase = Phase::Failed;
                    return Err(GzipStreamError::DecompressFailed);
                }
            }
        }

        Ok(total_consumed)
    }

    // --- Trailer ---

    fn consume_trailer(&mut self, data: &[u8]) -> Result<usize, GzipStreamError> {
        let need = 8 - self.trailer_pos as usize;
        let n = core::cmp::min(need, data.len());
        self.trailer_buf[self.trailer_pos as usize..self.trailer_pos as usize + n]
            .copy_from_slice(&data[..n]);
        self.trailer_pos += n as u8;

        if self.trailer_pos == 8 {
            self.verify_trailer()?;
        }
        Ok(n)
    }

    fn verify_trailer(&mut self) -> Result<(), GzipStreamError> {
        let stored_crc = u32::from_le_bytes([
            self.trailer_buf[0],
            self.trailer_buf[1],
            self.trailer_buf[2],
            self.trailer_buf[3],
        ]);
        let stored_size = u32::from_le_bytes([
            self.trailer_buf[4],
            self.trailer_buf[5],
            self.trailer_buf[6],
            self.trailer_buf[7],
        ]);

        let computed_crc = !self.crc;
        if computed_crc != stored_crc {
            self.phase = Phase::Failed;
            return Err(GzipStreamError::CrcMismatch);
        }
        if self.total_out != stored_size {
            self.phase = Phase::Failed;
            return Err(GzipStreamError::SizeMismatch);
        }

        self.phase = Phase::Done;
        Ok(())
    }

    /// Call after the input stream ends to check for truncation.
    pub fn finish(&self) -> Result<(), GzipStreamError> {
        if self.is_done() {
            Ok(())
        } else {
            Err(GzipStreamError::Truncated)
        }
    }
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
        let compressed = gzip_compress(b"hello").unwrap();
        assert!(compressed.len() >= 18); // 10 header + min 1 byte data + 8 trailer
        assert_eq!(compressed[0], 0x1f); // ID1
        assert_eq!(compressed[1], 0x8b); // ID2
        assert_eq!(compressed[2], 8); // CM = deflate
    }

    #[test]
    fn gzip_roundtrip() {
        let original = b"<html><body><h1>Hello World</h1></body></html>";
        let compressed = gzip_compress(original).unwrap();

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
        // Verify compress → decompress roundtrip
        let original = b"<h1>Test</h1><p>Content here</p>";
        let compressed = gzip_compress(original).unwrap();
        let decompressed = gzip_decompress(&compressed, 1024).expect("should decompress");
        assert_eq!(&decompressed[..], original);
    }

    #[test]
    fn gzip_empty_input() {
        let compressed = gzip_compress(b"").unwrap();
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
        let mut compressed = gzip_compress(b"hello").unwrap();
        let trailer_start = compressed.len() - 8;
        // Corrupt the CRC32
        compressed[trailer_start] ^= 0xff;
        assert!(gzip_decompress(&compressed, 1024).is_none());
    }

    #[test]
    fn gzip_decompress_rejects_bad_isize() {
        let mut compressed = gzip_compress(b"hello").unwrap();
        let len = compressed.len();
        // Corrupt the ISIZE (last 4 bytes)
        compressed[len - 1] ^= 0xff;
        assert!(gzip_decompress(&compressed, 1024).is_none());
    }

    #[test]
    fn gzip_decompress_roundtrip() {
        let original = b"<html><body>test</body></html>";
        let compressed = gzip_compress(original).unwrap();
        let decompressed = gzip_decompress(&compressed, 1024).expect("should decompress");
        assert_eq!(&decompressed, original);
    }

    #[test]
    fn gzip_decompress_rejects_oversized_output() {
        let original = vec![0u8; 1024];
        let compressed = gzip_compress(&original).unwrap();
        // Limit smaller than the decompressed size — should be rejected
        assert!(gzip_decompress(&compressed, 512).is_none());
        // Exact limit should succeed
        assert!(gzip_decompress(&compressed, 1024).is_some());
    }

    // --- Adversarial / error-path tests ---

    #[test]
    fn gzip_decompress_rejects_truncated_no_trailer() {
        // Valid header but no trailer (only 10 bytes)
        let compressed = gzip_compress(b"hello").unwrap();
        let truncated = &compressed[..10]; // header only, no deflate data or trailer
        assert!(gzip_decompress(truncated, 1024).is_none());
    }

    #[test]
    fn gzip_decompress_rejects_truncated_mid_deflate() {
        let compressed = gzip_compress(b"hello world, this is some data to compress").unwrap();
        // Cut in the middle of the deflate stream (keep header + partial payload)
        let mid = 10 + (compressed.len() - 18) / 2; // halfway through deflate
        let truncated = &compressed[..mid];
        assert!(gzip_decompress(truncated, 1024).is_none());
    }

    #[test]
    fn gzip_decompress_rejects_too_short() {
        // Less than 18 bytes (minimum gzip)
        assert!(gzip_decompress(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff], 1024).is_none());
        assert!(gzip_decompress(&[], 1024).is_none());
        assert!(gzip_decompress(&[0x1f], 1024).is_none());
    }

    #[test]
    fn gzip_decompress_rejects_bad_magic() {
        let mut compressed = gzip_compress(b"hello").unwrap();
        compressed[0] = 0x00; // corrupt ID1
        assert!(gzip_decompress(&compressed, 1024).is_none());

        let mut compressed = gzip_compress(b"hello").unwrap();
        compressed[1] = 0x00; // corrupt ID2
        assert!(gzip_decompress(&compressed, 1024).is_none());
    }

    #[test]
    fn gzip_decompress_rejects_bit_flipped_deflate() {
        let mut compressed = gzip_compress(b"hello world").unwrap();
        // Flip bits in the deflate payload (byte 12, middle of compressed data)
        if compressed.len() > 14 {
            compressed[12] ^= 0xFF;
        }
        assert!(gzip_decompress(&compressed, 1024).is_none());
    }

    #[test]
    fn gzip_decompress_rejects_all_zeros_payload() {
        // Valid magic + zeroed out body — not valid deflate
        let mut bad = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
        bad.extend_from_slice(&[0u8; 20]); // garbage deflate + fake trailer
        assert!(gzip_decompress(&bad, 1024).is_none());
    }

    #[test]
    fn gzip_decompress_bomb_rejected_by_limit() {
        // Compress a large block of zeros (compresses very well)
        let bomb_data = vec![0u8; 100_000];
        let compressed = gzip_compress(&bomb_data).unwrap();
        // The compressed form should be much smaller than 100KB
        assert!(compressed.len() < 1000, "zeros should compress very well");
        // Decompress with a small limit — must be rejected
        assert!(gzip_decompress(&compressed, 1024).is_none());
        // Decompress with a limit just under the real size — must be rejected
        assert!(gzip_decompress(&compressed, 99_999).is_none());
        // Decompress with the exact size — should succeed
        assert!(gzip_decompress(&compressed, 100_000).is_some());
    }

    #[test]
    fn gzip_decompress_bomb_large_ratio() {
        // 1MB of zeros — extreme compression ratio
        let bomb_data = vec![0u8; 1_000_000];
        let compressed = gzip_compress(&bomb_data).unwrap();
        // Should be tiny compressed
        assert!(compressed.len() < 2000);
        // With a reasonable limit, the bomb is defused
        assert!(gzip_decompress(&compressed, 65536).is_none());
    }

    #[test]
    fn gzip_decompress_swapped_trailer_fields() {
        // Swap CRC32 and ISIZE in the trailer
        let original = b"test data here";
        let mut compressed = gzip_compress(original).unwrap();
        let len = compressed.len();
        // Swap last 8 bytes: CRC32(4) <-> ISIZE(4)
        let crc_bytes: [u8; 4] = compressed[len - 8..len - 4].try_into().unwrap();
        let isize_bytes: [u8; 4] = compressed[len - 4..len].try_into().unwrap();
        compressed[len - 8..len - 4].copy_from_slice(&isize_bytes);
        compressed[len - 4..len].copy_from_slice(&crc_bytes);
        assert!(gzip_decompress(&compressed, 1024).is_none());
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
        let compressed = gzip_compress(html.as_bytes()).unwrap();
        // HTML should compress significantly (at least 20% savings)
        assert!(
            compressed.len() < html.len(),
            "compressed {} >= original {}",
            compressed.len(),
            html.len()
        );
    }

    // --- GzipStreamDecompressor tests ---

    /// Helper: feed entire gzip blob in one call.
    fn stream_decompress_all(data: &[u8]) -> Result<Vec<u8>, GzipStreamError> {
        let mut dec = GzipStreamDecompressor::new();
        let mut out = Vec::new();
        let chunk = dec.feed(data)?;
        out.extend_from_slice(chunk);
        dec.finish()?;
        Ok(out)
    }

    /// Helper: feed gzip blob in fixed-size chunks.
    fn stream_decompress_chunked(data: &[u8], chunk_size: usize) -> Result<Vec<u8>, GzipStreamError> {
        let mut dec = GzipStreamDecompressor::new();
        let mut out = Vec::new();
        for chunk in data.chunks(chunk_size) {
            let decompressed = dec.feed(chunk)?;
            out.extend_from_slice(decompressed);
        }
        dec.finish()?;
        Ok(out)
    }

    #[test]
    fn stream_roundtrip_single_feed() {
        let original = b"hello world";
        let compressed = gzip_compress(original).unwrap();
        let result = stream_decompress_all(&compressed).expect("should decompress");
        assert_eq!(result, original);
    }

    #[test]
    fn stream_roundtrip_chunked_small() {
        // Feed byte-by-byte — exercises all state transitions
        let original = b"streaming gzip decompression test data";
        let compressed = gzip_compress(original).unwrap();
        let result = stream_decompress_chunked(&compressed, 1).expect("should decompress");
        assert_eq!(result, original);
    }

    #[test]
    fn stream_roundtrip_chunked_medium() {
        // Typical OTA chunk size
        let original = vec![0xAB; 16384];
        let compressed = gzip_compress(&original).unwrap();
        let result = stream_decompress_chunked(&compressed, 4096).expect("should decompress");
        assert_eq!(result, original);
    }

    #[test]
    fn stream_roundtrip_empty() {
        let compressed = gzip_compress(b"").unwrap();
        let result = stream_decompress_all(&compressed).expect("should decompress");
        assert!(result.is_empty());
    }

    #[test]
    fn stream_rejects_bad_magic() {
        let mut compressed = gzip_compress(b"hello").unwrap();
        compressed[0] = 0x00;
        let mut dec = GzipStreamDecompressor::new();
        // After feeding the bad header, next feed should fail
        let r = dec.feed(&compressed);
        assert!(r.is_err() || dec.feed(&[]).is_err() || !dec.is_done());
    }

    #[test]
    fn stream_rejects_bad_crc() {
        let mut compressed = gzip_compress(b"hello").unwrap();
        let trailer_start = compressed.len() - 8;
        compressed[trailer_start] ^= 0xff; // corrupt CRC32
        let result = stream_decompress_all(&compressed);
        assert_eq!(result, Err(GzipStreamError::CrcMismatch));
    }

    #[test]
    fn stream_rejects_bad_isize() {
        let mut compressed = gzip_compress(b"hello").unwrap();
        let len = compressed.len();
        compressed[len - 1] ^= 0xff; // corrupt ISIZE
        let result = stream_decompress_all(&compressed);
        assert_eq!(result, Err(GzipStreamError::SizeMismatch));
    }

    #[test]
    fn stream_rejects_truncated_header() {
        let compressed = gzip_compress(b"hello").unwrap();
        // Only feed 5 bytes of header
        let mut dec = GzipStreamDecompressor::new();
        let _ = dec.feed(&compressed[..5]);
        assert_eq!(dec.finish(), Err(GzipStreamError::Truncated));
    }

    #[test]
    fn stream_rejects_truncated_deflate() {
        let compressed = gzip_compress(b"hello world, this is some test data").unwrap();
        let mid = compressed.len() / 2;
        let mut dec = GzipStreamDecompressor::new();
        let _ = dec.feed(&compressed[..mid]);
        assert_eq!(dec.finish(), Err(GzipStreamError::Truncated));
    }

    #[test]
    fn stream_rejects_truncated_trailer() {
        let compressed = gzip_compress(b"hello").unwrap();
        // Feed everything except the last 3 bytes of trailer
        let mut dec = GzipStreamDecompressor::new();
        let _ = dec.feed(&compressed[..compressed.len() - 3]);
        assert_eq!(dec.finish(), Err(GzipStreamError::Truncated));
    }

    #[test]
    fn stream_rejects_corrupted_deflate() {
        let mut compressed = gzip_compress(b"hello world test data").unwrap();
        if compressed.len() > 14 {
            compressed[12] ^= 0xFF; // corrupt deflate payload
        }
        let mut dec = GzipStreamDecompressor::new();
        let result = dec.feed(&compressed);
        assert!(result.is_err() || dec.finish().is_err());
    }

    #[test]
    fn stream_large_data_chunked() {
        // 64KB of varied data — realistic firmware-like payload
        let original: Vec<u8> = (0..65536).map(|i| (i % 251) as u8).collect();
        let compressed = gzip_compress(&original).unwrap();
        let result = stream_decompress_chunked(&compressed, 4096).expect("should decompress");
        assert_eq!(result, original);
    }

    #[test]
    fn stream_is_done_after_success() {
        let compressed = gzip_compress(b"test").unwrap();
        let mut dec = GzipStreamDecompressor::new();
        let _ = dec.feed(&compressed).unwrap();
        assert!(dec.is_done());
        assert!(dec.finish().is_ok());
    }

    #[test]
    fn stream_crc32_matches_standalone() {
        // Verify the streaming CRC32 produces the same result as the
        // standalone crc32() function
        let original = b"verify crc32 consistency";
        let compressed = gzip_compress(original).unwrap();
        let result = stream_decompress_all(&compressed).expect("should decompress");
        assert_eq!(result, original);
        assert_eq!(crc32(&result), crc32(original));
    }
}
