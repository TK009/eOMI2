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
* To run e2e tests, check that some devices are available `ls /dev/ttyUSB*`
* E2e tests: `./scripts/run-e2e.sh`

# Device locking

Device locking uses kernel-level `flock`, safe across containers.

* `claim-device.sh` and `release-device.sh` must be **sourced** (`. ./scripts/claim-device.sh`), not executed
* `claim-device.sh` sets `DEVICE_PORT` and `DEVICE_FD` in the caller's shell
* `release-device.sh` closes the fd and unsets the variables
* `run-with-device.sh` is a convenience wrapper: claims, runs a command, releases on exit
* Set `CLAIM_DEVICES` env var to pin specific device(s), e.g. `CLAIM_DEVICES="/dev/ttyUSB0"`
* Lockfiles live in `.device-locks/` — `cat .device-locks/*.lock` shows current holders

