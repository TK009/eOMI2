# Architecture Review: Hyper-Text IoT Device

**Date:** 2026-03-03

## Executive Summary

This is a well-architected embedded project. The core logic is clean, memory-conscious, and well-tested. The issues below are ordered by severity — most are refinements, not fundamental flaws.

---

## What's Done Well

**Platform separation** — Platform code (main.rs, server.rs, nvs.rs) is cleanly isolated from portable logic (odf/, omi/, http.rs). This is textbook embedded architecture and enables the excellent host-test story.

**Memory discipline** — Fixed-capacity ring buffers, PSRAM encapsulation, bounded everything (subscriptions=32, pages=100KB, bodies=16KB/64KB, scripts=4KB, nesting=8). No heap fragmentation risk in steady state.

**Error modeling** — Five distinct error types (TreeError, ParseError, ScriptError, PageError, BodyError) with proper Display/Error impls and mapping to HTTP status codes. Graceful degradation throughout.

**Thread safety** — Arc<Mutex<>> with documented lock ordering (Engine before WsSenders). Poisoned-mutex recovery via `unwrap_or_else(|e| e.into_inner())`. AtomicBool for NVS dirty flag, AtomicU32 for session IDs.

**Input validation** — Path traversal prevention, depth guards before deserialization, constant-time auth comparison, XSS escaping, null-byte stripping, content-type checks.

**Testing** — ~275 unit tests + ~500 integration tests + full E2E suite. Tests are behavior-driven, well-isolated, and cover real edge cases (ring overflow, TTL expiry, batch mixed results, cascading script depth).

---

## Issues Found

### 1. No Timeout/Watchdog for Script Execution (Severity: HIGH)

mJS scripts run synchronously during write operations (`engine.rs:361`). A script with an infinite loop will deadlock the engine mutex and freeze the device.

**Best practice:** All external/user-supplied code execution needs a timeout. Options:
- mJS has a `MJS_EXEC_TIMEOUT` flag — enable it
- Run scripts in a FreeRTOS task with a watchdog
- At minimum, document the risk and add a `MAX_SCRIPT_EXEC_MS` constant

**Impact:** A single bad script makes the device unrecoverable without power cycle.

---

### 2. Platform Code Has Zero Unit Tests (Severity: HIGH)

| File | Lines | Unit tests |
|------|-------|------------|
| server.rs | ~400 | 0 |
| main.rs | ~170 | 0 |
| nvs.rs | ~100 | 0 |

These are only covered by E2E tests on real hardware. Error paths in `read_body()`, `check_auth()`, NVS corruption recovery, and WiFi reconnect are untested.

**Best practice:** Extract testable logic behind traits. For example:
- `read_body()` parsing logic can be tested with byte slices
- Auth checking can be tested without an HTTP server
- NVS serialization/deserialization can be tested with mock storage

---

### 3. Mutex Poisoning Strategy Is Risky (Severity: MEDIUM)

```rust
engine.lock().unwrap_or_else(|e| e.into_inner())
```

This recovers from poisoned mutexes by accessing the inner data. If the panic that poisoned the mutex left the data in an inconsistent state (partial write, half-updated subscription), the device continues operating on corrupt data.

**Best practice:** On an embedded device, a poisoned mutex usually means something went seriously wrong. Consider:
- Log the poisoning event prominently
- Reset the engine state to a known-good snapshot
- Or restart the device (watchdog reset) — often the safest choice in embedded

---

### 4. Callback Subscriptions Not Implemented (Severity: MEDIUM)

`main.rs:136-139` logs a TODO for callback delivery. Subscriptions can be created with callback type, but delivery silently does nothing. This is a correctness issue — the client thinks it has a working subscription.

**Best practice:** Either:
- Return an error (501 Not Implemented) when callback subscriptions are requested
- Or implement the feature

Silently accepting but not delivering is the worst option.

---

### 5. No Structured Logging / Log Levels (Severity: MEDIUM)

The codebase uses `log::info!`, `log::warn!`, etc., which is good. But there's no evidence of:
- Structured fields (request ID, session ID, path) in log messages
- Configurable log levels per module
- Rate limiting for repeated errors (e.g., WiFi reconnect spam)

**Best practice:** On constrained devices, logging can consume significant resources. Consider:
- `log::set_max_level()` at boot based on build profile
- Include session IDs in server logs for debugging WebSocket issues
- Rate-limit repeated warnings (WiFi reconnect loop at 5s interval could flood serial)

---

### 6. Two recv() Calls Per WebSocket Frame (Severity: LOW-MEDIUM)

`server.rs:315` — The first `recv()` call with an empty buffer gets the frame length, then a second call reads the data. This doubles syscall overhead per message.

**Best practice:** Allocate a reasonable initial buffer (e.g., 1KB) and resize only if needed. Check if the ESP-IDF API supports a single-call pattern. Even if not, document the cost.

---

### 7. No Graceful Shutdown Path (Severity: LOW-MEDIUM)

`main.rs` has an infinite loop with no exit condition. There's no mechanism to:
- Flush NVS before power-off
- Drain pending subscription deliveries
- Close WebSocket connections cleanly

**Best practice:** While embedded devices often don't shut down gracefully, having a shutdown signal (e.g., GPIO button, MQTT command) that flushes state prevents data loss during firmware updates or controlled restarts.

---

### 8. BTreeMap for Hot-Path Lookups (Severity: LOW)

Subscriptions, WebSocket senders, and session mappings all use `BTreeMap`. With the current bounds (32 subscriptions, handful of WS clients), this is fine. But BTreeMap is O(log n) lookup vs HashMap's O(1).

**Best practice:** This is acceptable given the small N. Just be aware that if bounds increase significantly, switching to HashMap (or a fixed-size array with linear scan for very small N) would be faster. The ordered iteration benefit of BTreeMap is only used in serialization.

---

### 9. No Fuzz Testing for Parsers (Severity: LOW)

The JSON parsing (serde_json) and path parsing are well-tested but not fuzzed. On an internet-facing device, parser bugs are a common attack vector.

**Best practice:** Add `cargo-fuzz` targets for:
- OMI JSON message parsing
- O-DF path parsing
- WebSocket frame handling
- Script source validation

Even a few hours of fuzzing can catch edge cases that unit tests miss.

---

### 10. PSRAM Allocator Panics on Failure (Severity: LOW)

`psram.rs` panics if `heap_caps_malloc` returns null. On an embedded device, panic = reset (with `panic_abort`).

**Best practice:** This is arguably correct for embedded (OOM is unrecoverable), but consider:
- Logging the allocation size and available PSRAM before panicking
- Using `Result` and propagating, so callers can try to free memory first

---

## Structural Observations

### What's Missing (Not Bugs, but Gaps)

| Area | Status | Risk |
|------|--------|------|
| OTA firmware updates | Not present | Can't update in the field |
| Watchdog timer | Not configured | Device can hang silently |
| Metrics/telemetry | Only FreeHeap | Limited observability |
| Rate limiting | None | DoS via rapid requests |
| TLS/HTTPS | Not configured | Credentials sent in cleartext |
| Configuration validation | Partial | Bad .env silently fails |

### Architecture Diagram (Actual)

```
┌─────────────────────────────────────────────┐
│  main.rs (ESP-only)                         │
│  WiFi · NVS · Main loop · Sensor polling    │
├────────────┬────────────────────────────────┤
│ server.rs  │  http.rs      pages.rs         │
│ HTTP/WS    │  Routing      Page store       │
├────────────┴──────┬─────────────────────────┤
│  omi/             │  odf/                   │
│  Engine · Subs    │  Tree · Object · Item   │
│  Read · Write     │  Value · RingBuffer     │
│  Delete · Cancel  │                         │
├───────────────────┴─────────────────────────┤
│  scripting/           nvs.rs    psram.rs    │
│  mJS engine · FFI     Storage   PSRAM alloc │
└─────────────────────────────────────────────┘
```

The layering is clean. Dependencies flow downward. No circular dependencies. Platform code is confined to the top layer.

---

## Summary Scorecard

| Category | Grade | Notes |
|----------|-------|-------|
| HAL / Platform separation | **A** | Exemplary. ESP code isolated to 3 files |
| Memory management | **A** | Bounded everything, ring buffers, PSRAM encapsulation |
| Error handling | **A-** | Comprehensive, except silent callback subscription issue |
| Thread safety | **B+** | Correct, but mutex poisoning recovery strategy is risky |
| Input validation | **A** | Path traversal, depth guards, XSS, constant-time auth |
| Testing | **B+** | Strong core coverage, platform code untested |
| Security | **B** | Good basics, missing TLS and rate limiting |
| Fault tolerance | **B-** | No watchdog, no script timeout, no graceful shutdown |
| Observability | **C+** | Basic logging only, no structured telemetry |

**Overall: B+ / A-** — This is well above average for an embedded project. The core architecture is sound. The main risks are in operational resilience (script timeout, watchdog, OTA) rather than code quality.
