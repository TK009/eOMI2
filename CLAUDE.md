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


