# Implementation Spec: WiFi Secure Onboarding Protocol (WSOP)

**Spec**: `010-wifi-secure-onboarding`
**Created**: 2026-03-18
**Status**: Draft
**Design Doc**: `doc/wifi-secure-onboarding-spec.md` (WSOP v1.0)
**Depends On**: `002-wifi-provisioning` (captive portal as fallback)

## Overview

Implement the WSOP protocol from the design doc. Two roles exist — both
run the same eOMI firmware:

- **New device (joiner)**: Has no WiFi credentials. Scans for a hidden
  onboarding SSID. If found, connects to it, generates an X25519 keypair,
  displays a verification color on its NeoPixel, and writes a JOIN_REQUEST
  to the gateway's O-DF tree via a standard OMI write.
- **Gateway device**: An already-provisioned eOMI device that runs a hidden
  AP (`_eomi_onboard`) and exposes `JoinRequest` / `JoinResponse` InfoItems
  in its O-DF tree. The owner reads the pending request (via any OMI client),
  visually confirms the LED color, and writes an approval. The gateway then
  constructs the encrypted JOIN_RESPONSE and writes it to the `JoinResponse`
  InfoItem. The joiner reads it, decrypts, connects to the real WiFi, and
  destroys the private key.

If no hidden onboarding SSID is found within a timeout, the device falls back
to captive portal provisioning (spec 002).

After onboarding completes, the NeoPixel is released for general application
use (GPIO status, scripting, etc.).

## Clarifications (2026-03-18)

- **Q: Who is the AP?** → Any already-provisioned eOMI device. It runs a hidden
  SSID and exposes OMI InfoItems for the join/response exchange.
- **Q: Transport?** → Dedicated hidden SSID. Joiner connects as STA, uses
  standard OMI HTTP writes to exchange messages.
- **Q: Keypair lifecycle on retry?** → Fresh keypair per attempt. Verification
  color changes each retry.
- **Q: Management UI?** → Gateway model. Any OMI client (phone app, web UI,
  another device) reads the gateway's JoinRequest InfoItem and writes approval.
  No custom approval UI needed — it's just OMI read/write.
- **Q: Replaces or layers on portal?** → WSOP replaces the portal when the
  hidden SSID is found within a scan timeout. If not found, falls back to
  captive portal (spec 002).
- **Q: NeoPixel after onboarding?** → Released for general application use.
  The `ws2812.rs` driver is a general-purpose module, not onboarding-only.

## User Scenarios & Testing

### User Story 1 — First Boot Secure Onboarding (Priority: P1)

A user has an existing eOMI device on their network acting as a gateway. They
power on a new device. The new device's NeoPixel lights up Cyan. The user opens
an OMI client (e.g., phone browser pointed at the gateway), reads the
`PendingRequests` InfoItem, sees "Cyan" + the device MAC, looks at the physical
LED to confirm, and writes an approval to the `Approval` InfoItem. The new
device connects to WiFi automatically.

**Why P1**: Core protocol — without it there is no secure onboarding.

**Acceptance Scenarios**:

1. **Given** a factory-fresh device with a NeoPixel and a gateway device broadcasting `_eomi_onboard`, **When** the new device powers on, **Then** it connects to the hidden SSID, generates an X25519 keypair, computes BLAKE2b(pubkey), displays the corresponding color, and writes a JOIN_REQUEST to the gateway's `Objects/OnboardingGateway/JoinRequest` InfoItem within 5 seconds.
2. **Given** the gateway has received a JOIN_REQUEST, **When** any OMI client reads the `JoinRequest` InfoItem, **Then** it returns: device name, MAC, verification color name, pubkey (hex), and remaining approval time.
3. **Given** the owner has verified the LED color, **When** they write an approval value to the `Approval` InfoItem (containing the device's MAC and `"action": "approve"`), **Then** the gateway encrypts its WiFi credentials via `crypto_box_seal` to the device's pubkey and writes the encrypted payload to the `JoinResponse` InfoItem.
4. **Given** the joiner reads an approved JoinResponse, **When** it decrypts successfully, **Then** it stores credentials in NVS, disconnects from the onboarding SSID, connects to the real WiFi, destroys the X25519 private key, and turns off the verification color (releasing the NeoPixel for application use).
5. **Given** the joiner receives a denial or exhausts retries, **When** onboarding fails, **Then** it disconnects from the onboarding SSID and falls back to captive portal mode (spec 002).

---

### User Story 2 — Gateway Role (Priority: P1)

An already-provisioned eOMI device acts as an onboarding gateway. It runs a
hidden AP alongside its normal STA connection and exposes O-DF InfoItems for
the onboarding exchange.

**Why P1**: Without a gateway there is no one to onboard joiners.

**Acceptance Scenarios**:

1. **Given** a provisioned device with `secure_onboarding` enabled, **When** it is connected to WiFi in STA mode, **Then** it also starts a hidden AP with SSID `_eomi_onboard` (AP+STA coexistence).
2. **Given** the gateway is running, **When** a joiner writes to the `JoinRequest` InfoItem, **Then** the gateway validates the timestamp (±300s), computes the verification code from the pubkey, and stores the pending request with a 60-second approval timer.
3. **Given** a pending request exists, **When** the approval window expires without owner action, **Then** the gateway writes a denial to `JoinResponse` and clears the pending request.
4. **Given** the owner writes an approval to the `Approval` InfoItem, **When** the gateway processes it, **Then** it reads its own WiFi credentials from NVS, encrypts them with `crypto_box_seal(plaintext, joiner_pubkey)`, and writes the ciphertext to `JoinResponse`.
5. **Given** multiple joiners simultaneously, **When** requests arrive, **Then** the gateway queues up to 4 pending requests, each with independent approval timers and MAC disambiguation.

---

### User Story 3 — Digit Verification for Non-LED Devices (Priority: P2)

A device without an RGB LED (e.g., the WROVER dev board) uses blink-count
verification. On first boot, the onboard LED blinks N times (0–9), and the
OMI JoinRequest value includes the expected digit.

**Why P2**: Extends WSOP to devices without NeoPixels. The protocol is
identical; only the display mode changes.

**Acceptance Scenarios**:

1. **Given** a device without `neopixel_pin` configured, **When** it boots for onboarding, **Then** it blinks its default LED `digit` times (derived from `BLAKE2b(pubkey) % 10`) in a repeating pattern.
2. **Given** the JoinRequest InfoItem value, **When** read by an OMI client from a non-LED device, **Then** the value includes `display_mode: "digit"` and the expected blink count.

---

### User Story 4 — Timeout, Denial, and Fallback (Priority: P1)

No gateway found, or the owner ignores/denies all requests. The device falls
back to captive portal gracefully.

**Acceptance Scenarios**:

1. **Given** a new device scanning for `_eomi_onboard`, **When** the SSID is not found within the scan timeout (default 10s, 3 active scan passes), **Then** the device skips WSOP and enters captive portal mode directly.
2. **Given** a pending join request on the gateway, **When** the 60-second approval window expires, **Then** the gateway writes a denial response.
3. **Given** the joiner has been denied, **When** it retries, **Then** it generates a **new** keypair (verification color changes).
4. **Given** the joiner has exhausted `max_retry_attempts` (default 6), **Then** it disconnects from the onboarding SSID and transitions to captive portal mode.

---

### User Story 5 — Replay and MITM Resistance (Priority: P1)

**Acceptance Scenarios**:

1. **Given** a JOIN_REQUEST with a timestamp outside ±300s of gateway clock, **When** the gateway validates it, **Then** the request is silently discarded.
2. **Given** a JOIN_RESPONSE, **When** its `nonce_echo` does not match the joiner's sent nonce, **Then** the joiner discards the response.
3. **Given** an attacker substitutes their pubkey in a JOIN_REQUEST, **When** the owner compares the OMI-reported color to the device LED, **Then** there is a 7-in-8 chance of mismatch detection.

---

### Edge Cases

- Device has no RTC and timestamp is wildly off → gateway uses ±300s tolerance; device uses compile-time epoch as lower bound.
- Multiple devices onboarding simultaneously → gateway queues requests; each identified by MAC in the InfoItem tree.
- Power loss during credential storage → NVS write is atomic (existing FR-013 from spec 002).
- Joiner reads a JoinResponse for a different nonce → silently ignored.
- NeoPixel pin is configured but LED is physically broken → OMI client can still read the digit from the JoinRequest value.
- Gateway reboots while joiner is waiting → joiner's poll times out, retries with fresh keypair.
- Onboarding SSID scan timeout vs. slow AP startup → timeout must accommodate multiple scan cycles across all channels; default 10s determined by hardware testing.
- Hidden SSID not found by standard scan → `wifi_ap::scan_networks()` filters `ssid.is_empty()` (line 100 of `wifi_ap.rs`). Joiner MUST use ESP-IDF active scan API with `show_hidden=true` and target SSID set, not the existing `scan_networks()` function.
- Gateway's hidden AP channel mismatch → ESP32-S2 single radio requires AP and STA on same channel. If the home network is on channel 6, the hidden AP is automatically on channel 6. Joiner's active scan covers all channels, so this is transparent.

## Requirements

### Functional Requirements

**Cryptography:**

- **FR-100**: Joiner MUST generate an X25519 keypair from hardware RNG (`esp_fill_random`) on each onboarding attempt.
- **FR-101**: Private key MUST be stored only in RAM (not NVS) during the onboarding window. It MUST be zeroed after successful credential decryption or after onboarding failure.
- **FR-102**: Verification code MUST be computed as `BLAKE2b(pubkey, output_length=1)` with the top 3 bits selecting one of 8 colors.
- **FR-103**: Gateway MUST encrypt credentials using `crypto_box_seal(plaintext, joiner_pubkey)` (XSalsa20-Poly1305 authenticated encryption).
- **FR-104**: Joiner MUST reject any ciphertext that fails Poly1305 authentication.

**Protocol — Joiner Side:**

- **FR-110**: Joiner MUST perform an ESP-IDF **active scan** with `show_hidden=true` and `ssid` set to `_eomi_onboard` on boot when no WiFi credentials exist. Standard passive scanning (as used by `wifi_ap::scan_networks()`) will not discover hidden SSIDs. The scan MUST be repeated up to 3 times across all channels (active scan with specific SSID forces probe requests). If not found within the scan timeout (default 10s, configurable 5–30s), fall back to captive portal.
- **FR-111**: After connecting to the onboarding SSID, joiner MUST write a JOIN_REQUEST to the gateway's `Objects/OnboardingGateway/JoinRequest` InfoItem via OMI HTTP write.
- **FR-112**: JOIN_REQUEST value MUST contain: protocol_version (0x01), device name, MAC (6 bytes), X25519 pubkey (32 bytes), random nonce (8 bytes), Unix timestamp (4 bytes), display_mode ("color" or "digit"). Serialized as the binary format from the design doc (max 83 bytes), base64-encoded for the OMI InfoItem value.
- **FR-113**: After writing the request, joiner MUST poll the gateway's `Objects/OnboardingGateway/JoinResponse` InfoItem at 10-second intervals (configurable 5–60s), filtering by its own nonce.
- **FR-114**: Joiner MUST discard any JoinResponse whose `nonce_echo` does not match.
- **FR-115**: Joiner MUST retry up to `max_retry_attempts` (default 6) with a fresh keypair each attempt before falling back to captive portal.

**Protocol — Gateway Side:**

- **FR-120**: A provisioned device with `secure_onboarding` enabled MUST run a hidden AP with SSID `_eomi_onboard` alongside its STA connection (AP+STA mode). The hidden AP MUST use the same channel as the STA connection (ESP32-S2 single-radio constraint).
- **FR-121**: Gateway MUST expose `/Objects/OnboardingGateway/JoinRequest`, `/Objects/OnboardingGateway/JoinResponse`, `/Objects/OnboardingGateway/PendingRequests`, and `/Objects/OnboardingGateway/Approval` InfoItems in its O-DF tree.
- **FR-122**: On receiving a JoinRequest write, gateway MUST validate `timestamp` within ±300s of its own clock. Invalid requests are silently discarded.
- **FR-123**: Gateway MUST compute the verification code from the received pubkey and include it (color name or digit) in the JoinRequest InfoItem value alongside the device name, MAC, and remaining approval time.
- **FR-124**: Gateway MUST enforce the approval window (default 60s, configurable 30–300s). Expired requests are auto-denied by writing a denial to JoinResponse.
- **FR-125**: Gateway MUST support up to 4 simultaneous pending requests, disambiguated by MAC address.
- **FR-126**: On owner approval (write to `Approval` InfoItem with target MAC and `"action": "approve"`), gateway MUST read its own WiFi credentials from NVS, serialize them per the design doc plaintext format, encrypt with `crypto_box_seal`, and write the ciphertext to the `JoinResponse` InfoItem. For the `security_type` byte in the credential payload, the gateway MUST use `0x01` (WPA2-PSK) in v1. Determining the actual security type from the connected AP is deferred to v2.

**Verification Display:**

- **FR-130**: On boards with `neopixel_pin`, joiner MUST display the verification color on the WS2812 NeoPixel using the 8-color palette from the design doc.
- **FR-131**: On boards without `neopixel_pin`, joiner MUST blink the default GPIO LED `(BLAKE2b(pubkey) % 10)` times in a repeating pattern.
- **FR-132**: Verification display MUST remain active until onboarding completes (success or fallback).
- **FR-133**: After successful onboarding, the NeoPixel MUST be released for general application use (GPIO control, scripting, etc.).

**Credential Delivery:**

- **FR-140**: Encrypted plaintext MUST contain: SSID (length-prefixed, max 32 bytes), security_type (1 byte: 0x01=WPA2-PSK, 0x02=WPA3-SAE), credential (length-prefixed, max 63 bytes).
- **FR-141**: After successful decryption, joiner MUST store credentials in NVS using the existing `wifi_cfg` persistence format.
- **FR-142**: After successful WiFi connection, the X25519 private key MUST be zeroed in RAM.

**Integration:**

- **FR-150**: The onboarding flow MUST run as a **separate state machine** (`onboard_sm.rs`) that executes *before* `WifiSm` is entered. `WifiSm` remains unchanged — it is a clean, platform-independent WiFi connection FSM and MUST NOT be extended with onboarding-specific states. On successful onboarding, credentials are stored in NVS and `WifiSm` is constructed with `num_creds > 0` (normal boot path). On failure, `WifiSm` is constructed with `num_creds == 0` (captive portal path).
- **FR-151**: If WSOP fails (SSID not found or retries exhausted), the system MUST fall back to captive portal (spec 002) transparently.
- **FR-152**: A build-time feature flag `secure_onboarding` MUST gate all WSOP code. When disabled, the device uses captive portal only.
- **FR-153**: Gateway functionality (hidden AP + OMI InfoItems) MUST be enabled automatically on any provisioned device with `secure_onboarding` enabled.

### Non-Functional Requirements

- **NFR-001**: X25519 keypair generation MUST complete in under 100ms.
- **NFR-002**: `crypto_box_seal` encryption/decryption MUST complete in under 200ms.
- **NFR-003**: Total flash overhead for WSOP crypto code MUST be under 16 KB.
- **NFR-004**: Joiner RAM usage for onboarding state (keypair + nonce + buffers) MUST be under 512 bytes. All onboarding state is stack-allocated or short-lived heap and freed after onboarding completes.
- **NFR-004b**: Gateway RAM usage for pending request queue MUST be under 512 bytes. Each pending entry stores: pubkey (32B), MAC (6B), name (up to 32B), nonce (8B), timestamp (4B), timer (8B) = ~90 bytes × 4 max entries = ~360 bytes.
- **NFR-005**: All crypto and protocol code MUST be host-testable (no ESP dependencies in core logic).
- **NFR-006**: Hidden AP (`_eomi_onboard`) MUST NOT degrade STA throughput by more than 10%.
- **NFR-007**: WS2812 driver MUST be usable independently of WSOP (general-purpose NeoPixel control).

## Architecture

### Dual Role: Every Device is Both Joiner and Gateway

```
┌─────────────────────────────────────────────────────────────────┐
│                        eOMI Firmware                            │
│                                                                 │
│  ┌──────────────────────┐    ┌──────────────────────────────┐  │
│  │   Joiner Role        │    │   Gateway Role               │  │
│  │   (no creds in NVS)  │    │   (provisioned, STA active)  │  │
│  │                      │    │                              │  │
│  │ 1. Scan for hidden   │    │ 1. Start hidden AP           │  │
│  │    SSID              │    │    "_eomi_onboard"           │  │
│  │ 2. Connect as STA    │    │ 2. Register OMI InfoItems:   │  │
│  │ 3. Generate keypair  │    │    JoinRequest (writable)    │  │
│  │ 4. Display verify    │    │    JoinResponse (readable)   │  │
│  │    code on NeoPixel  │    │ 3. On JoinRequest write:     │  │
│  │ 5. OMI write →       │───>│    validate, queue, timer    │  │
│  │    JoinRequest       │    │ 4. On owner approval:        │  │
│  │ 6. OMI read ←        │<───│    seal creds → JoinResponse │  │
│  │    JoinResponse      │    │ 5. Serve onboarding clients  │  │
│  │ 7. Decrypt, connect, │    │    on hidden AP HTTP server  │  │
│  │    destroy key       │    │                              │  │
│  └──────────────────────┘    └──────────────────────────────┘  │
│                                                                 │
│  Role selected at boot based on: NVS has WiFi creds?           │
│    No  → Joiner                                                │
│    Yes → Gateway (+ normal operation)                          │
└─────────────────────────────────────────────────────────────────┘
```

### O-DF Tree: Gateway InfoItems

```xml
<Objects>
  <Object>
    <id>OnboardingGateway</id>
    <InfoItem name="JoinRequest">
      <!-- Writable by joiners. Value: base64-encoded JOIN_REQUEST binary.
           Gateway processes on write, enriches with verify code + timer. -->
    </InfoItem>
    <InfoItem name="JoinResponse">
      <!-- Read-only for joiners. Written ONLY by gateway logic.
           Value: base64-encoded JOIN_RESPONSE binary (per nonce).
           Joiner polls this for a response matching its nonce. -->
    </InfoItem>
    <InfoItem name="PendingRequests">
      <!-- Readable by owner/OMI clients. JSON array of pending requests:
           [{ "mac": "AA:BB:...", "name": "sensor-1", "color": "Cyan",
              "digit": 4, "display_mode": "color", "expires_in": 45 }] -->
    </InfoItem>
    <InfoItem name="Approval">
      <!-- Writable by owner/OMI clients. Owner writes here to approve/deny.
           Value: { "mac": "AA:BB:CC:DD:EE:FF", "action": "approve" }
           Gateway's onwrite handler processes this and writes to JoinResponse. -->
    </InfoItem>
  </Object>
</Objects>
```

The owner's approval flow is a standard OMI write to the `Approval` InfoItem:
```
POST /write
{ "/Objects/OnboardingGateway/Approval": { "mac": "AA:BB:CC:DD:EE:FF", "action": "approve" } }
```

The gateway's `onwrite` handler for `Approval` triggers the `crypto_box_seal`
encryption and writes the result to the `JoinResponse` InfoItem. This keeps
`JoinResponse` as a single-purpose output channel (gateway → joiner) and
`Approval` as a single-purpose input channel (owner → gateway), avoiding the
ambiguity of dual-purpose InfoItems.

### New Modules

```
src/
├── wsop/
│   ├── mod.rs              # Feature-gated module root, role selection
│   ├── crypto.rs           # X25519 keygen, BLAKE2b verify code, sealed-box seal/open
│   ├── protocol.rs         # JOIN_REQUEST/JOIN_RESPONSE serialization (no_std, no-alloc)
│   ├── joiner.rs           # Joiner-side logic: scan, connect, write request, poll response
│   ├── gateway.rs          # Gateway-side logic: hidden AP, InfoItems, approval queue, seal creds
│   ├── onboard_sm.rs       # Joiner onboarding state machine (retry, timeout, fallback)
│   └── display.rs          # Verification code display (delegates to ws2812 or blink)
├── ws2812.rs               # General-purpose WS2812 NeoPixel driver (RMT-based, ESP-only)
```

### Crypto Strategy: Pure Rust

**Decision: Use pure-Rust crates (`x25519-dalek`, `blake2`, `crypto_box`).**

Rationale:
- mbedTLS has ECC disabled in `sdkconfig.defaults` (~15 KB savings). Re-enabling
  pulls in ECP + bignum — more flash than the pure-Rust approach.
- `x25519-dalek` ~8 KB + `crypto_box` ~4 KB + `blake2` ~2 KB = ~14 KB total.
- Host-testable without ESP-IDF.
- Gateway needs `crypto_box_seal` (encrypt). Joiner needs `crypto_box_seal_open`
  (decrypt). Both crates needed on every device since any device can be either role.

New dependencies in `Cargo.toml`:
```toml
x25519-dalek = { version = "2", default-features = false, features = ["static_secrets"], optional = true }
crypto_box = { version = "0.9", default-features = false, features = ["seal"], optional = true }
blake2 = { version = "0.10", default-features = false, optional = true }
base64 = { version = "0.22", default-features = false, features = ["alloc"], optional = true }
getrandom = { version = "0.2", features = ["custom"], optional = true }
```

Feature gate:
```toml
secure_onboarding = ["dep:x25519-dalek", "dep:crypto_box", "dep:blake2", "dep:base64", "dep:getrandom"]
```

**`getrandom` backend**: `x25519-dalek` and `crypto_box` use `OsRng` which requires
`getrandom`. On ESP-IDF, `getrandom` v0.2 with the `custom` feature needs a backend
registration that calls `esp_fill_random()`. This is a one-time setup in
`wsop/crypto.rs`:
```rust
#[cfg(all(feature = "esp", feature = "secure_onboarding"))]
getrandom::register_custom_getrandom!(esp_getrandom);

fn esp_getrandom(buf: &mut [u8]) -> Result<(), getrandom::Error> {
    unsafe { esp_idf_svc::sys::esp_fill_random(buf.as_mut_ptr() as _, buf.len() as _) };
    Ok(())
}
```

**`base64`**: Used for encoding/decoding JOIN_REQUEST and JOIN_RESPONSE binary
payloads as OMI InfoItem string values. The `alloc` feature provides `encode`/`decode`
without requiring `std`.

### WS2812 NeoPixel Driver (General-Purpose)

```rust
// src/ws2812.rs — general-purpose WS2812 driver via ESP-IDF RMT
pub struct Ws2812 { /* rmt channel handle, pin */ }

impl Ws2812 {
    pub fn new(pin: u8) -> Result<Self, EspError>;
    pub fn set_color(&mut self, r: u8, g: u8, b: u8) -> Result<(), EspError>;
    pub fn off(&mut self) -> Result<(), EspError>;
}
```

Uses `espressif/led_strip` managed component (RMT hardware timing). Available
for WSOP verification display, then released for application use. Registered
in the GPIO system like any other peripheral after onboarding.

### State Machine Integration

The onboarding flow runs as a **separate FSM** (`OnboardSm` in `wsop/onboard_sm.rs`)
that executes in `main.rs` *before* constructing `WifiSm`. This preserves `WifiSm`
as a clean, platform-independent WiFi connection manager — no WSOP-specific states
leak into it.

```
Boot
  │
  ├─ NVS has WiFi creds?
  │    │
  │    No ──► secure_onboarding enabled?
  │    │       │
  │    │       Yes ──► OnboardSm runs:
  │    │       │         ├─ ActiveScan (hidden SSID, timeout 10s)
  │    │       │         │    │              │
  │    │       │         │  Found         Timeout
  │    │       │         │    │              │
  │    │       │         │    ▼              ▼
  │    │       │         │  Connected to   OnboardSm returns Err
  │    │       │         │  onboard AP     → WifiSm(num_creds=0) → Portal
  │    │       │         │    │
  │    │       │         │  Keygen + Display color
  │    │       │         │  OMI write JoinRequest
  │    │       │         │  Poll JoinResponse
  │    │       │         │    │           │
  │    │       │         │  Approved    Exhausted (6 retries)
  │    │       │         │    │           │
  │    │       │         │    ▼           ▼
  │    │       │         │  Decrypt →   OnboardSm returns Err
  │    │       │         │  Store NVS   → WifiSm(num_creds=0) → Portal
  │    │       │         │  Destroy key
  │    │       │         │  Release NeoPixel
  │    │       │         │  OnboardSm returns Ok(creds)
  │    │       │         │    │
  │    │       │         │    ▼
  │    │       │         │  WifiSm(num_creds=N) → normal connect flow
  │    │       │
  │    │       No ──► WifiSm(num_creds=0) → Portal (002)
  │    │
  │    Yes ──► WifiSm(num_creds=N) → normal connect flow
  │            + Start gateway (hidden AP + InfoItems)
```

`WifiSm` is **NOT modified**. The `OnboardSm` states are:

```rust
// wsop/onboard_sm.rs — joiner-only FSM, platform-independent
pub enum OnboardState {
    Scanning { attempts: u8 },       // active scan for _eomi_onboard
    Connecting,                       // associating with hidden AP
    Requesting { attempt: u8 },       // write JoinRequest, await response
    Polling { attempt: u8, polls: u8 }, // polling JoinResponse
    Decrypting,                       // received response, decrypting
    Succeeded,                        // creds obtained
    Failed,                           // exhausted retries or timeout
}

pub enum OnboardEvent {
    SsidFound,
    SsidTimeout,
    Connected,
    ConnectFailed,
    RequestWritten,
    ResponseApproved { ciphertext: Vec<u8> },
    ResponseDenied,
    PollTimeout,
    DecryptOk { ssid: String, password: String },
    DecryptFailed,
}

pub enum OnboardAction {
    ActiveScan,
    ConnectToSsid,
    WriteJoinRequest,
    PollJoinResponse,
    Decrypt { ciphertext: Vec<u8> },
    StoreCredentials { ssid: String, password: String },
    Done,             // success — caller constructs WifiSm with creds
    Fallback,         // failure — caller constructs WifiSm with 0 creds
}
```

### Gateway HTTP Server on Hidden AP

The existing HTTP server binds to `0.0.0.0:80`, which serves both the STA and
AP network interfaces. **No second server is needed.** When the gateway starts
the hidden AP in AP+STA mode, the joiner connects to the AP interface
(192.168.4.1) and reaches the same HTTP server. The joiner uses standard OMI
read/write to interact with the OnboardingGateway InfoItems.

No custom WSOP endpoints needed. The OMI engine's existing write handler
processes `JoinRequest` writes, and the gateway logic hooks in via the
`onwrite` trigger mechanism (similar to spec 006 InfoItem triggers).

### Build-Time Configuration

Board TOML gets a new optional field:

```toml
[board]
neopixel_pin = 18          # existing — also used for WSOP + general app use
onboard_display = "color"  # "color" (NeoPixel) | "digit" (LED blink) | "none"
```

`build.rs` generates:
```rust
pub const ONBOARD_DISPLAY_MODE: u8 = 0; // 0=color, 1=digit, 2=none
```

**Defaults**: If `onboard_display` is absent from the board TOML:
- If `neopixel_pin` is set → default to `"color"`
- If `neopixel_pin` is absent but a GPIO with `name = "LED"` exists → default to `"digit"`
- Otherwise → default to `"none"` (onboarding still works, but no visual verification)

## Implementation Plan

### Phase 1 — Crypto & Protocol (host-testable, no ESP deps)

1. Add `x25519-dalek`, `crypto_box`, `blake2` behind `secure_onboarding` feature.
2. `wsop/crypto.rs`: keygen, verify-code derivation, `seal` (gateway), `seal_open` (joiner).
3. `wsop/protocol.rs`: serialize/deserialize JOIN_REQUEST and JOIN_RESPONSE, base64 encode/decode for InfoItem values.
4. `wsop/onboard_sm.rs`: joiner state machine (scan → connect → request → poll → decrypt → fallback).
5. Host tests for all three modules — full round-trip: keygen → serialize → seal → deserialize → open.

### Phase 2 — WS2812 Driver (general-purpose)

1. Add `espressif/led_strip` managed component.
2. `ws2812.rs`: RMT-based NeoPixel driver with `set_color` / `off`.
3. `wsop/display.rs`: color display (NeoPixel) and digit display (LED blink).
4. Update `boards/mod.rs` to initialize WS2812 from `neopixel_pin`.
5. Register NeoPixel in GPIO system for post-onboarding application use.
6. Extend board TOML schema with `onboard_display` field.

### Phase 3 — Gateway Role

1. Implement hidden AP startup in AP+STA mode (`_eomi_onboard`, hidden=true).
2. Register `OnboardingGateway` O-DF subtree with `JoinRequest`, `JoinResponse`, `PendingRequests`, and `Approval` InfoItems.
3. Implement `onwrite` handler for `JoinRequest`: validate timestamp, compute verify code, start approval timer, update `PendingRequests`.
4. Implement `onwrite` handler for `Approval`: look up pending request by MAC, read NVS creds, `crypto_box_seal`, write encrypted response to `JoinResponse`.
5. Implement approval timeout → auto-denial.
6. Wire gateway startup into `main.rs`: if provisioned + `secure_onboarding` → start gateway.

### Phase 4 — Joiner Role & main.rs Integration

1. `wsop/joiner.rs`: ESP-specific active scan for hidden SSID, connect to onboard AP, OMI write JoinRequest, poll JoinResponse.
2. Wire `OnboardSm` into `main.rs` boot sequence: no creds + `secure_onboarding` → run `OnboardSm` before constructing `WifiSm`. `WifiSm` is NOT modified.
3. On `OnboardSm::Succeeded`: credentials already in NVS → construct `WifiSm(num_creds=N)` → normal connect flow. Destroy private key, release NeoPixel.
4. On `OnboardSm::Failed`: construct `WifiSm(num_creds=0)` → captive portal fallback.
5. Fallback: SSID not found (10s) or retries exhausted → captive portal.

### Phase 5 — E2E Testing

1. `test_wsop_onboarding.py`: Two devices — gateway approves joiner, joiner connects to real WiFi.
2. `test_wsop_denial.py`: Deny/timeout, verify fallback to captive portal.
3. `test_wsop_replay.py`: Replay old JOIN_REQUEST, verify gateway rejection.
4. `test_wsop_color_change.py`: Deny first attempt, verify joiner's LED color changes on retry.
5. `test_wsop_no_gateway.py`: No hidden SSID available, verify fallback to portal within scan timeout (~10s).
6. Requires two devices: Saola-1 (NeoPixel, joiner) + WROVER (gateway).

## Success Criteria

- **SC-001**: Joiner displays verification color within 3 seconds of connecting to onboarding SSID.
- **SC-002**: Full onboarding (boot → real WiFi connected) completes in under 90 seconds with prompt owner approval.
- **SC-003**: Fallback to captive portal occurs within 15 seconds when no onboarding SSID exists (10s scan + margin).
- **SC-004**: Fallback to captive portal occurs within ~80 seconds when gateway denies all attempts (10s scan + 6 retries × ~10s poll + processing).
- **SC-005**: Private key is zeroed in RAM immediately after credential decryption (host test verified).
- **SC-006**: All crypto and protocol code passes host tests with zero ESP dependencies.
- **SC-007**: Flash overhead for WSOP feature is under 16 KB.
- **SC-008**: NeoPixel is usable for application purposes after onboarding completes.
- **SC-009**: Gateway hidden AP does not degrade STA throughput by more than 10%.
- **SC-010**: Standard OMI read/write is the only interface needed for owner approval — no custom UI required.

## Assumptions

- ESP32-S2 supports AP+STA coexistence (hidden AP + STA simultaneously). ESP-IDF docs confirm this for ESP32-S2.
- `esp_fill_random()` provides sufficient entropy on cold boot (hardware RNG on ESP32-S2).
- `espressif/led_strip` managed component supports ESP32-S2 RMT.
- `x25519-dalek` and `crypto_box` compile for Xtensa with `default-features = false`.
- The gateway reads its own WiFi credentials from NVS to encrypt for the joiner. This means the gateway stores credentials in a retrievable format (not just hashed). The current `wifi_cfg.rs` stores SSID + password in binary — this is already retrievable.
- Only WPA2-PSK credentials are delivered in v1 (matches current `wifi_cfg` support). WPA3-SAE is protocol-specified but deferred.
- The hidden SSID `_eomi_onboard` uses a fixed name (not configurable in v1). A future enhancement could use mDNS to discover the onboarding gateway.
- Base64 encoding of the binary protocol for OMI InfoItem values adds ~33% overhead (83 bytes → ~112 bytes). This is acceptable for the InfoItem value size.

## Open Questions

1. **RNG seeding on cold boot**: Does `esp_fill_random` block until entropy is ready, or do we need to wait? If it doesn't block, we may need a short delay before keygen.
2. **Xtensa crypto performance**: Need to benchmark `x25519-dalek` scalar multiply on ESP32-S2 (240 MHz LX7). The 100ms NFR may need adjustment.
3. **LED strip component flash size**: Measure actual impact. If >4 KB, consider minimal RMT bit-bang instead.
4. **AP+STA channel constraint & scan timeout**: ESP32-S2 has a single radio — the hidden AP and STA must be on the same channel (handled automatically by ESP-IDF). The joiner uses active scan with `ssid=_eomi_onboard` and `show_hidden=true`, forcing probe requests on each channel. Default timeout set to 10s (configurable 5–30s) with up to 3 scan passes. Hardware testing needed to validate: (a) whether 10s is sufficient for reliable hidden SSID discovery across 13 channels, (b) whether 3 passes is enough. This directly determines the fallback-to-portal latency.
5. **`crypto_box` no_std + Xtensa**: Verify `seal` feature compiles. May need `getrandom` backend wiring for `OsRng` on ESP-IDF.
6. ~~**Gateway credential retrieval**~~: **RESOLVED.** Code review confirms `wifi_cfg.rs` stores SSID + password in cleartext binary (lines 96–141). Only the API key is SHA-256 hashed (line 44). WiFi passwords are retrievable via `deserialize_wifi_config()`. The gateway can read its own credentials from NVS to encrypt for the joiner.
