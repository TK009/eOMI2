# Next Steps: Thesis vs Implementation Gap Analysis

Review of the master thesis ("Smart Home: Design and Implementation of IoT-Based Reconfigurable Smart Home System", Keyriläinen 2021) against the current Rust/ESP32 reimplementation.

---

## Thesis Vision Summary

The thesis proposes a **Reconfigurable Smart Object System (RSOS)** with three layers:
1. **Service Layer** — online IoT app marketplace, adapter database, 3rd-party services
2. **Management Layer** — configurator app for discovery, provisioning, reconfiguration
3. **Device Layer** — embedded O-MI/O-DF system with scripting support

The embedded system (device layer) was the implemented thesis component. The current project is a **Rust reimplementation** of that C/Arduino prototype, using the OMI-Lite spec (JSON instead of XML).

---

## What's Complete (Thesis Goals Achieved)

These thesis objectives are fully implemented in the current codebase:

| Thesis Feature | Status | Notes |
|---|---|---|
| O-DF hierarchical data model | DONE | `src/odf/` — Object, InfoItem, Value, tree operations |
| O-MI request handling (read/write/delete/cancel) | DONE | `src/omi/engine.rs` — full request processing |
| Subscription system (event, interval, poll) | DONE | `src/omi/subscriptions.rs` — all 3 types |
| HTTP transport (POST /omi) | DONE | `src/server.rs` |
| REST discovery (GET /omi/*) | DONE | `src/server.rs` |
| WebSocket transport (/omi/ws) | DONE | `src/server.rs` — bidirectional, sub delivery |
| Sensor interfacing via O-DF tree | DONE | `src/device.rs` + `src/main.rs` — DHT11 |
| Scripting engine integration | DONE | `src/scripting/` — mJS, FFI bindings |
| onwrite triggers invoke scripts | DONE | `engine.rs:process_write` calls `run_onwrite_script` |
| Script cascading with depth guard | DONE | MAX_SCRIPT_DEPTH = 4 |
| Script API: odf.writeItem(value, path) | DONE | `src/scripting/bindings.rs` |
| NVS persistence across reboots | DONE | `src/nvs.rs` |
| JSON instead of XML (OMI-Lite evolution) | DONE | Thesis used XML; this implements JSON variant |
| Platform-independent core testable on host | DONE | 415+ tests, `cargo test-host` |
| HTML page storage (configurator app hosting) | DONE | `src/pages.rs` |
| Bearer token authentication | DONE | `src/server.rs` — constant-time comparison |

### Architecture review issues (from tasks/architecture-review.md) — resolved:

| Issue | Status |
|---|---|
| main.rs monolith | FIXED — HTTP logic extracted to server.rs |
| ObjectTree.objects pub | FIXED — now private with accessor methods |
| html.rs/js.rs empty stubs | FIXED — removed |
| No on_write script invocation | FIXED — fully wired |
| Missing Default impls | N/A — Engine::new() with side-effects is appropriate |

---

## What's Missing (Thesis Features Not Yet Implemented)

### Priority 1: Core Protocol Gaps

#### 1.1 mDNS / DNS-SD Device Discovery
**Thesis**: Chapter 4.2.1 — devices advertise via mDNS with DNS-SD, common hostname for configurator app entry, LAN device discovery results published as O-DF InfoItems.
**Current**: Not implemented. No mDNS advertisement, no discovery.
**Impact**: High — required for plug-and-play and configurator app.
**Task**: Integrate `esp-idf-svc` mDNS service, advertise `_omi._tcp` service, register hostname. Write discovered devices to O-DF tree.

#### 1.2 Callback Subscription Delivery (HTTP client)
**Thesis**: Chapter 4.3 / Section 5.1.2 — event/interval subscriptions with callback URL send results to the subscriber's HTTP endpoint. This is the P2P mechanism.
**Current**: Subscriptions work for WebSocket and poll, but **HTTP callback delivery is not implemented**. The subscription system stores callbacks but never POSTs to them.
**Impact**: High — P2P device-to-device communication depends on this.
**Task**: Implement HTTP client POST in subscription delivery path. Fire-and-forget with 1 retry.

#### 1.3 Script API: odf.readItem(path)
**Thesis**: Table 4.3 — scripts can read values from the local O-DF tree via `odf.readItem(path)`.
**Current**: Only `odf.writeItem(value, path)` is implemented. No read binding.
**Impact**: Medium — scripts that need to make decisions based on current values require this.
**Task**: Add `odf_read_item` FFI binding in `src/scripting/bindings.rs`.

#### 1.4 onread Script Trigger
**Thesis**: Table 4.2 — scripts can be triggered on read requests, not just writes.
**Current**: Only `onwrite` trigger exists. No `onread`.
**Impact**: Low-medium — enables computed/derived values.
**Task**: Check for `onread` metadata in `process_read`, execute script, use return value.

### Priority 2: Device Management

#### 2.1 Wi-Fi Provisioning
**Thesis**: Section 4.3.1 — automated Wi-Fi provisioning via hotspot/captive portal, with hidden Wi-Fi for device-to-device credential sharing.
**Current**: Wi-Fi credentials are hardcoded in `.env`. No provisioning flow.
**Impact**: Medium — required for consumer-grade UX.
**Task**: Implement ESP-IDF Wi-Fi provisioning (SoftAP mode with captive portal for credential input). Fall back to `.env` if provisioned credentials exist.

#### 2.2 Over-the-Air (OTA) Updates
**Thesis**: Section 3 mentions OTA as a capability of the platform.
**Current**: Not implemented. Firmware updates require USB.
**Impact**: Medium — important for deployed devices.
**Task**: Use ESP-IDF OTA API. Could be triggered via an OMI write to a special path (e.g., `/Device/FirmwareUpdate`).

### Priority 3: Robustness & Quality

#### 3.1 Lock Ordering Documentation / Refactor
**Thesis**: N/A (new concern from Rust reimplementation).
**Current**: `architecture-review.md` flags the two-lock pattern (Engine mutex + ws_senders). Lock ordering is undocumented.
**Impact**: Low (single-core ESP32-S2) but fragile.
**Task**: Document lock ordering invariant. Consider making ws_senders a field of Engine.

#### 3.2 Rate Limiting
**Thesis**: Chapter 7 — acknowledges no security beyond LAN assumption.
**Current**: `architecture-review.md` flags no rate limiting. Any LAN device can flood requests.
**Impact**: Medium — denial of service on constrained device.
**Task**: Token bucket rate limiter in main loop. Simple counter per time window.

#### 3.3 NVS Binary Encoding
**Thesis**: N/A.
**Current**: NVS uses JSON serialization. `architecture-review.md` suggests `postcard` for more compact encoding.
**Impact**: Low — only matters when NVS is near capacity.
**Task**: Replace serde_json with postcard for NVS blob format.

#### 3.4 WebSocket Error Responses via OmiResponse Builder
**Thesis**: N/A.
**Current**: Hand-written JSON error strings in WS handler bypass `OmiResponse` builder.
**Impact**: Low — format inconsistency risk.
**Task**: Use `OmiResponse::error(status, desc)` and serialize for WS error frames.

### Priority 4: Thesis Architecture (Not in Embedded Scope)

These are thesis-proposed features that live **outside** the embedded system but are referenced for completeness:

#### 4.1 Configurator Application
**Thesis**: Chapter 4.2 — web app for device discovery, script installation, subscription setup, adapter matching.
**Current**: Not implemented. Scripts/subscriptions configured manually via API.
**Note**: This is a separate project (web app), not embedded firmware.

#### 4.2 IoT App Marketplace / Adapter Database
**Thesis**: Chapter 4.2.3–4.2.4 — online registry of scripts, adapters for data type conversion, device matcher rules.
**Current**: Not implemented.
**Note**: Cloud service, separate project.

#### 4.3 Multi-LAN / Internet Gateway
**Thesis**: Chapter 7 (future work) — extend P2P beyond single LAN.
**Current**: Not in scope.

---

## Recommended Execution Order

```
Phase A — P2P Communication (enables thesis use cases)
  A.1  Callback subscription delivery (HTTP client)
  A.2  mDNS/DNS-SD device discovery
  A.3  odf.readItem() script binding

Phase B — Device UX
  B.1  Wi-Fi provisioning (SoftAP + captive portal)
  B.2  OTA firmware updates

Phase C — Hardening
  C.1  Rate limiting
  C.2  Lock ordering documentation
  C.3  WS error response cleanup
  C.4  NVS binary encoding (postcard)

Phase D — Extended Scripting
  D.1  onread trigger scripts
  D.2  Script subscription API (future thesis work)

Phase E — Ecosystem (separate projects)
  E.1  Configurator web app
  E.2  Adapter/app marketplace
```

### Rationale

Phase A is prioritized because callback delivery + mDNS are the **minimum for P2P device-to-device communication**, which is the thesis's core contribution. Without callback delivery, subscriptions only work when the subscriber holds an open WebSocket or polls — insufficient for autonomous device operation. Without mDNS, devices can't find each other.

Phase B follows because provisioning and OTA are needed before any real deployment.

Phase C addresses robustness issues identified in the architecture review. None are blockers but they accumulate risk.

Phase D adds scripting features that the thesis specifies but that aren't critical path.

Phase E is out of scope for the embedded firmware project but documented for completeness.

---

## Test Coverage for Next Steps

Each phase should add tests:

| Phase | Host Tests | E2E Tests |
|---|---|---|
| A.1 Callback delivery | Mock HTTP client, verify delivery scheduling | Create sub with callback → write → verify callback received |
| A.2 mDNS | N/A (ESP-specific) | Verify device appears via mDNS query |
| A.3 odf.readItem | Unit test script reads values | Script reads + acts on sensor value |
| B.1 Provisioning | N/A | Manual test flow |
| B.2 OTA | N/A | Flash via OTA, verify running |
| C.1 Rate limiting | Unit test token bucket | Rapid fire requests, verify throttling |
| D.1 onread trigger | Engine test: read triggers script | HTTP read with onread script |

---

## Summary

The current implementation has achieved **all core embedded system goals** from the thesis:
- Full OMI-Lite protocol (read/write/delete/cancel/subscriptions)
- Scripting with onwrite triggers and cascading
- JSON data model replacing XML
- Persistence, authentication, REST discovery, WebSocket

The **critical gaps** for matching the thesis vision are:
1. **HTTP callback delivery** — enables autonomous P2P subscriptions
2. **mDNS discovery** — enables plug-and-play device finding
3. **odf.readItem()** — completes the script API from thesis Table 4.3

Everything else is either hardening, UX improvement, or ecosystem work (configurator app, marketplace) that lives outside the embedded firmware.
