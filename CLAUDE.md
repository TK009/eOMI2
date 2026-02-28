# Overview

This project implements a simple and small extensions to HTML and JS that allow definition and running of device-side JS.

The core idea is that an embedded device can dynamically be programmed with an HTML file with embedded JS by sending a HTTP POST request.

This project is only the software for the embedded devices.

Language: Rust
Main Target devices: Espressif ESP* family

# Goals

* Lightweight to facilitate support for the cheapest devices
    - As low memory use as possible 
    - As low computation use as possible
* Extremely reliable
    - No errors: extensive testing
    - Fault tolerance

