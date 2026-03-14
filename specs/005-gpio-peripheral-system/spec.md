# Feature Specification: GPIO & Peripheral Device System

**Feature Branch**: `005-gpio-peripheral-system`
**Created**: 2026-03-07
**Status**: Draft
**Input**: User description: "Modular GPIO and peripheral device system for firmware. There should be a build flag configuration that enables GPIOs as writable InfoItems. They should be in the odf tree available for read to discover their existence and value history. Build configuration can change their mode and infoitem name which defaults to GPIO{x}. Mode should be added to metadata. Modes to be implemented: digital_in, digital_out, analog_in, pwm. Also add build flags to enable common peripheral protocols, which would create infoitems of GPIO{x}_{protocol}_RX and TX similarly. If the protocol has device discovery, implement it to add objects automatically to the tree."

## Clarifications

### Session 2026-03-07

- Q: Where in the O-DF tree should GPIO InfoItems be placed? → A: Directly under device root (e.g., `/DeviceName/GPIO2`, `/DeviceName/LED`).
- Q: How should digital input pins detect state changes? → A: `digital_in` uses polling. Two additional interrupt-driven modes: `low_edge_trigger` and `high_edge_trigger` for edge-triggered detection.
- Q: What format should peripheral protocol RX/TX InfoItem values use? → A: Write requests specify a `type` key (`hex`, `base64`, or `string`) to indicate encoding. The `odf.writeItem()` script API also accepts an optional type parameter. Default is `string` if omitted.
- Q: What build configuration format for per-pin GPIO/peripheral setup? → A: Per-board TOML config files (e.g., `boards/esp32-s2-wrover.toml`), each with a default GPIO/peripheral configuration. Selected by feature flag or env var. Parsed by `build.rs` into const declarations. Scales to dozens of board variants.
- Q: Can scripts interact with GPIO InfoItems via existing odf APIs? → A: Yes. GPIO InfoItems are standard O-DF InfoItems; `odf.readItem()` and `odf.writeItem()` work on them with no special handling. Same write-rejection rules apply for input modes.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Configure and Use Digital GPIO Pins (Priority: P1)

A firmware deployer configures GPIO pins at build time using per-board TOML configuration. Each enabled GPIO pin appears in the O-DF tree as an InfoItem directly under the device root. For digital output pins, scripts (via `odf.writeItem()`) or external clients write values (high/low) to control the pin. For digital input pins, the firmware reads the physical state and updates the InfoItem value, maintaining value history. Scripts can read any GPIO pin's value via `odf.readItem()`.

**Why this priority**: Digital GPIO is the most fundamental hardware interaction. Without it, the device cannot interact with the physical world at all. This is the minimum viable capability for any IoT device.

**Independent Test**: Can be tested by enabling a GPIO pin in build configuration with `digital_out` mode, writing a value to its InfoItem, and verifying the pin state changes. For `digital_in`, can be tested by toggling the physical pin and reading the InfoItem value.

**Acceptance Scenarios**:

1. **Given** a build configuration enabling GPIO2 as `digital_out`, **When** the firmware boots, **Then** an InfoItem named "GPIO2" appears directly under the device root object (e.g., `/DeviceName/GPIO2`) with mode "digital_out" in its metadata.
2. **Given** GPIO2 is configured as `digital_out`, **When** a client writes `1` to the GPIO2 InfoItem, **Then** the physical pin goes high and the value is recorded in the InfoItem's value history.
3. **Given** GPIO5 is configured as `digital_in`, **When** the physical pin state changes, **Then** the InfoItem value updates on the next poll cycle and the change is recorded with a timestamp.
4. **Given** a build configuration with a custom name `"LED"` for GPIO2, **When** the firmware boots, **Then** the InfoItem appears as "LED" instead of "GPIO2".
5. **Given** GPIO2 is configured as `digital_in` (input), **When** a client attempts to write to the GPIO2 InfoItem, **Then** the write is rejected because the pin is read-only.
6. **Given** GPIO4 is configured as `low_edge_trigger`, **When** the pin transitions from high to low, **Then** the InfoItem value is updated immediately via interrupt and the transition is recorded with a timestamp.
7. **Given** GPIO4 is configured as `high_edge_trigger`, **When** the pin transitions from low to high, **Then** the InfoItem value is updated immediately via interrupt and the transition is recorded with a timestamp.

---

### User Story 2 - Analog Input and PWM Output (Priority: P2)

A firmware deployer configures GPIO pins for analog input (ADC) or PWM output. Analog input pins periodically sample the voltage level and update their InfoItem with the reading. PWM output pins accept a duty cycle value written to the InfoItem.

**Why this priority**: Analog and PWM extend GPIO to cover most common sensor and actuator use cases (e.g., reading temperature sensors, dimming LEDs, controlling servo motors). They build on the digital GPIO foundation.

**Independent Test**: Can be tested by configuring a pin as `analog_in`, applying a known voltage, and verifying the InfoItem reflects the reading. For PWM, write a duty cycle and verify the output signal.

**Acceptance Scenarios**:

1. **Given** GPIO34 is configured as `analog_in`, **When** the firmware is running, **Then** the InfoItem value reflects the current analog reading and mode metadata shows "analog_in".
2. **Given** GPIO25 is configured as `pwm`, **When** a client writes a duty cycle value (e.g., `128` for ~50%), **Then** the physical pin outputs a PWM signal at the specified duty cycle.
3. **Given** GPIO34 is configured as `analog_in`, **When** the voltage changes, **Then** new readings are recorded in value history with timestamps.
4. **Given** a PWM pin, **When** a client writes an out-of-range value, **Then** the value is clamped or rejected with a clear indication.

---

### User Story 3 - Peripheral Protocol Buses (Priority: P2)

A firmware deployer enables a peripheral protocol (e.g., I2C, SPI, UART) via build flags, specifying which GPIO pins to use. The system creates RX and TX InfoItems for data exchange. For protocols with device discovery (e.g., I2C address scanning), discovered devices are automatically added as child objects in the O-DF tree.

**Why this priority**: Peripheral protocols are essential for communicating with external sensors and actuators (e.g., I2C temperature sensors, SPI displays). They extend the device's capabilities beyond simple pin-level I/O.

**Independent Test**: Can be tested by enabling I2C on GPIO21/GPIO22, connecting an I2C device, and verifying it appears as a child object in the O-DF tree.

**Acceptance Scenarios**:

1. **Given** I2C is enabled on GPIO21 (SDA) and GPIO22 (SCL), **When** the firmware boots, **Then** InfoItems "GPIO21_I2C_RX" and "GPIO22_I2C_TX" appear in the O-DF tree (or use the configured custom names).
2. **Given** I2C is enabled with discovery, **When** an I2C device is detected at address 0x48, **Then** a child object (e.g., "I2C_0x48") is automatically added under the device's O-DF tree.
3. **Given** UART is enabled on GPIO16 (RX) and GPIO17 (TX), **When** data is received, **Then** the RX InfoItem value is updated with the received data.
4. **Given** UART TX InfoItem, **When** a client writes data with `type: "hex"` set to `"48656C6C6F"`, **Then** the raw bytes for "Hello" are transmitted on the physical UART TX pin.
5. **Given** UART TX InfoItem, **When** a client writes data with `type: "string"` (or no type), **Then** the string value is transmitted as UTF-8 bytes.
6. **Given** UART TX InfoItem, **When** a script calls `odf.writeItem("AQID", "/DeviceName/GPIO16_UART_TX", {type: "base64"})`, **Then** the decoded bytes are transmitted.
7. **Given** SPI is enabled on designated pins, **When** the firmware boots, **Then** appropriate RX/TX InfoItems are created with "SPI" in their names and mode metadata.

---

### User Story 4 - Discover GPIOs and Peripherals via O-DF Tree (Priority: P1)

A client or script discovers all available GPIOs and peripherals by reading the O-DF tree. Each GPIO InfoItem includes metadata indicating its mode, and protocol InfoItems indicate their protocol type. This allows dynamic introspection of the device's hardware capabilities.

**Why this priority**: Discoverability is fundamental to the O-MI/O-DF model. Without it, clients cannot know what hardware is available on the device. This enables generic tooling and scripts to work across different device configurations.

**Independent Test**: Can be tested by reading the O-DF tree root and verifying that all configured GPIOs and peripherals are listed with correct metadata.

**Acceptance Scenarios**:

1. **Given** multiple GPIOs configured in different modes, **When** a client reads the device's O-DF tree, **Then** all configured GPIO InfoItems are visible with their names and metadata.
2. **Given** a GPIO InfoItem, **When** a client reads its metadata, **Then** the mode field (e.g., "digital_in", "digital_out", "analog_in", "pwm", "low_edge_trigger", "high_edge_trigger") is present.
3. **Given** I2C discovery has found devices, **When** a client reads the tree, **Then** discovered device objects appear as children with identifying information (address).

---

### Edge Cases

- What happens when a GPIO pin number is configured that doesn't exist on the target chip? The build configuration should validate pin numbers at compile time where possible, or fail gracefully at boot with a log message.
- What happens when two configurations claim the same GPIO pin? The build system should detect conflicts and produce a compile-time or boot-time error.
- What happens when an I2C device is disconnected after discovery? The child object remains in the tree but its values become stale; metadata should indicate last-seen time.
- What happens when analog reads fail (e.g., ADC not available on the pin)? The InfoItem value should indicate an error state, not crash the firmware.
- What happens when value history grows unbounded? A configurable maximum history depth should apply (using existing O-DF tree limits).
- What happens when a write request specifies an invalid `type` value? The write is rejected with an error indicating the supported types (`hex`, `base64`, `string`).
- What happens when `type: "hex"` is specified but the value contains non-hex characters? The write is rejected with an error.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST support enabling individual GPIO pins via per-board TOML configuration files (e.g., `boards/<board-name>.toml`), selected at build time by feature flag or environment variable. Each board file MUST include a default GPIO and peripheral configuration.
- **FR-002**: Each enabled GPIO pin MUST appear as an InfoItem directly under the device root object in the O-DF tree, using the name "GPIO{x}" by default or a custom name if configured.
- **FR-003**: Each GPIO InfoItem MUST include a mode field in its metadata, set to one of: `digital_in`, `digital_out`, `analog_in`, `pwm`, `low_edge_trigger`, `high_edge_trigger`.
- **FR-004**: `digital_out` and `pwm` InfoItems MUST be writable; writing a value MUST change the physical pin state.
- **FR-005**: `digital_in` and `analog_in` InfoItems MUST be updated by the firmware via polling when the physical pin state changes or is sampled.
- **FR-005a**: `low_edge_trigger` and `high_edge_trigger` InfoItems MUST be updated immediately via interrupt when the configured edge transition occurs on the pin.
- **FR-006**: `digital_in`, `analog_in`, `low_edge_trigger`, and `high_edge_trigger` InfoItems MUST reject external write attempts.
- **FR-007**: All GPIO InfoItem value changes MUST be recorded in value history with timestamps.
- **FR-008**: System MUST support enabling peripheral protocols (I2C, SPI, UART) via build flags, specifying the GPIO pins to use.
- **FR-009**: Enabled peripheral protocols MUST create RX and TX InfoItems named "{name}_{protocol}_RX" and "{name}_{protocol}_TX".
- **FR-009a**: Write requests to protocol TX InfoItems MUST accept an optional `type` key with values `hex`, `base64`, or `string`. If omitted, `string` (UTF-8) is assumed.
- **FR-009b**: The `odf.writeItem()` script API MUST accept an optional type parameter (e.g., `odf.writeItem(value, path, {type: "hex"})`) for protocol TX writes.
- **FR-010**: Protocols with device discovery capability (I2C) MUST scan for connected devices and add discovered devices as child objects in the O-DF tree.
- **FR-011**: Build configuration MUST detect and reject conflicting GPIO pin assignments (same pin used for multiple purposes).
- **FR-012**: GPIO and peripheral code MUST be conditionally compiled -- disabled features add zero code size or memory overhead.
- **FR-013**: Adding a new board variant MUST require only creating a new TOML config file and a corresponding feature flag -- no other source code changes.
- **FR-014**: GPIO InfoItems MUST be accessible via the existing `odf.readItem()` and `odf.writeItem()` script APIs with no special handling. Write-rejection rules for input modes apply equally to script and external client writes.

### Key Entities

- **GPIO InfoItem**: Represents a single GPIO pin in the O-DF tree. Attributes: name (default "GPIO{x}"), mode, current value, value history.
- **Peripheral Bus**: A communication protocol (I2C, SPI, UART) configured on specific GPIO pins. Creates RX/TX InfoItems for data exchange.
- **Discovered Device**: A device detected via protocol discovery (e.g., I2C address scan). Represented as a child object in the O-DF tree under the peripheral bus.
- **GPIO Build Config**: Build-time configuration specifying which pins are enabled, their modes, custom names, and protocol assignments.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All configured GPIO pins are discoverable in the O-DF tree within 1 second of device boot.
- **SC-002**: Digital output pin state changes within 10 milliseconds of a write to its InfoItem.
- **SC-003**: `digital_in` state changes are reflected in the InfoItem within 50 milliseconds of the physical change (polling). Edge trigger modes (`low_edge_trigger`, `high_edge_trigger`) reflect changes within 1 millisecond (interrupt-driven).
- **SC-004**: Disabling all GPIO features results in zero additional memory and code size compared to a build without this feature.
- **SC-005**: I2C device discovery detects all connected devices and populates the tree within 5 seconds of boot.
- **SC-006**: A deployer can configure GPIOs and peripherals using only build flags and config -- no source code changes required.
- **SC-007**: Value history for GPIO InfoItems follows the same retention and retrieval behavior as all other InfoItems in the system.

## Assumptions

- The target hardware is the ESP32 family (ESP32, ESP32-S2, ESP32-S3, ESP32-C3), which determines available GPIO pin numbers and ADC/PWM capabilities.
- Build-time configuration uses per-board TOML files in a `boards/` directory (e.g., `boards/esp32-s2-wrover.toml`), parsed by `build.rs` to generate const declarations. Board selection is via Cargo feature flag or environment variable. This scales to dozens of board variants, each with self-contained default configurations.
- Analog input uses the hardware ADC with default resolution (12-bit for ESP32). Calibration and resolution are not user-configurable in the initial implementation.
- PWM uses the LEDC peripheral on ESP32, with a default frequency suitable for LED dimming (~5 kHz). Frequency configuration may be added later.
- I2C discovery scans all 7-bit addresses (0x08-0x77) at boot. Periodic re-scanning is not included in initial scope.
- SPI and UART do not have device discovery; they only create RX/TX InfoItems for data exchange.
- The sampling rate for `analog_in` and polling rate for `digital_in` use sensible defaults and are not user-configurable in the initial version.
