# Next Steps: Thesis vs Implementation Gap Analysis

Review of the master thesis ("Smart Home: Design and Implementation of IoT-Based Reconfigurable Smart Home System", Keyriläinen 2021) against the current Rust/ESP32 reimplementation using the OMI-Lite spec (JSON instead of XML).

**Last updated**: 2026-03-15

---

## Thesis Vision Summary

The thesis proposes a **Reconfigurable Smart Object System (RSOS)** with three layers:
1. **Service Layer** — online IoT app marketplace, adapter database, 3rd-party services
2. **Management Layer** — configurator app for discovery, provisioning, reconfiguration
3. **Device Layer** — embedded O-MI/O-DF system with scripting support

The embedded system (device layer) was the implemented thesis component. The current project is a **Rust reimplementation** of that C/Arduino prototype, using the OMI-Lite spec (JSON instead of XML).

---

## What's Complete (Thesis & Spec Goals Achieved)

| Feature | Thesis Source | Spec Source | Implementation |
|---------|-------------|-------------|----------------|
| O-DF hierarchical data model | Ch.4-5 | §4 | `src/odf/` — Object, InfoItem, Value, tree ops |
| Read/Write/Delete/Cancel operations | §5.1.2 | §6 | `src/omi/engine.rs` — full request processing |
| Subscription system (event, interval, poll) | §5.1.2 | §7 | `src/omi/subscriptions.rs` — all 3 types, max 32 |
| HTTP transport (POST /omi) | §5.1 | §2.1 | `src/server.rs` |
| REST discovery (GET /omi/*) | — | §2.2 | `src/server.rs` — query params, trailing slash |
| WebSocket transport (/omi/ws) | §2.2.1 | §2.3 | `src/server.rs` — bidirectional, sub delivery |
| HTTP callback delivery | §4.2.2, §5.1.2 | §7.2 | `src/callback.rs` — fire-and-forget, 1 retry, 5s timeout |
| `javascript://` callback protocol | Table 4.2 | — | `src/callback.rs` — local script execution via sub |
| Scripting engine (mJS) | §4.3.2, §5.2 | — | `src/scripting/` — FFI bindings, safety limits |
| `onwrite` trigger | Table 4.2 | — | `engine.rs:process_write` calls `run_onwrite_script` |
| `onread` trigger | Table 4.2 | — | Engine executes on read, transforms value in-place |
| `odf.writeItem(path, value)` | Table 4.3 | — | `src/scripting/bindings.rs` |
| `odf.readItem(path)` | Table 4.3 | — | `src/scripting/bindings.rs` — reads from tree snapshot |
| Script cascading with depth guard | §5.2 | — | MAX_SCRIPT_DEPTH = 4 |
| `global` object for cross-invocation state | Table 4.3 | — | mJS global context persists |
| `event` / `arguments[0]` context | Table 4.3 | — | Injected before script execution |
| mDNS hostname advertisement | §4.2.1 | — | `src/mdns.rs` — STA-only, auto IP update |
| DNS-SD service browsing | §4.2.1 | — | `src/mdns_discovery.rs` — 30s polling cycle |
| Discovery results as O-DF InfoItems | §4.2.1 | — | `/System/discovery/<hostname>` with IP:port |
| WiFi provisioning (captive portal) | §4.3.1 | — | `src/wifi_sm.rs` — AP mode, form-based, multi-AP |
| WiFi auto-reconnect | §4.3.1 | — | Exponential backoff, SSID reappear detection |
| NVS persistence across reboots | §5.1 | — | `src/nvs.rs` — atomic blob write, SHA256 keys |
| GPIO (digital in/out, ADC, PWM) | §5.1 | — | `src/gpio/` — polling, ISR edge triggers, 100ms cadence |
| Peripheral buses (UART, SPI, I2C) | §5.1 | — | `src/gpio/` — RX/TX, encoding support, I2C scan |
| System stats (heap, flash, PSRAM, temp) | Appendix A | — | `/System/` InfoItems with metadata |
| Sensor interfacing (DHT11 etc.) | §5.3 | — | `src/device.rs` + `src/main.rs` |
| JSON instead of XML | — | §1 | OMI-Lite JSON protocol throughout |
| Platform-independent core on host | — | — | 415+ tests, `cargo test-host` |
| HTML page hosting | §4.2.1 | — | `src/pages.rs` — configurator app entry point |
| Bearer token authentication | Ch.7 | §12 | `src/server.rs` — constant-time comparison |
| Script execution safety | — | Spec 001 | Op-count (50k) + wall-clock (5s) limits |
| Rate-limited logging | — | Spec 001 | FNV-1a dedup, 10s window |
| Batch write | — | §6.2 | Single, batch (items array), object tree forms |
| Message envelope validation | — | §5 | Version check, TTL, single-op validation |

### Architecture review issues (from tasks/architecture-review.md) — resolved:

| Issue | Status |
|---|---|
| main.rs monolith | FIXED — HTTP logic extracted to server.rs |
| ObjectTree.objects pub | FIXED — now private with accessor methods |
| html.rs/js.rs empty stubs | FIXED — removed |
| No on_write script invocation | FIXED — fully wired |
| Missing Default impls | N/A — Engine::new() with side-effects is appropriate |

---

## What's Missing (Remaining Gaps)

### Priority 1: Embedded Firmware Gaps

#### 1.1 Over-the-Air (OTA) Firmware Updates
**Thesis**: Section 3 — Arduino supports OTA if flash has room for two firmware images.
**Current**: Not implemented. Firmware updates require USB.
**Impact**: High — critical for deployed devices that can't be physically accessed.
**Task**: Use ESP-IDF OTA API (`esp_ota_ops`). Trigger via OMI write to `/System/FirmwareUpdate` or dedicated HTTP endpoint. Requires dual-partition flash layout.

#### 1.2 NTP / Time Synchronization
**Thesis**: §5.1 — NTP mentioned for timestamps but "did not work at the moment of writing" in original C implementation.
**Current**: Unclear if ESP-IDF SNTP is configured. Accurate timestamps matter for subscription delivery and value history.
**Impact**: Medium — value timestamps may be wrong after reboot until time syncs.
**Task**: Verify SNTP configuration in ESP-IDF. If missing, enable `esp_sntp` with pool.ntp.org fallback. Write current time source to `/System/Time` InfoItem.

#### 1.3 `oncall` Script Trigger (Design Decision)
**Thesis**: Table 4.2 — `oncall` MetaData InfoItem triggers script on O-MI `call` request. Receives parameters, returns result. Essentially an RPC mechanism.
**OMI-Lite Spec**: Intentionally dropped `call` operation — "use write + onwrite scripts (proven by eOMI)".
**Current**: Not implemented.
**Impact**: Low — `onwrite` covers most use cases. The thesis itself proved the system works without `call` in both test cases (bathroom fan, A/C).
**Decision**: Skip unless a use case emerges that `onwrite` can't handle. Document as intentional omission.

#### 1.4 Hidden WiFi Auto-Provisioning Between Devices
**Thesis**: §4.3.1, Fig 4.4 — provisioned devices create a hidden WiFi hotspot. New devices auto-discover nearby eOMI devices and request WiFi credentials without user intervention.
**Current**: Captive portal for manual provisioning is done. Device-to-device auto-provisioning is not.
**Impact**: Low — nice-to-have for multi-device setup UX.
**Task**: After STA connect, create a hidden AP with known SSID pattern. New devices scan for hidden SSIDs, connect, and request credentials via HTTP GET.

#### 1.5 Device Identity InfoItems
**Thesis**: Appendix A — read response includes `ChipModel`, `ChipRevision`, `FirmwareBuildVersion`, `SdkVersion`, `FirmwareBuildDate`, `FirmwareBuildTime` under `/Device/`.
**Current**: May be partially present. These help the configurator app identify device model for adapter matching.
**Impact**: Low — metadata for tooling, not core functionality.
**Task**: Verify presence. If missing, add to `build_sensor_tree()` using compile-time constants from `build.rs`.

### Priority 2: OMI-Lite Spec Completeness

#### 2.1 CBOR Encoding
**Spec**: §3 — CBOR supported as compact binary alternative. Same data model, ~30-50% smaller wire size.
**Current**: JSON only.
**Impact**: Low — spec marks CBOR as optional. Useful for bandwidth-constrained scenarios.
**Task**: Add CBOR serializer/deserializer behind feature flag. Content-Type negotiation.

#### 2.2 Content Negotiation
**Spec**: §3 — `Content-Type` and `Accept` headers. WebSocket protocol negotiation (`omi-json`, `omi-cbor`). Return 415 for unsupported types.
**Current**: Likely accepts any Content-Type without checking.
**Impact**: Low — correctness issue, not functionality.
**Task**: Validate `Content-Type: application/json`, return 415 otherwise. Check `Accept` header.

#### 2.3 TTL Expiry (408 Response)
**Spec**: §8.3 — if server can't fulfill before TTL expires, return 408.
**Current**: TTL is parsed but expiry checking during processing may not be implemented.
**Impact**: Low — only matters for slow operations or ttl=0 (respond immediately).
**Task**: Check elapsed time against TTL before responding. Return 408 if expired.

#### 2.4 Partial Success for Batch Write
**Spec**: §8.4 — batch write with partial failure returns per-item `{path, status, desc}` array.
**Current**: May return aggregate success/failure without per-item detail.
**Impact**: Low — correctness for batch error reporting.
**Task**: Collect per-item results during batch processing, return array on partial failure.

### Priority 3: Ecosystem (Outside Embedded Scope)

These are thesis-proposed features that live **outside** the embedded firmware:

| Feature | Thesis Source | Notes |
|---------|-------------|-------|
| Configurator web application | §4.2 | Web app for discovery, script install, subscription setup, adapter matching |
| Adapter database | §4.2.3 | Online registry of data-type conversion scripts |
| IoT App marketplace | §4.2.4 | Online registry of agent scripts with device matcher rules |
| Multi-LAN / Internet gateway | §7 (future) | Extend P2P beyond single LAN |
| Script subscription API | §7, §4.2.2 (future) | Allow scripts to create subscriptions dynamically |

---

## Recommended Execution Order

```
Phase A — Deployment Readiness
  A.1  OTA firmware updates
  A.2  NTP time synchronization (verify/fix)
  A.3  Device identity InfoItems (verify/fix)

Phase B — Spec Completeness
  B.1  Content negotiation (Accept/Content-Type validation)
  B.2  TTL expiry checking (408 response)
  B.3  Batch write partial success reporting

Phase C — Extended Features
  C.1  Hidden WiFi auto-provisioning
  C.2  CBOR encoding (feature-flagged)

Phase D — Ecosystem (separate projects)
  D.1  Configurator web app
  D.2  Adapter/app marketplace
```

### Rationale

Phase A is prioritized because OTA is the **only blocking gap for real-world deployment**. Everything else in the embedded firmware is functional. NTP and device identity are quick verification tasks.

Phase B addresses spec compliance gaps — all are small, self-contained tasks.

Phase C adds nice-to-have features. Hidden WiFi provisioning improves multi-device UX. CBOR improves wire efficiency.

Phase D is out of scope for the embedded firmware project.

---

## Reference: Thesis Table 4.2 — Script Trigger Mechanisms

| Location in O-DF/O-MI | Content | Trigger | Status |
|------------------------|---------|---------|--------|
| MetaData: `onwrite` (type="javascript") | JS code in `<value>` | Script executes when InfoItem value is written | **DONE** |
| MetaData: `onread` (type="javascript") | JS code in `<value>` | Script executes when InfoItem value is read | **DONE** |
| MetaData: `oncall` (type="javascript") | JS code in `<value>` | Script executes on O-MI `call` request | **SKIPPED** — `call` dropped in OMI-Lite |
| Read `callback` attribute | `javascript://<O-DF-path>` | Subscription results passed to script at path | **DONE** |

## Reference: Thesis Table 4.3 — Script API

| API | Description | Status |
|-----|-------------|--------|
| `global` | Global object containing all global variables/functions. Persists across invocations. | **DONE** |
| `event` / `arguments[0]` | Event handler arguments. For `onwrite`: the InfoItem with value being written. For `oncall`: the O-DF parameter. | **DONE** (onwrite only; oncall skipped) |
| `odf.readItem(path)` | Read element or value from O-DF tree. `/value` ending returns value directly. | **DONE** |
| `odf.writeItem(path, value)` | Write element or value to O-DF tree. | **DONE** |
| Result of last statement | Replaces event value (onread) or is the return value (oncall). | **DONE** (onread; oncall skipped) |

## Reference: OMI-Lite Spec — Feature Coverage

| Spec Section | Feature | Status | Notes |
|-------------|---------|--------|-------|
| §2.1 | HTTP POST /omi | **DONE** | |
| §2.2 | REST Discovery (GET /omi/*) | **DONE** | Query params: newest, oldest, begin, end, depth |
| §2.3 | WebSocket /omi/ws | **DONE** | Bidirectional, subscription delivery |
| §3 | JSON encoding | **DONE** | Default and only encoding |
| §3 | CBOR encoding | **TODO** | Optional per spec |
| §3 | Content negotiation | **TODO** | Accept/Content-Type headers |
| §4 | Hierarchical object tree | **DONE** | Objects, InfoItems, Values, Metadata |
| §5 | Message envelope | **DONE** | Version, TTL, single-op validation |
| §6.1 | Read (one-time) | **DONE** | Path, newest, oldest, begin, end, depth |
| §6.1 | Read (subscription) | **DONE** | interval, callback |
| §6.1 | Read (poll by rid) | **DONE** | |
| §6.2 | Write (single value) | **DONE** | path + v + optional t |
| §6.2 | Write (batch) | **DONE** | items array |
| §6.2 | Write (object tree) | **DONE** | path + objects |
| §6.3 | Delete | **DONE** | Forbids deleting `/` |
| §6.4 | Cancel | **DONE** | Array of rids |
| §6.5 | Response | **DONE** | Status codes, rid, desc, result |
| §7.1-7.2 | Subscription creation & delivery | **DONE** | Event (-1) and interval (>0) |
| §7.3 | WebSocket subscriptions | **DONE** | Deliver on same WS, cancel on disconnect |
| §7.4 | Subscription lifetime (TTL, cancel, WS close) | **DONE** | |
| §7.5 | Polling (callback-less HTTP) | **DONE** | Ring buffer, bounded |
| §8.1 | Transport errors (400, 405) | **DONE** | |
| §8.2 | Application errors in response body | **DONE** | |
| §8.3 | TTL expiry (408) | **TODO** | May not be checked during processing |
| §8.4 | Partial success (batch write) | **TODO** | Per-item status array |
| §12.3 | Write protection (writable metadata) | **DONE** | Enforced on write |
| §12.5 | Path validation | **DONE** | Rejects traversal attempts |

## Reference: Thesis Feature Coverage (Embedded Layer Only)

| Thesis Section | Feature | Status | Notes |
|---------------|---------|--------|-------|
| §4.2.1 | mDNS hostname advertisement | **DONE** | STA-only, auto IP update |
| §4.2.1 | DNS-SD service browsing | **DONE** | `_omi._tcp`, 30s polling |
| §4.2.1 | Discovery results in O-DF tree | **DONE** | `/System/discovery/<hostname>` |
| §4.2.1 | Configurator app hosted on device | **DONE** | HTML pages served |
| §4.2.2 | Event subscriptions (interval=-1) | **DONE** | |
| §4.2.2 | Interval subscriptions | **DONE** | Min 0.1s |
| §4.2.2 | Callback delivery (P2P) | **DONE** | HTTP POST, retry, javascript:// |
| §4.3.1 | WiFi captive portal provisioning | **DONE** | AP mode, form-based |
| §4.3.1 | Hidden WiFi auto-provisioning | **TODO** | Device-to-device credential sharing |
| §4.3.2 | Script engine (JavaScript) | **DONE** | mJS with safety limits |
| §4.3.2 | `onwrite` trigger | **DONE** | |
| §4.3.2 | `onread` trigger | **DONE** | |
| §4.3.2 | `oncall` trigger | **SKIPPED** | Intentional — `call` dropped in OMI-Lite |
| §4.3.2 | `javascript://` callback | **DONE** | Local script execution via subscription |
| Table 4.3 | `global` persistent state | **DONE** | |
| Table 4.3 | `event` context object | **DONE** | |
| Table 4.3 | `odf.readItem(path)` | **DONE** | |
| Table 4.3 | `odf.writeItem(path, value)` | **DONE** | |
| Table 4.3 | Last-statement return value | **DONE** | For onread value transform |
| §3 | OTA firmware updates | **TODO** | ESP-IDF OTA API |
| §5.1 | NTP time synchronization | **VERIFY** | May be handled by ESP-IDF SNTP |
| §5.1 | O-DF tree with sorted paths | **DONE** | BTreeMap (Rust equivalent) |
| §5.1 | Streaming parser | **DONE** | Lite JSON parser (spec 007) |
| §5.1.2 | Interval subscription scheduler | **DONE** | Priority queue, timer-based |
| §5.3 | Bathroom fan use case | **DONE** | Equivalent test coverage in e2e |
| Appendix A | Device info InfoItems | **VERIFY** | ChipModel, FirmwareVersion, Memory stats |
| Appendix A | Memory stats (Heap, PSRAM, Flash) | **DONE** | `/System/` InfoItems with totals |

---

## Summary

The implementation has achieved **all core embedded system goals** from the thesis and nearly complete OMI-Lite spec coverage:

- Full OMI-Lite protocol (read/write/delete/cancel/subscriptions)
- All three subscription types (event, interval, poll) with all delivery mechanisms (WebSocket, HTTP callback, polling)
- Complete script API from thesis Table 4.3 (`onwrite`, `onread`, `odf.readItem`, `odf.writeItem`, `global`, `event`, `javascript://` callbacks)
- mDNS/DNS-SD device discovery with O-DF tree integration
- WiFi provisioning via captive portal with multi-AP support
- GPIO/peripheral system (digital, analog, PWM, UART, SPI, I2C)
- System stats, NVS persistence, bearer token auth
- JSON data model replacing XML, platform-independent core with 415+ host tests

**The only significant remaining gap is OTA firmware updates.** Everything else is either spec polish (CBOR, content negotiation, TTL expiry), nice-to-have UX (hidden WiFi auto-provisioning), or ecosystem work outside the embedded firmware (configurator app, marketplace).
