// OTA firmware update handler — ESP-only.
//
// POST /ota: streaming gzip-compressed firmware upload.
// FR-005 through FR-010b, FR-015 through FR-019.

use std::sync::atomic::{AtomicBool, Ordering};

use esp_idf_svc::io::{Read, Write};
use log::{info, warn};

use crate::compress::{GzipStreamDecompressor, GzipStreamError};
use crate::http::check_bearer_auth;

/// Maximum OTA partition size (from partitions.csv: 0x1E0000 = 1,966,080 bytes).
const OTA_PARTITION_SIZE: usize = 0x1E0000;

/// Read buffer size for streaming HTTP body.
const OTA_READ_BUF: usize = 4096;

/// Wall-clock timeout for the entire OTA operation (5 minutes) in microseconds.
const OTA_TIMEOUT_US: i64 = 5 * 60 * 1_000_000;

/// Gzip magic bytes (RFC 1952).
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// ESP-IDF application image magic byte (first byte of a valid image).
const ESP_IMAGE_MAGIC: u8 = 0xE9;

/// Categorised OTA streaming failure — mapped to specific HTTP responses.
enum OtaStreamError {
    Timeout,
    ReadFailed,
    WriteFailed,
    DecompressFailed,
    GzipFinish(GzipStreamError),
}

/// Global OTA lock — only one update at a time.
static OTA_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// RAII guard that releases the OTA lock on drop.
struct OtaLockGuard;

impl OtaLockGuard {
    fn try_acquire() -> Option<Self> {
        OTA_IN_PROGRESS
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for OtaLockGuard {
    fn drop(&mut self) {
        OTA_IN_PROGRESS.store(false, Ordering::Release);
    }
}

type Req<'a, 'b> =
    esp_idf_svc::http::server::Request<&'a mut esp_idf_svc::http::server::EspHttpConnection<'b>>;

fn send_json(req: Req<'_, '_>, status: u16, reason: &str, body: &[u8]) {
    let headers = [("Content-Type", "application/json")];
    match req.into_response(status, Some(reason), &headers) {
        Ok(mut resp) => {
            if let Err(e) = resp.write_all(body) {
                warn!("OTA response write: {}", e);
            }
        }
        Err(e) => warn!("OTA response: {}", e),
    }
}

fn json_err(msg: &str) -> Vec<u8> {
    format!(r#"{{"status":"error","message":"{}"}}"#, msg).into_bytes()
}

fn abort_ota(handle: esp_idf_svc::sys::esp_ota_handle_t) {
    unsafe {
        let _ = esp_idf_svc::sys::esp_ota_abort(handle);
    }
}

/// Write output to OTA flash, yielding periodically.
///
/// Flash erase/write blocks the CPU; without yielding, the idle task starves
/// and the task watchdog panics after ~5 s.  A short sleep after each write
/// lets the scheduler run the WiFi stack, TCP ACK processing, and the
/// watchdog feeder.
fn write_decompressed(
    out: &[u8],
    handle: esp_idf_svc::sys::esp_ota_handle_t,
    total_out: &mut usize,
) -> bool {
    if out.is_empty() {
        return true;
    }
    let err = unsafe {
        esp_idf_svc::sys::esp_ota_write(handle, out.as_ptr() as *const _, out.len())
    };
    if err != esp_idf_svc::sys::ESP_OK {
        warn!("OTA: write failed: {}", err);
        return false;
    }
    *total_out += out.len();
    // Yield so the idle task / WiFi stack can run (prevents watchdog timeout).
    std::thread::sleep(std::time::Duration::from_millis(1));
    true
}

/// Read the next chunk from the HTTP body with timeout checking.
///
/// Returns `Ok(n)` with bytes read into `buf[..n]`, `Ok(0)` on EOF.
fn read_ota_chunk(
    req: &mut Req<'_, '_>,
    buf: &mut [u8],
    total_in: &mut usize,
    content_len: usize,
    start_us: i64,
) -> Result<usize, OtaStreamError> {
    let now = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
    if now - start_us > OTA_TIMEOUT_US {
        warn!("OTA: timeout after {}s", (now - start_us) / 1_000_000);
        return Err(OtaStreamError::Timeout);
    }
    let to_read = core::cmp::min(buf.len(), content_len - *total_in);
    match req.read(&mut buf[..to_read]) {
        Ok(0) => Ok(0),
        Ok(n) => {
            *total_in += n;
            Ok(n)
        }
        Err(e) => {
            warn!("OTA: read error: {}", e);
            Err(OtaStreamError::ReadFailed)
        }
    }
}

/// Stream gzip-compressed firmware: read → decompress → write to OTA flash.
///
/// On failure, aborts the OTA handle and returns the error category.
///
/// `#[inline(never)]` keeps the ~12 KB stack frame (8 KB GzipStreamDecompressor
/// + 4 KB read buffer) off `handle_ota`'s frame, which shares the 16 KB HTTP
/// thread stack with ESP-IDF framework code.
#[inline(never)]
fn stream_gzip_ota(
    req: &mut Req<'_, '_>,
    magic: &[u8; 2],
    handle: esp_idf_svc::sys::esp_ota_handle_t,
    content_len: usize,
    start_us: i64,
    total_in: &mut usize,
    total_out: &mut usize,
) -> Result<(), OtaStreamError> {
    // Box the decompressor (~8 KB struct with inflate_buf) to keep it off the
    // 16 KB HTTP thread stack — only an 8-byte pointer lives on the stack.
    let mut gz = Box::new(GzipStreamDecompressor::new());
    let mut buf = [0u8; OTA_READ_BUF];

    // Feed the gzip magic bytes through decompressor
    match gz.feed(magic) {
        Ok(out) => {
            if !write_decompressed(out, handle, total_out) {
                abort_ota(handle);
                return Err(OtaStreamError::WriteFailed);
            }
        }
        Err(_) => {
            abort_ota(handle);
            return Err(OtaStreamError::DecompressFailed);
        }
    }

    // Stream remaining body
    while *total_in < content_len {
        let n = match read_ota_chunk(req, &mut buf, total_in, content_len, start_us) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                abort_ota(handle);
                return Err(e);
            }
        };

        match gz.feed(&buf[..n]) {
            Ok(out) => {
                if !write_decompressed(out, handle, total_out) {
                    abort_ota(handle);
                    return Err(OtaStreamError::WriteFailed);
                }
            }
            Err(e) => {
                warn!("OTA: decompress: {:?}", e);
                abort_ota(handle);
                return Err(OtaStreamError::DecompressFailed);
            }
        }
    }

    // Verify gzip stream completed
    if let Err(e) = gz.finish() {
        warn!("OTA: gzip finish: {:?}", e);
        abort_ota(handle);
        return Err(OtaStreamError::GzipFinish(e));
    }

    Ok(())
}

/// Stream raw (uncompressed) firmware directly to OTA flash.
///
/// Avoids allocating the ~40 KB gzip InflateState, which may exceed free heap
/// on memory-constrained targets like the ESP32-S2.
///
/// On failure, aborts the OTA handle and returns the error category.
#[inline(never)]
fn stream_raw_ota(
    req: &mut Req<'_, '_>,
    magic: &[u8; 2],
    handle: esp_idf_svc::sys::esp_ota_handle_t,
    content_len: usize,
    start_us: i64,
    total_in: &mut usize,
    total_out: &mut usize,
) -> Result<(), OtaStreamError> {
    let mut buf = [0u8; OTA_READ_BUF];

    // Write the first 2 bytes (already read for format detection)
    if !write_decompressed(magic, handle, total_out) {
        abort_ota(handle);
        return Err(OtaStreamError::WriteFailed);
    }

    // Stream remaining body directly to flash
    while *total_in < content_len {
        let n = match read_ota_chunk(req, &mut buf, total_in, content_len, start_us) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                abort_ota(handle);
                return Err(e);
            }
        };

        if !write_decompressed(&buf[..n], handle, total_out) {
            abort_ota(handle);
            return Err(OtaStreamError::WriteFailed);
        }
    }

    Ok(())
}

/// Handle a POST /ota request — streaming firmware upload (gzip or raw).
///
/// Steps: (1) auth, (2) OTA lock, (3) Content-Length check, (4) format detect,
/// (5-7) streaming read→[decompress→]write, (8) validate, (9) set boot, (10) reboot.
pub fn handle_ota(mut req: Req<'_, '_>, api_token: &str) {
    // (2) Auth check
    if !check_bearer_auth(req.header("authorization"), api_token) {
        send_json(req, 401, "Unauthorized", &json_err("Authentication required"));
        return;
    }

    // (3) OTA lock
    let _lock = match OtaLockGuard::try_acquire() {
        Some(g) => g,
        None => {
            send_json(
                req,
                409,
                "Conflict",
                &json_err("OTA update already in progress"),
            );
            return;
        }
    };

    // (4) Content-Length validation
    let content_len: usize = match req.header("content-length").and_then(|v| v.parse().ok()) {
        Some(n) if n > 0 => n,
        _ => {
            send_json(
                req,
                400,
                "Bad Request",
                &json_err("Valid Content-Length required"),
            );
            return;
        }
    };
    if content_len > OTA_PARTITION_SIZE {
        send_json(
            req,
            400,
            "Bad Request",
            &json_err("Payload exceeds OTA partition size"),
        );
        return;
    }

    // (5) Read first 2 bytes to detect payload format
    let mut magic = [0u8; 2];
    let mut magic_pos = 0;
    while magic_pos < 2 {
        match req.read(&mut magic[magic_pos..]) {
            Ok(0) => break,
            Ok(n) => magic_pos += n,
            Err(e) => {
                warn!("OTA: magic read: {}", e);
                send_json(req, 400, "Bad Request", &json_err("Failed to read body"));
                return;
            }
        }
    }
    if magic_pos < 2 {
        send_json(req, 400, "Bad Request", &json_err("Payload too short"));
        return;
    }

    let is_gzip = magic == GZIP_MAGIC;
    if !is_gzip && magic[0] != ESP_IMAGE_MAGIC {
        send_json(
            req,
            400,
            "Bad Request",
            &json_err("Payload must be gzip-compressed or raw ESP firmware"),
        );
        return;
    }

    info!(
        "OTA: starting, {}={}B",
        if is_gzip { "compressed" } else { "raw" },
        content_len,
    );

    // Get next OTA partition
    let partition = unsafe {
        esp_idf_svc::sys::esp_ota_get_next_update_partition(core::ptr::null())
    };
    if partition.is_null() {
        send_json(
            req,
            500,
            "Internal Server Error",
            &json_err("No OTA partition available"),
        );
        return;
    }
    // Begin OTA with sequential writes — erase sectors incrementally during
    // esp_ota_write rather than erasing the entire ~2 MB partition upfront.
    // Passing 0 or OTA_SIZE_UNKNOWN would erase the whole partition in a
    // single blocking call (10–20 s), starving the WiFi stack and causing
    // TCP timeouts / task-watchdog panics.
    let mut handle: esp_idf_svc::sys::esp_ota_handle_t = 0;
    let err = unsafe {
        esp_idf_svc::sys::esp_ota_begin(
            partition,
            esp_idf_svc::sys::OTA_WITH_SEQUENTIAL_WRITES as usize,
            &mut handle,
        )
    };
    if err != esp_idf_svc::sys::ESP_OK {
        warn!("OTA: begin failed: {}", err);
        send_json(
            req,
            500,
            "Internal Server Error",
            &json_err("OTA begin failed"),
        );
        return;
    }

    let start_us = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
    let mut total_in: usize = 2; // magic bytes already read
    let mut total_out: usize = 0;

    // Each streaming helper owns its 4 KB read buffer on its own stack
    // frame; when it returns the frame is freed, leaving enough stack for
    // esp_ota_end's image-validation calls (HTTP thread = 16 KB stack).
    let result = if is_gzip {
        stream_gzip_ota(&mut req, &magic, handle, content_len, start_us, &mut total_in, &mut total_out)
    } else {
        stream_raw_ota(&mut req, &magic, handle, content_len, start_us, &mut total_in, &mut total_out)
    };

    if let Err(e) = result {
        let (status, reason, msg) = match e {
            OtaStreamError::Timeout => (408, "Request Timeout", "OTA timeout exceeded"),
            OtaStreamError::ReadFailed => (500, "Internal Server Error", "Body read failed"),
            OtaStreamError::WriteFailed => (500, "Internal Server Error", "OTA write failed"),
            OtaStreamError::DecompressFailed => (400, "Bad Request", "Decompression failed"),
            OtaStreamError::GzipFinish(ref ge) => match ge {
                GzipStreamError::Truncated => (400, "Bad Request", "Gzip stream truncated"),
                GzipStreamError::CrcMismatch => (400, "Bad Request", "Gzip CRC mismatch"),
                GzipStreamError::SizeMismatch => (400, "Bad Request", "Gzip size mismatch"),
                _ => (400, "Bad Request", "Decompression error"),
            },
        };
        send_json(req, status, reason, &json_err(msg));
        return;
    }

    info!("OTA: wrote {}B from {}B{}", total_out, total_in,
        if is_gzip { " (gzip)" } else { " (raw)" });

    // (8) esp_ota_end — validates the written image
    let err = unsafe { esp_idf_svc::sys::esp_ota_end(handle) };
    if err != esp_idf_svc::sys::ESP_OK {
        warn!("OTA: end failed: {}", err);
        send_json(
            req,
            500,
            "Internal Server Error",
            &json_err("OTA validation failed"),
        );
        return;
    }

    // (9) Set boot partition to the newly written slot
    let err = unsafe { esp_idf_svc::sys::esp_ota_set_boot_partition(partition) };
    if err != esp_idf_svc::sys::ESP_OK {
        warn!("OTA: set boot failed: {}", err);
        send_json(
            req,
            500,
            "Internal Server Error",
            &json_err("Set boot partition failed"),
        );
        return;
    }

    // (10) JSON success response, then 500ms delay + reboot
    info!("OTA: success, rebooting");
    let body = r#"{"status":"ok","message":"OTA update successful, rebooting"}"#;
    send_json(req, 200, "OK", body.as_bytes());

    std::thread::sleep(std::time::Duration::from_millis(500));
    unsafe {
        esp_idf_svc::sys::esp_restart();
    }
}
