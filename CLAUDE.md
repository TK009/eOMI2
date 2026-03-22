# Overview

This is embedded device project for dynamically programmable IoT devices. See README.md for details.

# Goals

* Lightweight to facilitate support for the cheapest devices
    - As low memory use as possible 
    - As low computation use as possible
* Extremely reliable
    - No errors: extensive testing
    - Fault tolerance

# Conventions

* Keep platform specific code separate from independent code, in other files and folders as far as possible.
* Test independent code heavily on computer, in unit tests, emulators and simulations.
* Make and run e2e tests with hardware as the last task after all non-hw tasks succeeded, or if it is needed for debugging.

# Setup

```sh
./scripts/setup-esp.sh
cargo build
```

# Test

There is host-only tests and device tests.
* Run host tests with alias `cargo test-host`
* E2e tests: `./scripts/run-e2e.sh`

# Device locking

Devices are shared between many docker containers and host, so they need to be locked when in use. Locking is coordinated by an HTTP lock server that works across containers, worktrees, and independent clones.

## Lock server

Start: `./scripts/start-lock-server.sh` (runs on `localhost:7357` by default)
Stop: `./scripts/stop-lock-server.sh`
Override URL: `DEVICE_LOCK_URL=http://host:port`

Locks have a 60-second TTL with automatic heartbeat renewal. Crashed clients' locks auto-expire.

## Usage

* `claim-device.sh` and `release-device.sh` must be **sourced** (`. ./scripts/claim-device.sh`), not executed
* `claim-device.sh` sets `DEVICE_PORT`, `LOCK_ID`, and `HEARTBEAT_PID` in the caller's shell
* `release-device.sh` releases the lock and kills the heartbeat
* `run-with-device.sh <cmd>` is a convenience wrapper: claims, runs a command, releases on exit
* `list-devices.sh` shows device status from the lock server
* Set `CLAIM_DEVICES` env var to pin specific device(s), e.g. `CLAIM_DEVICES="/dev/ttyUSB0"`
* run-e2e.sh handles locking automatically

