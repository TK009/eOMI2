# Hyper-Text IoT device

In development

This project implements a reconfigurable IoT device, which can be dynamically programmed with device-side JS and HTML.

The API can be accessed with HTTP or WebSocket and includes a simple data storage and subscription system to connect many devices together.

This project is only the software for the embedded devices of the whole system, which is an improved version of the one implemented in ./doc/master_thesis.pdf.

Main Target devices: Espressif ESP* family

## Development

### Host tests (no hardware needed)

Requires only stable Rust:

    cargo test-host

> **Note:** The alias defaults to `x86_64-unknown-linux-gnu`. On other
> architectures (e.g. Apple Silicon), edit the target triple in
> `.cargo/config.toml`.

### Device build (requires ESP toolchain)

Install the ESP toolchain with [espup](https://github.com/esp-rs/espup), then:

    rustup override set esp   # one-time, in the project directory
    cargo build

### Wi-Fi credentials

Copy `.env.example` to `.env` and fill in your network name and password before building for the device.
