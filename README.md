# Embedded O-MI v2

In development

This project implements a reconfigurable IoT device, which can be dynamically programmed with device-side JS and HTML.

The API can be accessed with HTTP or WebSocket and includes a simple data storage and subscription system to connect many devices together.

This project is only the software for the embedded devices of the whole system, which is an improved version of the one implemented in ./doc/master_thesis.pdf.

Main Target devices: Espressif ESP* family

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

Test devices:
- ESP32-S2 WROVER development module

#### ESP toolchain setup

Linux users also need: `gcc build-essential curl pkg-config` (Debian/Ubuntu)
or the equivalent for your distro.

Install esp toolchain and build:
```sh
./scripts/setup-esp.sh
cargo build
```

### Device locking

When multiple processes or containers share the same USB devices, flock-based
locking prevents double-claims. The lock is held at the kernel level, so it
works correctly across container PID namespaces.

```sh
# Run any command with an auto-claimed device:
./scripts/run-with-device.sh espflash flash --port '$DEVICE_PORT' firmware.bin
./scripts/run-with-device.sh bash   # interactive shell with device claimed

# Pin a specific device:
CLAIM_DEVICES="/dev/ttyUSB0" ./scripts/run-with-device.sh minicom -D '$DEVICE_PORT'

# See who holds each device:
cat .device-locks/*.lock
```

The e2e test script (`run-e2e.sh`) automatically waits for a free device.

### Wi-Fi credentials

Copy `.env.example` to `.env` and fill in your network name and password before building for the device.


