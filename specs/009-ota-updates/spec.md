# Feature Specification: Over-The-Air Firmware Updates

**Feature Branch**: `009-ota-updates`
**Created**: 2026-03-15
**Status**: Reviewed
**Input**: User description: "OTA firmware updates. API key required. E2e tested. Use compression. Two app slots (no factory), keep data partition."

## Clarifications

### Session 2026-03-15

- Q: Partition layout? → A: Two OTA app slots (`ota_0`, `ota_1`), no factory partition. Keep NVS and data partitions. Custom `partitions.csv` required.
- Q: Compression format? → A: Gzip. Device receives gzip-compressed firmware image over HTTP, decompresses on-the-fly during flash write. The project already has `gzip_decompress()` in `src/compress.rs`.
- Q: Auth? → A: Same bearer token auth as all other mutating endpoints. API key checked before accepting any OTA data.
- Q: How to trigger? → A: HTTP POST to `/ota` with firmware binary as request body.
- Q: Rollback? → A: ESP-IDF anti-rollback via `esp_ota_mark_app_valid_cancel_rollback()`. New firmware must self-validate on first boot; if it doesn't, watchdog reboots into previous slot.
- Q: E2E test? → A: Build a second firmware with a different version string, OTA-upload it, verify the device reboots with the new version.
- Q: Streaming body reads? → A: Confirmed. `esp-idf-svc` v0.51 request implements `std::io::Read`. `req.read(&mut buf)` calls ESP-IDF `httpd_req_recv()` underneath. Chunked loop pattern works for streaming.
- Q: OTA binary format? → A: `espflash save-image --chip esp32s2 --format esp-idf <ELF> <output.bin>` produces the app-only image (with ESP-IDF app header, no bootloader/partition table). This is what `esp_ota_write()` expects.
- Q: Version differentiation for e2e? → A: Override version via env var (e.g., `FIRMWARE_VERSION`), injected by `build.rs` into a compile-time constant. Default to `CARGO_PKG_VERSION` when env var is not set.
- Q: Response format for `/ota`? → A: Plain JSON (not OMI envelope). OTA is not an OMI operation.
- Q: Upload timeout? → A: Yes. 5-minute maximum for the entire OTA upload. Prevents stalled clients from holding the OTA lock indefinitely.

---

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Upload Firmware Over HTTP (Priority: P1)

A device operator has a new firmware binary (gzip-compressed). They POST it to the device's `/ota` endpoint with their API key. The device validates the API key, decompresses the image, writes it to the inactive OTA slot, and reboots into the new firmware. NVS data (WiFi credentials, API key, O-DF tree) survives the update.

**Why this priority**: OTA is the only way to update deployed devices without physical access. This is the core capability — without it, firmware updates require USB.

**Independent Test**: Can be tested by building firmware with version string "A", flashing via USB, then building version "B", gzip-compressing it, POSTing to `/ota`, and verifying the device reboots running version "B" with WiFi and API key intact.

**Acceptance Scenarios**:

1. **Given** a device running firmware version "A", **When** an authenticated POST to `/ota` sends gzip-compressed firmware version "B", **Then** the device writes version "B" to the inactive OTA slot, reboots, and responds with version "B" on subsequent requests.
2. **Given** a device with saved WiFi credentials and API key, **When** an OTA update completes and the device reboots, **Then** WiFi reconnects automatically and the same API key is accepted.
3. **Given** a device with O-DF tree data persisted in NVS, **When** an OTA update completes, **Then** the persisted tree data is restored on boot.
4. **Given** a successful OTA write, **When** the device reboots into the new firmware, **Then** the new firmware marks itself as valid (cancels rollback) after successful initialization.

---

### User Story 2 - Reject Unauthorized OTA Requests (Priority: P1)

An unauthenticated or wrongly-authenticated client attempts to upload firmware. The device rejects the request before reading the firmware payload, preventing unauthorized firmware replacement.

**Why this priority**: OTA is the highest-privilege operation — it replaces the entire firmware. Auth must be enforced before any data is accepted to prevent both unauthorized updates and resource exhaustion from unauthenticated large uploads.

**Independent Test**: Can be tested by POSTing to `/ota` without a token or with an invalid token and verifying a 401 response with no side effects.

**Acceptance Scenarios**:

1. **Given** a POST to `/ota` with no `Authorization` header, **When** the request arrives, **Then** the device returns HTTP 401 without reading the body.
2. **Given** a POST to `/ota` with an invalid bearer token, **When** the request arrives, **Then** the device returns HTTP 401 without reading the body.
3. **Given** a POST to `/ota` with a valid bearer token, **When** the request arrives, **Then** the device proceeds to read and process the firmware payload.
4. **Given** an unauthorized OTA attempt, **When** the device responds 401, **Then** the active firmware and inactive OTA slot remain unchanged.

---

### User Story 3 - Reject Invalid Firmware Images (Priority: P1)

A client uploads a corrupted, truncated, or non-firmware file to `/ota`. The device detects the problem and rejects the update, remaining on its current firmware without rebooting.

**Why this priority**: A bad OTA image that gets written and booted would brick the device. Validation before committing the update is critical for reliability.

**Independent Test**: Can be tested by POSTing random bytes, a truncated image, or an uncompressed image (when gzip expected) to `/ota` and verifying the device rejects it and remains on the current firmware.

**Acceptance Scenarios**:

1. **Given** a POST to `/ota` with random (non-firmware) data, **When** the device attempts to decompress and validate, **Then** it returns an error response and does not reboot.
2. **Given** a POST to `/ota` with a truncated gzip stream, **When** decompression fails mid-stream, **Then** the device aborts the OTA write, returns an error, and remains on current firmware.
3. **Given** a POST to `/ota` with uncompressed firmware (raw binary, not gzip), **When** the device checks for gzip header bytes, **Then** it returns HTTP 400 indicating gzip compression is required.
4. **Given** a POST to `/ota` with valid gzip but containing non-firmware data, **When** ESP-IDF OTA validation runs, **Then** `esp_ota_end()` fails validation, the device returns an error, and does not reboot.
5. **Given** a valid OTA upload is in progress, **When** a second `/ota` request arrives, **Then** the second request is rejected with HTTP 409 Conflict (only one OTA at a time).

---

### User Story 4 - Automatic Rollback on Bad Firmware (Priority: P2)

A device receives and boots into new firmware that fails to initialize properly (e.g., crashes during startup, fails WiFi, fails to start HTTP server). The ESP-IDF rollback mechanism detects that the new firmware never marked itself as valid, and the watchdog timer reboots the device back to the previous working firmware.

**Why this priority**: Rollback is the safety net. Without it, a buggy OTA update bricks the device. However, ESP-IDF provides most of this mechanism — the implementation work is calling `esp_ota_mark_app_valid_cancel_rollback()` at the right time.

**Independent Test**: Difficult to test in automated e2e (would require deliberately uploading broken firmware). Can be verified by inspecting the boot log for rollback state and confirming `esp_ota_mark_app_valid_cancel_rollback()` is called after successful init.

**Acceptance Scenarios**:

1. **Given** new firmware boots successfully (WiFi connects, HTTP server starts, OMI engine initializes), **When** all init steps complete, **Then** the firmware calls `esp_ota_mark_app_valid_cancel_rollback()` to confirm itself as valid.
2. **Given** new firmware is booted but crashes before marking itself valid, **When** the watchdog timer triggers a reboot, **Then** ESP-IDF boots the previous OTA slot with the last known-good firmware.
3. **Given** a rollback has occurred, **When** the device is queried, **Then** it reports the previous firmware version and the rollback event is logged.

---

### User Story 5 - OTA Progress and Version Reporting (Priority: P3)

A device operator can check the current firmware version via the O-DF tree, and receives progress feedback during an OTA upload. This enables tooling to verify that an update was applied and to show upload progress.

**Why this priority**: Version reporting is essential for fleet management but is not blocking for the OTA mechanism itself. Progress feedback improves UX but is not strictly necessary.

**Independent Test**: Can be tested by reading `/System/FirmwareVersion` before and after an OTA update and verifying the version changes.

**Acceptance Scenarios**:

1. **Given** a device running firmware, **When** a client reads `/System/FirmwareVersion`, **Then** it returns the current firmware version string (from build-time `CARGO_PKG_VERSION` or git describe).
2. **Given** an OTA upload completes successfully, **When** the device reboots, **Then** `/System/FirmwareVersion` reflects the new version.
3. **Given** an OTA upload is in progress, **When** the `/ota` response is sent after completion, **Then** it includes the number of bytes written and a success/error status.

---

### Edge Cases

- What happens if the device loses power during an OTA write? The inactive slot is corrupted but the active slot is untouched. Next boot runs current firmware. The incomplete OTA slot will be overwritten by the next OTA attempt.
- What happens if the gzip-decompressed image exceeds the OTA partition size? `esp_ota_write()` returns an error when the partition is full. The device aborts and returns an error.
- What happens if OTA is attempted while subscriptions are active? Subscriptions continue during upload. After reboot, WebSocket subscriptions are lost (connection closes). HTTP callback subscriptions with TTL survive if their state is in NVS.
- What happens if the firmware binary is the same version as currently running? The update proceeds normally — no version comparison is enforced. The operator decides whether to re-flash.
- What happens if the device has no configured API key (auth disabled)? OTA endpoint requires authentication and rejects requests when no API key is configured. OTA with no auth is never allowed.
- What happens when Content-Length header is missing? The device reads the body in chunks until the connection closes or the upload timeout expires. Content-Length is recommended but not required.
- What happens if Content-Length is present and too large? The endpoint rejects immediately with HTTP 400 before reading the body — cheap early validation that avoids wasting time on obviously-too-large uploads.
- What happens during OTA on heap exhaustion? Streaming decompression processes small chunks (4 KB read buffer → decompress → write to flash). Peak heap usage should not exceed ~20 KB for OTA buffers (4 KB read + 8 KB decompress output + overhead).
- What happens if the upload stalls or the client is very slow? A 5-minute upload timeout aborts the OTA and releases the lock. The inactive slot may be partially written but is not booted.
- What happens if the OTA lock is stuck (e.g., previous handler crashed)? The lock is an `AtomicBool` — if the handler exits (normally or via panic), the lock is released via a drop guard. No persistent lock state survives a reboot.

---

## Requirements *(mandatory)*

### Functional Requirements

#### Partition Layout

- **FR-001**: Project MUST use a custom `partitions.csv` with two OTA app slots (`ota_0`, `ota_1`) and no factory partition. NVS and any data partitions MUST be preserved.
- **FR-002**: Each OTA app slot MUST be large enough to hold the maximum expected firmware image. With 4 MB flash, each slot should be approximately 1.5-1.8 MB (exact sizes determined by partition table layout after accounting for NVS, otadata, and PHY partitions).
- **FR-003**: The `otadata` partition MUST be included for ESP-IDF OTA boot selection.
- **FR-004**: The `sdkconfig.defaults` MUST be updated to reference the custom partition table and enable OTA-related config options.

#### OTA Endpoint

- **FR-005**: System MUST expose a `POST /ota` HTTP endpoint that accepts a gzip-compressed firmware image as the request body.
- **FR-006**: The `/ota` endpoint MUST require bearer token authentication. The token MUST be validated before reading any request body data.
- **FR-007**: If authentication fails, the endpoint MUST return HTTP 401 and MUST NOT read the request body or modify any OTA partition.
- **FR-008**: The endpoint MUST reject concurrent OTA requests with HTTP 409 Conflict.
- **FR-008b**: The `/ota` URI MUST only accept the POST method. Requests with any other method (GET, PUT, etc.) MUST receive HTTP 405 Method Not Allowed.
- **FR-008c**: If a `Content-Length` header is present and its value exceeds the OTA partition size (0x1E0000 = 1,966,080 bytes), the endpoint MUST reject the request with HTTP 400 before reading the body.
- **FR-009**: The endpoint MUST validate the gzip magic bytes (`0x1f 0x8b`) in the first two bytes of the body. If not present, return HTTP 400 with a message indicating gzip compression is required.
- **FR-010a**: The endpoint MUST enforce a 5-minute (300s) upload timeout. The timeout MUST be implemented by tracking wall-clock elapsed time in the read loop using `esp_idf_svc::sys::esp_timer_get_time()`. If the upload is not complete within this window, the OTA write MUST be aborted and the lock released.
- **FR-010b**: The OTA lock MUST be implemented as an `AtomicBool` with a RAII drop guard that releases the lock when the handler exits (normally or on panic).

#### Streaming Decompression and Flash Write

- **FR-011**: The system MUST decompress the gzip stream incrementally (streaming), not buffer the entire decompressed image in memory.
- **FR-012**: The system MUST read the HTTP body in chunks using `req.read(&mut buf)` (which calls ESP-IDF `httpd_req_recv()` underneath). Recommended chunk size: 4 KB read buffer, 8 KB decompression output buffer.
- **FR-013**: The system MUST use `miniz_oxide::inflate::stream::InflateState` for streaming DEFLATE decompression. The existing `gzip_decompress()` in `compress.rs` buffers the entire output and MUST NOT be used for OTA. A new `GzipStreamDecompressor` struct is required, encapsulating the state machine (header parsing → DEFLATE inflate → trailer verification) with a `fn feed(&mut self, input: &[u8]) -> Result<&[u8], Error>` interface that accepts input chunks and returns decompressed output.
- **FR-014**: The streaming decompressor MUST parse and strip the gzip header (10+ bytes), feed the DEFLATE payload to `InflateState`, and after the DEFLATE stream ends, verify the gzip trailer (CRC32 + ISIZE) against the decompressed data. The decompressor MUST compute a running CRC32 over all decompressed output bytes and verify it against the gzip trailer CRC32 field.
- **FR-015**: If decompression fails at any point (invalid gzip, CRC mismatch, truncated stream), the system MUST abort the OTA write via `esp_ota_abort()`, return an error response, and NOT reboot.
- **FR-016**: If `esp_ota_write()` returns an error (e.g., partition full), the system MUST abort, return an error, and NOT reboot.
- **FR-017**: On successful completion, the system MUST call `esp_ota_end()` to validate the image (ESP-IDF checks the app image header and hash). If validation fails, return an error and do NOT reboot.
- **FR-018**: On successful validation, the system MUST call `esp_ota_set_boot_partition()` to set the new slot as boot target, send a success response, and reboot after a short delay (500ms via `FreeRTOS::delay_ms()` to allow the HTTP response to flush).

#### Response Format

- **FR-019**: The `/ota` endpoint MUST return plain JSON responses (NOT OMI envelopes). Format:
  - Success: `{"status": "ok", "bytes_written": <n>}` with HTTP 200
  - Auth failure: `{"status": "error", "message": "unauthorized"}` with HTTP 401
  - Bad request: `{"status": "error", "message": "<reason>"}` with HTTP 400
  - Conflict: `{"status": "error", "message": "OTA already in progress"}` with HTTP 409
  - Method not allowed: `{"status": "error", "message": "method not allowed"}` with HTTP 405
  - Internal error: `{"status": "error", "message": "<reason>"}` with HTTP 500

#### Rollback Protection

- **FR-020**: On every boot, after successful initialization (WiFi connected, HTTP server started, OMI engine ready), the firmware MUST call `esp_ota_mark_app_valid_cancel_rollback()` to confirm the running firmware as valid.
- **FR-021**: If the firmware crashes or fails to initialize before calling the validation function, ESP-IDF's built-in rollback mechanism MUST reboot into the previously valid OTA slot.
- **FR-022**: The `sdkconfig.defaults` MUST enable `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y`.

#### Version Reporting

- **FR-023**: The system MUST expose a read-only InfoItem at `/System/FirmwareVersion` containing the firmware version string, set at build time.
- **FR-024**: The version string MUST default to `CARGO_PKG_VERSION` but MUST be overridable via a `FIRMWARE_VERSION` environment variable. `build.rs` injects this as a compile-time constant.

#### Binary Production

- **FR-025**: The OTA-compatible binary MUST be produced using `espflash save-image --chip esp32s2 --format esp-idf <ELF> <output.bin>`. This produces the app-only image with ESP-IDF app header (no bootloader, no partition table).
- **FR-026**: The binary MUST be gzip-compressed before upload: `gzip -c <output.bin> > <output.bin.gz>`.

#### Preservation

- **FR-027**: OTA updates MUST NOT erase or modify the NVS partition. WiFi configuration, API key hash, and persisted O-DF tree data MUST survive across OTA updates.

### Key Entities

- **OTA Slot**: One of two app partitions (`ota_0`, `ota_1`). At any time, one is "active" (running) and one is "inactive" (target for next update).
- **otadata Partition**: Small partition that tracks which OTA slot to boot and its validation state.
- **Firmware Image**: The ELF binary produced by `cargo build`, converted to an app-only `.bin` by `espflash save-image --chip esp32s2 --format esp-idf`. Gzip-compressed before upload.
- **Rollback State**: ESP-IDF state machine tracking whether newly booted firmware has confirmed itself valid. States: `ESP_OTA_IMG_NEW`, `ESP_OTA_IMG_PENDING_VERIFY`, `ESP_OTA_IMG_VALID`.

---

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: An authenticated OTA upload of a gzip-compressed firmware image completes successfully, the device reboots, and reports the new version — end to end in under 60 seconds (for a typical ~1.2 MB compressed image over LAN).
- **SC-002**: An unauthenticated OTA attempt is rejected with HTTP 401 in under 100ms with no side effects on flash.
- **SC-003**: A corrupted or truncated firmware upload is rejected without rebooting, and the device remains on its current firmware.
- **SC-004**: WiFi credentials and API key survive an OTA update — the device reconnects to the same network with the same API key after reboot.
- **SC-005**: Peak heap usage during OTA does not exceed 20 KB above baseline (streaming decompression keeps memory bounded).
- **SC-006**: The memory budget test (`cargo test-host --test memory_budget`) passes with the new partition layout — firmware fits within the OTA slot size.
- **SC-007**: An e2e test automatically builds two firmware versions, uploads one via OTA, and verifies the version change and data preservation.

---

## Assumptions

- Flash size is 4 MB. The partition table must fit two app images plus NVS, otadata, and PHY init data within this budget.
- ESP-IDF v5.3.3 (current) supports `esp_ota_ops.h` and app rollback. No additional IDF components needed.
- The `miniz_oxide` crate (already in `Cargo.toml`, v0.8) supports streaming DEFLATE decompression via `inflate::stream::InflateState`. The existing `gzip_decompress()` in `compress.rs` buffers the entire output — a new streaming variant is needed for OTA.
- `esp-idf-svc` v0.51 request objects implement `std::io::Read`. `req.read(&mut buf)` calls `httpd_req_recv()` underneath, enabling chunked body reads in a loop. Confirmed by source inspection.
- `espflash save-image --chip esp32s2 --format esp-idf` produces an app-only binary (with ESP-IDF app header, no bootloader or partition table). This is what `esp_ota_write()` expects. Confirmed.
- The e2e test infrastructure (`run-e2e.sh`, pytest, device locking) is sufficient to test OTA. The test builds two firmware variants (differing by `FIRMWARE_VERSION` env var) and uses `requests` to POST the compressed binary.
- The HTTP server's maximum body size limit (currently 16 KB for `/omi`, 64 KB for `/`) does NOT apply to `/ota` — the OTA handler reads the body in a custom streaming loop without a global size cap.
- First-time migration: devices currently using the default single-app (factory) partition table will need to be flashed once via USB with the new OTA partition layout. After that, all subsequent updates can be OTA. No backward compatibility with the old partition layout is required (product has not shipped yet).

---

## Partition Table Design

```
# Name,    Type,  SubType,  Offset,   Size,     Flags
otadata,   data,  ota,      0xd000,   0x2000,
phy_init,  data,  phy,      0xf000,   0x1000,
nvs,       data,  nvs,      0x10000,  0x6000,
ota_0,     app,   ota_0,    0x20000,  0x1E0000,
ota_1,     app,   ota_1,    0x200000, 0x1E0000,
storage,   data,  fat,      0x3E0000, 0x20000,
```

**Layout rationale** (4 MB = 0x400000 total):
- `otadata` (8 KB): OTA boot selection metadata — must exist for two-slot OTA.
- `phy_init` (4 KB): WiFi PHY calibration data.
- `nvs` (24 KB): Non-volatile storage for WiFi config, API key hash, O-DF tree.
- `ota_0` (1920 KB): First app slot. Sized to fit current firmware (~1913 KB) with margin.
- `ota_1` (1920 KB): Second app slot. Identical size.
- `storage` (128 KB): General data partition (pages, future use).

Total: 8 + 4 + 24 + 1920 + 1920 + 128 = 4004 KB (fits in 4 MB with partition table and bootloader overhead at 0x0000-0xCFFF).

The `memory_budget.rs` test MUST be updated: `FLASH_LIMIT` changes from 2 MB to 1920 KB (0x1E0000 = 1,966,080 bytes).

---

## E2E Test Design

### Test: `test_ota.py`

**Prerequisites**: Device flashed with firmware version "A" (the standard e2e build). A second firmware binary with version "B" is built and gzip-compressed before the test runs. The `run-e2e.sh` script prepares both.

**Test flow**:

1. **Read current version**: OMI read of `/System/FirmwareVersion` → expect version "A" (the default `CARGO_PKG_VERSION`).
2. **Reject unauthenticated OTA**: `POST /ota` with no token → expect HTTP 401, body `{"status": "error", "message": "unauthorized"}`.
3. **Reject invalid token OTA**: `POST /ota` with `Authorization: Bearer wrong` → expect HTTP 401.
4. **Reject non-gzip payload**: `POST /ota` with valid token, body = `b"not firmware"` → expect HTTP 400 with message about gzip.
5. **Successful OTA upload**: `POST /ota` with valid token, body = gzip-compressed firmware "B", `Content-Type: application/octet-stream` → expect HTTP 200 with `{"status": "ok", "bytes_written": <n>}`.
6. **Wait for reboot**: Use `wait_for_device()` helper (up to 30s).
7. **Verify new version**: Read `/System/FirmwareVersion` → expect version "B" (`"e2e-ota-test"`).
8. **Verify data preservation**: Device is online (WiFi survived), make an authenticated OMI read (API key survived).
9. **Restore original firmware**: OTA-upload the original firmware "A" (also gzip-compressed) to leave device in original state for subsequent tests. Wait for reboot again.
10. **Verify restoration**: Read `/System/FirmwareVersion` → expect version "A" again.

**Note**: Concurrent OTA rejection (FR-008, HTTP 409) is timing-dependent and NOT tested in e2e. It MUST be covered by a host unit test instead.

### Build script changes (`run-e2e.sh`):

```bash
# --- After building and flashing the primary firmware (version "A") ---

# Build version "B" for OTA test
FIRMWARE_A_BIN="$PROJECT_ROOT/target/xtensa-esp32s2-espidf/debug/firmware-a.bin"
FIRMWARE_B_BIN="$PROJECT_ROOT/target/xtensa-esp32s2-espidf/debug/firmware-b.bin"

# Save version "A" as OTA binary (for restore step)
espflash save-image --chip esp32s2 --format esp-idf "$FIRMWARE" "$FIRMWARE_A_BIN"
gzip -c "$FIRMWARE_A_BIN" > "$FIRMWARE_A_BIN.gz"

# Rebuild with different version for "B"
FIRMWARE_VERSION="e2e-ota-test" cargo build --no-default-features \
    --features std,esp,lite-json,scripting,mem-stats

espflash save-image --chip esp32s2 --format esp-idf "$FIRMWARE" "$FIRMWARE_B_BIN"
gzip -c "$FIRMWARE_B_BIN" > "$FIRMWARE_B_BIN.gz"

# Rebuild original version "A" to leave the build dir clean
# Explicitly unset to ensure cargo detects the env change via rerun-if-env-changed
unset FIRMWARE_VERSION
cargo build --no-default-features --features std,esp,lite-json,scripting,mem-stats

export OTA_FIRMWARE_A_GZ="$FIRMWARE_A_BIN.gz"
export OTA_FIRMWARE_B_GZ="$FIRMWARE_B_BIN.gz"
```

The e2e test reads `OTA_FIRMWARE_A_GZ` and `OTA_FIRMWARE_B_GZ` env vars.

### Conftest additions (`conftest.py`):

```python
@pytest.fixture(scope="session")
def ota_firmware_a_gz():
    """Path to gzip-compressed firmware version A (for restore)."""
    path = os.environ.get("OTA_FIRMWARE_A_GZ")
    if not path:
        pytest.skip("OTA_FIRMWARE_A_GZ not set")
    return path

@pytest.fixture(scope="session")
def ota_firmware_b_gz():
    """Path to gzip-compressed firmware version B (for OTA test)."""
    path = os.environ.get("OTA_FIRMWARE_B_GZ")
    if not path:
        pytest.skip("OTA_FIRMWARE_B_GZ not set")
    return path
```

### Helper additions (`helpers.py`):

```python
OTA_TIMEOUT = 120  # seconds — generous for ~1.2 MB compressed over LAN

def ota_upload(base_url, firmware_gz_path, token):
    """Upload gzip-compressed firmware via POST /ota."""
    with open(firmware_gz_path, "rb") as f:
        data = f.read()
    headers = {
        "Authorization": f"Bearer {token}",
        "Content-Type": "application/octet-stream",
    }
    return requests.post(
        f"{base_url}/ota", data=data, headers=headers, timeout=OTA_TIMEOUT,
    )
```

### `build.rs` changes:

```rust
// In build.rs, emit the version constant:
let version = std::env::var("FIRMWARE_VERSION")
    .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap());
println!("cargo:rustc-env=FIRMWARE_VERSION={version}");
println!("cargo:rerun-if-env-changed=FIRMWARE_VERSION");
```

Then in source code: `const FIRMWARE_VERSION: &str = env!("FIRMWARE_VERSION");`
