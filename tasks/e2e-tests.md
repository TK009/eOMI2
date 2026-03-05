# End-to-End Test Plan

E2E tests exercise the **real device** over USB and Wi-Fi.
They complement host-only integration tests (`cargo test-host`) by verifying
hardware-dependent behavior: flashing, boot, Wi-Fi, HTTP/WS transport, NVS
persistence, sensors, and memory constraints under real conditions.

Run e2e tests **after** all host tests pass, as stated in CLAUDE.md.

---

## USB Device Pool

Multiple developers or CI runners may share a set of ESP32 boards.
A lockfile-based mechanism prevents two processes from using the same
USB device simultaneously. See `scripts/claim-device.sh` and
`scripts/release-device.sh`.

Lockfiles are stored in `<project-root>/.device-locks/`. The project root
is resolved via `git rev-parse --git-common-dir` so it works from worktrees
and any checkout location.

### Usage pattern

Every script and test runner that touches a USB device **must** use this:

```bash
eval "$(./scripts/claim-device.sh)"
trap './scripts/release-device.sh "$DEVICE_LOCK"' EXIT

espflash flash --monitor --port "$DEVICE_PORT" target/xtensa-esp32s2-espidf/debug/reconfigurable-device
```

CI jobs should each call `claim-device.sh` at the start of the job and
`release-device.sh` in the cleanup step.

---

## Test Environment

### Prerequisites

- ESP32-S2 board connected via USB
- Board on the same Wi-Fi network as the test runner
- `.env` file with `WIFI_SSID`, `WIFI_PASS`, `API_TOKEN` configured
- Firmware built: `cargo build`
- `espflash` installed
- `curl` or a test HTTP client (Python `requests`, or a Rust test binary)

### Environment variables for tests

| Variable | Source | Purpose |
|----------|--------|---------|
| `DEVICE_PORT` | `claim-device.sh` | USB serial port for flashing and serial monitor |
| `DEVICE_IP` | Serial log or mDNS | Device IP on Wi-Fi for HTTP/WS requests |
| `API_TOKEN` | `.env` | Bearer token for mutating operations |

### Device IP discovery

After flashing and boot, the device prints its IP to the serial console:

```
I (xxxx) wifi: Got IP: 192.168.x.x
```

The test harness should:
1. Flash the firmware via `espflash`
2. Open the serial port and wait for the IP log line (timeout: 30s)
3. Export the IP for subsequent test steps

---

## Test Harness: `scripts/run-e2e.sh`

Orchestrator script that:

1. Claims a USB device
2. Builds firmware (unless `--skip-build`)
3. Flashes the device
4. Reads serial output to discover IP
5. Waits for the HTTP server to respond (health check: `GET /`)
6. Runs the test suite
7. Reports results
8. Releases the device

```bash
#!/usr/bin/env bash
set -euo pipefail

eval "$(./scripts/claim-device.sh)"
trap './scripts/release-device.sh "$DEVICE_LOCK"' EXIT

# 1. Build
if [[ "${1:-}" != "--skip-build" ]]; then
    cargo build
fi

# 2. Flash (background, captures serial)
SERIAL_LOG=$(mktemp)
espflash flash --port "$DEVICE_PORT" \
    target/xtensa-esp32s2-espidf/debug/reconfigurable-device
espflash serial-monitor --port "$DEVICE_PORT" > "$SERIAL_LOG" 2>&1 &
MONITOR_PID=$!
trap 'kill $MONITOR_PID 2>/dev/null; ./scripts/release-device.sh "$DEVICE_LOCK"; rm -f "$SERIAL_LOG"' EXIT

# 3. Discover IP (wait up to 30s)
DEVICE_IP=""
for i in $(seq 1 30); do
    DEVICE_IP=$(grep -oP 'Got IP: \K[0-9.]+' "$SERIAL_LOG" 2>/dev/null || true)
    [ -n "$DEVICE_IP" ] && break
    sleep 1
done
[ -z "$DEVICE_IP" ] && { echo "FAIL: device did not get IP within 30s"; exit 1; }

export DEVICE_IP
export DEVICE_PORT
export API_TOKEN  # from .env or caller

# 4. Health check — wait for HTTP server
for i in $(seq 1 15); do
    curl -sf "http://$DEVICE_IP/" >/dev/null 2>&1 && break
    sleep 1
done

echo "Device ready at $DEVICE_IP (port $DEVICE_PORT)"

# 5. Run tests
# (see Test Suite section — can be pytest, bash, or a Rust test binary)
python3 tests/e2e/run_all.py --base-url "http://$DEVICE_IP" --token "$API_TOKEN"
EXIT_CODE=$?

echo "--- Serial log tail ---"
tail -30 "$SERIAL_LOG"

exit $EXIT_CODE
```

---

## Test Suite Structure

```
tests/e2e/
  run_all.py            # Test runner (pytest or plain script)
  conftest.py           # Shared fixtures: base_url, token, ws_connect
  test_boot.py          # Boot and connectivity checks
  test_http_read.py     # OMI read operations over HTTP
  test_http_write.py    # OMI write operations over HTTP
  test_http_delete.py   # OMI delete operations over HTTP
  test_rest_discovery.py# GET /omi/* REST API
  test_websocket.py     # WebSocket operations and subscriptions
  test_subscriptions.py # Poll, interval, event subscription delivery
  test_pages.py         # PATCH/GET/DELETE stored HTML pages
  test_auth.py          # Authentication enforcement
  test_persistence.py   # NVS persistence across reboots (needs reflash)
  test_sensors.py       # DHT11 sensor readings
  test_scripting.py     # Onwrite script execution over network
  test_stress.py        # Concurrent connections, rapid requests
```

Python with `requests` + `websockets` is recommended for e2e: fast to write,
easy to read, no compilation, works on any CI runner. Alternatively a Rust
binary in `tests/e2e/` could be used but the iteration speed is worse.

---

## Test Specifications

### 1. Boot and Connectivity (`test_boot.py`)

Verifies the device boots, connects to Wi-Fi, and serves HTTP.

| Test | Steps | Expected |
|------|-------|----------|
| `test_device_boots` | Flash → wait for serial "Got IP" | IP line appears within 30s |
| `test_landing_page` | `GET /` | 200, HTML body, `Content-Type: text/html` |
| `test_omi_endpoint_reachable` | `POST /omi` with read root | 200, JSON with `"omi":"1.0"` |

### 2. HTTP Read Operations (`test_http_read.py`)

| Test | Steps | Expected |
|------|-------|----------|
| `test_read_root` | POST read `/` | 200, response contains `Dht11` object |
| `test_read_sensor_object` | POST read `/Dht11` | 200, `Temperature` and `RelativeHumidity` items |
| `test_read_sensor_value` | Wait 6s (sensor poll), POST read `/Dht11/Temperature` | 200, at least 1 value |
| `test_read_newest` | Read `/Dht11/Temperature?newest=1` | At most 1 value returned |
| `test_read_nonexistent` | POST read `/NoSuch` | 404 status in response body |
| `test_read_with_depth` | POST read `/` with `depth=1` | Objects listed, items not expanded |

### 3. HTTP Write Operations (`test_http_write.py`)

| Test | Steps | Expected |
|------|-------|----------|
| `test_write_new_item` | POST write `/Test/Value` with `42` | 200/201, readback returns `42` |
| `test_write_string_value` | Write string `"hello"` | Readback returns `"hello"` |
| `test_write_bool_value` | Write `true` | Readback returns `true` |
| `test_write_overwrite` | Write `1`, then write `2` | Readback latest is `2` |
| `test_write_sensor_rejected` | Write to `/Dht11/Temperature` (read-only) | 403 |
| `test_write_batch` | Write to 3 paths in one request | All 3 readable |
| `test_write_tree_merge` | Write an object subtree | Merges into tree, readable |

### 4. HTTP Delete Operations (`test_http_delete.py`)

| Test | Steps | Expected |
|------|-------|----------|
| `test_delete_user_item` | Write `/Test/X`, delete `/Test` | 200, subsequent read returns 404 |
| `test_delete_root_forbidden` | Delete `/` | 403 |
| `test_delete_nonexistent` | Delete `/Ghost` | 404 |

### 5. REST Discovery (`test_rest_discovery.py`)

| Test | Steps | Expected |
|------|-------|----------|
| `test_get_omi_root` | `GET /omi/` | 200, JSON lists `Dht11` |
| `test_get_omi_object` | `GET /omi/Dht11/` | 200, lists `Temperature`, `RelativeHumidity` |
| `test_get_omi_item` | `GET /omi/Dht11/Temperature` | 200, item with values |
| `test_get_omi_query_newest` | `GET /omi/Dht11/Temperature?newest=2` | At most 2 values |

### 6. WebSocket (`test_websocket.py`)

| Test | Steps | Expected |
|------|-------|----------|
| `test_ws_connect` | Open WS to `/omi/ws` | Connection succeeds |
| `test_ws_read` | Send read message over WS | Receive response with tree data |
| `test_ws_event_sub` | Create event sub on WS → write via HTTP → read WS | Sub update pushed over WS |
| `test_ws_close_cancels_subs` | Create sub → close WS → write → reopen WS | No stale deliveries |
| `test_ws_multiple_concurrent` | Open 3 WS connections, each subscribes | All receive independent updates |

### 7. Subscriptions (`test_subscriptions.py`)

| Test | Steps | Expected |
|------|-------|----------|
| `test_poll_sub_lifecycle` | Create poll sub → write value → poll by rid | Value delivered, buffer drained |
| `test_poll_sub_expiry` | Create sub with `ttl=5` → wait 6s → poll | 404 (expired) |
| `test_event_sub_on_ws` | Event sub over WS → write → check push | Update received on WS |
| `test_interval_sub` | Interval sub (10s) over WS → wait 12s | At least 1 push received |
| `test_cancel_sub` | Create sub → cancel by rid → write | No delivery after cancel |

### 8. Stored Pages (`test_pages.py`)

| Test | Steps | Expected |
|------|-------|----------|
| `test_store_page` | `PATCH /mypage` with HTML body | 200 |
| `test_retrieve_page` | `GET /mypage` after store | 200, body matches stored content |
| `test_landing_lists_page` | Store page → `GET /` | Landing page HTML contains link to `/mypage` |
| `test_delete_page` | `DELETE /mypage` | 200, subsequent GET returns 404 |
| `test_store_requires_auth` | `PATCH /mypage` without token | 401/403 |

### 9. Authentication (`test_auth.py`)

| Test | Steps | Expected |
|------|-------|----------|
| `test_read_no_auth` | POST read without token | 200 (reads are public) |
| `test_write_no_auth` | POST write without token | 401/403 |
| `test_write_wrong_token` | POST write with wrong token | 401/403 |
| `test_write_correct_token` | POST write with correct token | 200 |
| `test_delete_no_auth` | DELETE without token | 401/403 |
| `test_rest_get_no_auth` | `GET /omi/Dht11` | 200 (GETs are public) |

### 10. NVS Persistence (`test_persistence.py`)

Requires reflashing/rebooting the device mid-test.

| Test | Steps | Expected |
|------|-------|----------|
| `test_user_data_survives_reboot` | Write `/Persist/Key` = `"saved"` → reboot device (reset via serial DTR or reflash) → wait for boot → read `/Persist/Key` | Value `"saved"` present |
| `test_sensor_tree_rebuilt` | Reboot → read `/Dht11` | Sensor items present (rebuilt from code) |

Reboot mechanism: toggle DTR/RTS on the serial port, or use `espflash` to
reflash the same binary. The harness should expose a `reboot_device()` helper.

### 11. Sensor Readings (`test_sensors.py`)

| Test | Steps | Expected |
|------|-------|----------|
| `test_temperature_in_range` | Wait 6s, read `/Dht11/Temperature` | Value is a number between -10 and 60 °C |
| `test_humidity_in_range` | Wait 6s, read `/Dht11/RelativeHumidity` | Value is a number between 0 and 100 %RH |
| `test_values_update` | Read, wait 10s, read again | Timestamps differ (new reading taken) |

Note: these tests assume a DHT11 sensor is physically connected.
Skip gracefully if sensor returns errors (CI boards may not have one).

### 12. Scripting (`test_scripting.py`)

| Test | Steps | Expected |
|------|-------|----------|
| `test_onwrite_cascade` | Write item with `onwrite` script that copies to another path → write value → read target path | Target path has the converted value |
| `test_script_error_no_crash` | Write item with broken script → write value | Original write succeeds, device still responsive |

### 13. Stress and Stability (`test_stress.py`)

| Test | Steps | Expected |
|------|-------|----------|
| `test_rapid_writes` | 100 writes in quick succession | All succeed or return well-formed errors, device stays responsive |
| `test_concurrent_connections` | 5 simultaneous HTTP POST requests | All get valid responses |
| `test_large_payload` | Write a 2 KB JSON tree | Succeeds or returns 413/400, no crash |
| `test_long_running` | Write + read loop for 2 minutes | No OOM, no hang, responses stay consistent |

---

## Priority Order

1. **Boot + connectivity** — gate for all other tests
2. **HTTP read/write/delete** — core CRUD functionality
3. **Authentication** — security boundary
4. **REST discovery** — public API
5. **WebSocket** — persistent connections
6. **Subscriptions** — stateful multi-step
7. **Stored pages** — secondary feature
8. **Persistence** — requires reboot, slower
9. **Sensors** — hardware-dependent, may need skip
10. **Scripting** — later feature
11. **Stress** — longest running, run last or nightly

---

## CI Integration

```
Host tests (every commit):
  cargo test-host

E2E tests (nightly or pre-release, on a runner with USB device):
  ./scripts/run-e2e.sh
```

- CI runner needs physical ESP32-S2 boards connected via USB
- Each parallel CI job calls `claim-device.sh` independently
- Tests that need sensors should be tagged (e.g. `@pytest.mark.sensor`) and
  skippable via `--skip-sensor` for boards without a DHT11
- Persistence tests that reboot the device should be tagged `@pytest.mark.reboot`
  and run last (they disrupt other tests if interleaved)
- Set a global timeout of 5 minutes for the full e2e suite

---

## Serial Log Assertions

Some conditions can only be verified through serial output. The harness should
capture serial logs throughout and support assertions like:

- `assert_serial_contains("Got IP:")` — confirms Wi-Fi connected
- `assert_serial_not_contains("panic")` — no crashes
- `assert_serial_not_contains("stack overflow")` — no stack issues
- `assert_serial_contains("NVS loaded")` — persistence restored on boot

After each test run, dump the last N lines of serial log for debugging failures.
