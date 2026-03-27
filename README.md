# Embedded O-MI v2

**Alpha** -- Functional on ESP32-S2 with core features working. API and internals may change.

Dynamically reconfigurable IoT device firmware. Devices expose an HTTP and WebSocket API implementing the O-MI/O-DF standard, with device-side JavaScript for custom logic and a hierarchical data store for connecting devices together.

An improved version of the system implemented in [./doc/masters_thesis.pdf](./doc/masters_thesis.pdf).

## Features

- **O-MI v2 protocol** -- Read, write, delete, cancel operations over HTTP and WebSocket
- **O-DF data model** -- Hierarchical object/item tree with time-series ring buffers
- **Subscriptions** -- Interval-based data subscriptions (sub-5s support)
- **JavaScript scripting** -- Device-side JS via mJS (OnRead/OnWrite handlers, callbacks, timers)
- **OTA firmware updates** -- Streaming gzip-compressed upload with validation and rollback
- **WiFi management** -- Dual-mode STA+AP, captive portal provisioning, persistent credentials
- **Secure onboarding (WSOP)** -- X25519 key exchange, authenticated encryption, visual verification
- **GPIO control** -- Dynamic pin configuration via TOML board files (digital I/O, ADC, PWM, edge triggers, I2C/UART/SPI)
- **mDNS discovery** -- Automatic device announcement and peer discovery
- **Captive portal** -- Built-in provisioning UI served from the device

## Target hardware

ESP32-S2 boards with tested configurations in [`boards/`](./boards/):

| Board | Key features |
|-------|-------------|
| ESP32-S2-WROVER | Temperature sensor, LED on GPIO 2, digit-mode onboarding |
| ESP32-S2-Saola-1 | WS2812 RGB LED on GPIO 18, color-mode onboarding |

## Development

### Host tests (no hardware needed)

Requires only stable Rust:

```
cargo test-host
```

> **Note:** The alias defaults to `x86_64-unknown-linux-gnu`. On other
> architectures (e.g. Apple Silicon), edit the target triple in
> `.cargo/config.toml`.

### Device build (requires ESP toolchain)

Linux users also need: `gcc build-essential curl pkg-config` (Debian/Ubuntu)
or the equivalent for your distro.

```sh
./scripts/setup-esp.sh
cargo build
```

Select a board with `EOMI_BOARD=esp32-s2-wrover` (or `esp32-s2-saola-1`).

### E2E tests (requires hardware)

```sh
./scripts/start-lock-server.sh   # Start device lock server (once)
./scripts/run-e2e.sh             # Runs full suite with auto device claiming
```

### Wi-Fi credentials

Copy `.env.example` to `.env` and fill in your network name and password before building for the device.

### Device locking

Multiple processes and containers share USB devices via an HTTP lock server
with 60-second TTL and automatic heartbeat renewal.

```sh
# Start the lock server (runs on localhost:7357):
./scripts/start-lock-server.sh

# Run any command with an auto-claimed device:
./scripts/run-with-device.sh espflash flash --port '$DEVICE_PORT' firmware.bin

# Pin a specific device:
CLAIM_DEVICES="/dev/ttyUSB0" ./scripts/run-with-device.sh minicom -D '$DEVICE_PORT'

# See device status:
./scripts/list-devices.sh
```

Override the server URL with `DEVICE_LOCK_URL=http://host:port`.

## Build profiles

| Profile | Use case | Notes |
|---------|----------|-------|
| debug | Development | Symbols enabled, size-optimized (`opt-level = z`) |
| release | Testing | LTO, single codegen unit |
| production | Deployment | Like release + symbol stripping (~110 KB savings) |

## Documentation

- [O-MI Lite](./doc/omi-lite.md) -- Simplified protocol reference
- [WSOP Spec](./doc/wifi-secure-onboarding-spec.md) -- Secure onboarding protocol (draft)
- [O-MI Specification](./doc/omi.pdf) / [O-DF Specification](./doc/odf.pdf)
- [Master's thesis](./doc/masters_thesis.pdf) -- Original system design

## License

See [LICENSE](./LICENSE).
