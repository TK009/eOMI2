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

type Req<'a> =
    esp_idf_svc::http::server::Request<&'a mut esp_idf_svc::http::server::EspHttpConnection>;

fn send_json(req: Req<'_>, status: u16, reason: &str, body: &[u8]) {
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

/// Write decompressed output from a single feed() call to OTA flash.
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
    true
}

/// Handle a POST /ota request — streaming gzip firmware upload.
///
/// Steps: (1) auth, (2) OTA lock, (3) Content-Length check, (4) gzip magic,
/// (5-7) streaming read→decompress→write, (8) validate, (9) set boot, (10) reboot.
pub fn handle_ota(mut req: Req<'_>, api_token: &str) {
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

    // (5) Read first 2 bytes for gzip magic
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
    if magic_pos < 2 || magic != GZIP_MAGIC {
        send_json(
            req,
            400,
            "Bad Request",
            &json_err("Payload must be gzip compressed"),
        );
        return;
    }

    info!("OTA: starting, compressed={}B", content_len);

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

    // Begin OTA (size=0 → sequential writes, erase as needed)
    let mut handle: esp_idf_svc::sys::esp_ota_handle_t = 0;
    let err =
        unsafe { esp_idf_svc::sys::esp_ota_begin(partition, 0, &mut handle) };
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

    // (6)+(7) Streaming read → GzipStreamDecompressor → esp_ota_write
    let start_us = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
    let mut gz = GzipStreamDecompressor::new();
    let mut buf = [0u8; OTA_READ_BUF];
    let mut total_in: usize = 2; // magic bytes already read
    let mut total_out: usize = 0;

    // Feed the gzip magic bytes through decompressor
    match gz.feed(&magic) {
        Ok(out) => {
            if !write_decompressed(out, handle, &mut total_out) {
                abort_ota(handle);
                send_json(req, 500, "Internal Server Error", &json_err("OTA write failed"));
                return;
            }
        }
        Err(_) => {
            abort_ota(handle);
            send_json(req, 400, "Bad Request", &json_err("Decompression failed"));
            return;
        }
    }

    // Stream remaining body
    while total_in < content_len {
        let now = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
        if now - start_us > OTA_TIMEOUT_US {
            warn!("OTA: timeout after {}s", (now - start_us) / 1_000_000);
            abort_ota(handle);
            send_json(req, 408, "Request Timeout", &json_err("OTA timeout exceeded"));
            return;
        }

        let to_read = core::cmp::min(OTA_READ_BUF, content_len - total_in);
        let n = match req.read(&mut buf[..to_read]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                warn!("OTA: read error: {}", e);
                abort_ota(handle);
                send_json(
                    req,
                    500,
                    "Internal Server Error",
                    &json_err("Body read failed"),
                );
                return;
            }
        };
        total_in += n;

        match gz.feed(&buf[..n]) {
            Ok(out) => {
                if !write_decompressed(out, handle, &mut total_out) {
                    abort_ota(handle);
                    send_json(
                        req,
                        500,
                        "Internal Server Error",
                        &json_err("OTA write failed"),
                    );
                    return;
                }
            }
            Err(e) => {
                warn!("OTA: decompress: {:?}", e);
                abort_ota(handle);
                send_json(req, 400, "Bad Request", &json_err("Decompression failed"));
                return;
            }
        }
    }

    // Verify gzip stream completed
    if let Err(e) = gz.finish() {
        warn!("OTA: gzip finish: {:?}", e);
        abort_ota(handle);
        let msg = match e {
            GzipStreamError::Truncated => "Gzip stream truncated",
            GzipStreamError::CrcMismatch => "Gzip CRC mismatch",
            GzipStreamError::SizeMismatch => "Gzip size mismatch",
            _ => "Decompression error",
        };
        send_json(req, 400, "Bad Request", &json_err(msg));
        return;
    }

    info!("OTA: decompressed {}B from {}B", total_out, total_in);

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
