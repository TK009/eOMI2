# WiFi Secure Onboarding Protocol (WSOP) v1.0

**Status:** Draft  
**Date:** 2026-03-18  
**Author:** —  

---

## 1. Overview

This specification defines a protocol for securely onboarding new IoT/embedded devices onto a WiFi network. A first-time-booted device broadcasts a join request. The network owner approves or denies the request via a management interface. If approved, WiFi credentials are sent back encrypted so that only the original requesting device can decrypt them.

A visual verification step (RGB LED color or single-digit number displayed on the device) prevents man-in-the-middle attacks on the initial request.

### 1.1 Design Goals

- **Smallest possible public key:** 32 bytes (X25519).
- **No pre-shared secrets:** The device and access point have no prior relationship.
- **MITM-resistant:** Out-of-band visual verification closes the gap that raw public-key exchange leaves open.
- **Simple to implement:** Uses libsodium primitives available on virtually all microcontrollers.

### 1.2 Threat Model

| Threat | Mitigated By |
|--------|-------------|
| Attacker spoofs MAC to receive credentials | Encryption to device's public key (only holder of private key can decrypt) |
| Attacker intercepts join request and substitutes their own public key | Visual verification code derived from the public key — user confirms match |
| Attacker replays an old join request | Timestamp + nonce in the request; approval window is time-limited |
| Attacker brute-forces the verification code | Code is only used to confirm human-visible match, not as a secret; the 128-bit X25519 security remains the cryptographic barrier |

---

## 2. Cryptographic Primitives

| Primitive | Algorithm | Size |
|-----------|-----------|------|
| Key exchange | X25519 (Curve25519 ECDH) | 32-byte public key, 32-byte private key |
| Authenticated encryption | XSalsa20-Poly1305 via `crypto_box_seal` | 48 bytes overhead (32-byte ephemeral pubkey + 16-byte MAC) |
| Verification code derivation | BLAKE2b hash of public key, truncated | 1 byte (see Section 5) |

All cryptographic operations use **libsodium** (or compatible: mbedTLS, Monocypher).

---

## 3. Actors and Components

| Actor | Role |
|-------|------|
| **Device** | New IoT device booting for the first time. Has an RGB LED or numeric display. Generates a keypair. |
| **Access Point (AP)** | WiFi access point or onboarding gateway. Receives join requests, relays to the management interface, and sends encrypted credentials. |
| **Owner** | Human with access to a management interface (phone app, web UI). Approves or denies join requests after visual verification. |

---

## 4. Protocol Flow

```
Device                          AP / Gateway                     Owner (App/UI)
  |                                  |                                |
  |  1. Generate X25519 keypair      |                                |
  |  2. Compute verification code    |                                |
  |  3. Display code on LED/screen   |                                |
  |                                  |                                |
  |──── JOIN_REQUEST ───────────────>|                                |
  |  { name, mac, pubkey, nonce, ts }|                                |
  |                                  |──── Approval Prompt ──────────>|
  |                                  |  { name, mac, verify_code,     |
  |                                  |    time_remaining }            |
  |                                  |                                |
  |                                  |  Owner looks at device LED,    |
  |                                  |  confirms code matches UI      |
  |                                  |                                |
  |                                  |<──── APPROVE / DENY ──────────|
  |                                  |                                |
  |<──── JOIN_RESPONSE ─────────────|                                |
  |  { encrypted_credentials }       |                                |
  |                                  |                                |
  |  4. Decrypt with private key     |                                |
  |  5. Connect to WiFi              |                                |
  |  6. Destroy private key          |                                |
```

---

## 5. Verification Code

The verification code is derived deterministically from the device's public key so both sides can compute it independently.

### 5.1 Derivation

```
code_bytes = BLAKE2b(pubkey, output_length=1)   // 1 byte = 0x00..0xFF
```

The single byte is then mapped to one of two display modes depending on device hardware.

### 5.2 Display Mode A — RGB LED Color

The byte is mapped to one of **8 distinct colors** using the top 3 bits:

| Bits [7:5] | Color   | RGB Value      |
|------------|---------|----------------|
| 000        | Red     | (255, 0, 0)    |
| 001        | Green   | (0, 255, 0)    |
| 010        | Blue    | (0, 0, 255)    |
| 011        | Yellow  | (255, 255, 0)  |
| 100        | Cyan    | (0, 255, 255)  |
| 101        | Magenta | (255, 0, 255)  |
| 110        | White   | (255, 255, 255)|
| 111        | Orange  | (255, 128, 0)  |

The management UI displays the expected color name and a color swatch. The owner visually confirms the device LED matches.

### 5.3 Display Mode B — Single Digit

The byte is reduced to a digit 0–9:

```
digit = code_bytes[0] mod 10
```

The device shows this digit on a 7-segment display, small screen, or by blinking the LED that many times. The management UI displays the expected digit.

### 5.4 Security Consideration

With 8 colors or 10 digits, an attacker performing a MITM has a **1-in-8** or **1-in-10** chance of their substituted public key producing the same verification code. This is acceptable for the threat model (opportunistic IoT onboarding) because:

- The attacker must be physically proximate and actively intercepting at the exact moment of first boot.
- A failed verification is immediately visible to the owner.
- The window is time-limited (see Section 7).

For higher security, the code can be extended to **2 bytes** (a color + a digit, yielding 1-in-80 collision probability) or a **4-digit PIN** (1-in-10,000) at the cost of more complex display hardware.

---

## 6. Message Formats

### 6.1 JOIN_REQUEST (Device → AP)

Sent over an open/unauthenticated channel (e.g., probe request frame, BLE advertisement, or a dedicated onboarding SSID).

```
Field            Type        Bytes   Description
─────────────────────────────────────────────────────────
protocol_version uint8         1     0x01 for this spec
name             utf8         1+32   Length-prefixed, max 32 chars. Human-readable device name.
mac              bytes          6    Device MAC address
pubkey           bytes         32    X25519 public key
nonce            bytes          8    Random nonce (replay protection)
timestamp        uint32         4    Unix timestamp (seconds), device's best estimate
─────────────────────────────────────────────────────────
Total                     max 83 bytes
```

### 6.2 JOIN_RESPONSE (AP → Device)

Sent only after owner approval. The payload is a `crypto_box_seal` ciphertext addressed to the device's public key.

```
Field            Type        Bytes   Description
─────────────────────────────────────────────────────────
protocol_version uint8         1     0x01
nonce_echo       bytes          8    Must match the nonce from JOIN_REQUEST
status           uint8         1     0x01 = approved, 0x00 = denied
ciphertext       bytes      varies   crypto_box_seal(plaintext, device_pubkey)
─────────────────────────────────────────────────────────
```

**Plaintext structure inside ciphertext (when status = 0x01):**

```
Field            Type        Bytes   Description
─────────────────────────────────────────────────────────
ssid             utf8         1+32   Length-prefixed SSID
security_type    uint8         1     0x01=WPA2-PSK, 0x02=WPA3-SAE, 0x03=WPA2-Enterprise
credential       utf8         1+63   Length-prefixed passphrase or credential blob
─────────────────────────────────────────────────────────
Total plaintext           max 97 bytes
Ciphertext overhead           48 bytes (32 ephemeral pubkey + 16 MAC)
Total ciphertext          max 145 bytes
```

### 6.3 DENIED Response

When `status = 0x00`, the `ciphertext` field is empty. The device should back off and may retry after a configurable delay.

---

## 7. Timing and Expiry

| Parameter | Default | Range |
|-----------|---------|-------|
| Approval window | 60 seconds | 30–300 s |
| Request retry interval | 10 seconds | 5–60 s |
| Max retry attempts | 6 | 1–20 |
| Timestamp tolerance | ±300 seconds | Accounts for devices without RTC |

The AP must discard any JOIN_REQUEST whose `timestamp` is outside the tolerance window relative to its own clock. After the approval window expires without owner action, the request is implicitly denied.

---

## 8. Device Lifecycle

### 8.1 First Boot

1. Device generates an X25519 keypair using a hardware RNG (or OS-provided CSPRNG).
2. Private key is stored in secure storage (flash with read protection, secure element if available).
3. Verification code is computed from the public key and displayed on LED/screen.
4. Device begins transmitting JOIN_REQUEST at the retry interval.

### 8.2 Successful Onboarding

1. Device receives JOIN_RESPONSE with `status = 0x01`.
2. Device decrypts the ciphertext using its private key.
3. Device stores WiFi credentials in secure storage.
4. Device **destroys the X25519 private key** — it is no longer needed.
5. Device connects to the WiFi network.
6. LED/screen stops displaying the verification code.

### 8.3 Denied or Timed Out

1. Device receives `status = 0x00` or exhausts retries.
2. Device enters a backoff state (configurable: retry after power cycle, or exponential backoff).
3. Keypair may be retained or regenerated on next attempt (implementation-defined).

### 8.4 Factory Reset

1. Device destroys stored WiFi credentials and any stored keypair.
2. On next boot, device generates a fresh keypair and re-enters the onboarding flow.

---

## 9. Transport Options

This protocol is transport-agnostic. The JOIN_REQUEST and JOIN_RESPONSE payloads can be carried over any of:

| Transport | Notes |
|-----------|-------|
| **Dedicated onboarding SSID** | AP runs an open SSID (e.g., `_setup`) that only accepts onboarding traffic. Simplest option. |
| **WiFi probe request/response** | Embed payload in vendor-specific IEs in 802.11 management frames. No association needed. |
| **BLE** | Device advertises a GATT service; phone/gateway connects and exchanges messages. Good for phone-as-gateway setups. |
| **802.15.4 / Thread** | For mesh networks; onboarding payload carried as application-layer messages. |

---

## 10. Implementation Notes

### 10.1 Recommended Libraries

| Platform | Library |
|----------|---------|
| C / Embedded | libsodium (`sodium.h`), Monocypher |
| Rust | `x25519-dalek`, `crypto_box` crate |
| Python (gateway/AP) | PyNaCl (`nacl.public.SealedBox`) |
| JavaScript (management UI) | `tweetnacl` / `libsodium.js` |

### 10.2 crypto_box_seal Usage

**Encryption (AP side, Python example):**

```python
from nacl.public import SealedBox, PublicKey

device_pubkey = PublicKey(pubkey_bytes)  # 32 bytes from JOIN_REQUEST
box = SealedBox(device_pubkey)
ciphertext = box.encrypt(plaintext)      # 48 + len(plaintext) bytes
```

**Decryption (Device side, C example):**

```c
// device_pk = 32-byte public key
// device_sk = 32-byte private key
// ciphertext, ciphertext_len from JOIN_RESPONSE

unsigned char plaintext[ciphertext_len - crypto_box_SEALBYTES];
if (crypto_box_seal_open(plaintext, ciphertext, ciphertext_len,
                          device_pk, device_sk) != 0) {
    // decryption failed — tampered or wrong key
}
```

### 10.3 Verification Code (C example)

```c
#include <sodium.h>

uint8_t verify_byte;
crypto_generichash(
    &verify_byte, 1,           // output: 1 byte
    device_pk, 32,             // input: public key
    NULL, 0                    // no key (unkeyed hash)
);

uint8_t color_index = verify_byte >> 5;   // top 3 bits → 0..7
uint8_t digit = verify_byte % 10;         // mod 10 → 0..9
```

---

## 11. Security Analysis

### 11.1 What Is Protected

- **Confidentiality of WiFi credentials:** Only the device holding the private key can decrypt the response. Passive eavesdroppers and MAC-spoofing attackers cannot read the credentials.
- **Integrity of the response:** `crypto_box_seal` includes Poly1305 authentication. Any tampering is detected on decryption.
- **Binding to the correct device:** The verification code ties the public key to a physically observable property. An attacker substituting their own key produces a different code with high probability.

### 11.2 What Is NOT Protected

- **Anonymity of the device:** The join request is sent in the clear. An attacker can see that a device is attempting to onboard.
- **Denial of service:** An attacker can jam the channel or flood with fake join requests.
- **Compromised device firmware:** If the device's RNG or secure storage is compromised, all bets are off.

### 11.3 Verification Code Collision Probability

| Mode | Bits of Entropy | Collision Chance |
|------|----------------|-----------------|
| Color only (8 colors) | 3 bits | 12.5% |
| Digit only (0–9) | ~3.3 bits | 10% |
| Color + digit combined | ~6.3 bits | ~1.25% |
| 4-digit PIN | ~13.3 bits | ~0.01% |

For most IoT onboarding scenarios, the single-mode verification (color or digit) is sufficient given the requirement for physical proximity and active interception during a narrow time window.

---

## 12. Summary of Wire Sizes

| Component | Bytes |
|-----------|-------|
| X25519 public key | 32 |
| JOIN_REQUEST (max) | 83 |
| JOIN_RESPONSE ciphertext overhead | 48 |
| JOIN_RESPONSE total (max, approved) | 155 |
| Verification code | 1 (derived, not transmitted) |

---

## Appendix A: Quick Reference — Onboarding Checklist

**Device firmware must:**
1. Generate X25519 keypair from hardware RNG on first boot
2. Store private key in secure storage
3. Compute BLAKE2b(pubkey) and display as color/digit
4. Transmit JOIN_REQUEST with pubkey, name, MAC, nonce, timestamp
5. Listen for JOIN_RESPONSE matching its nonce
6. Decrypt credentials with private key
7. Destroy private key after successful connection
8. On factory reset, wipe credentials and keypair

**AP / Gateway must:**
1. Listen for JOIN_REQUESTs on the onboarding channel
2. Validate timestamp is within tolerance
3. Compute verification code from received pubkey
4. Present device info + verification code to owner
5. Enforce approval window timeout
6. On approval: encrypt credentials with `crypto_box_seal` to device pubkey
7. Transmit JOIN_RESPONSE with echoed nonce

**Owner must:**
1. Physically look at the device's LED/display
2. Confirm the color/digit matches what the management UI shows
3. Approve or deny within the time window
