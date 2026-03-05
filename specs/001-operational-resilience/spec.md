# Feature Specification: Operational Resilience Improvements

**Feature Branch**: `001-operational-resilience`
**Created**: 2026-03-05
**Status**: Draft
**Input**: User description: "Remaining important items from architecture review"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Script Execution Safety (Priority: P1)

A device operator deploys user-authored mJS scripts to customize device behavior. If a script contains an infinite loop or takes too long, the device must protect itself by terminating the script and continuing normal operation, rather than freezing and requiring a power cycle.

**Why this priority**: A single bad script can make the device completely unresponsive. This is the highest-severity issue identified in the architecture review and directly threatens device availability.

**Independent Test**: Can be tested by deploying a script with `while(true){}` and verifying the device remains responsive afterward.

**Acceptance Scenarios**:

1. **Given** a script containing an infinite loop is attached to a node, **When** a write triggers the script, **Then** the script is terminated after the configured timeout and an error is reported.
2. **Given** a script that completes within the timeout, **When** a write triggers the script, **Then** the script executes normally and its effects are applied.
3. **Given** a script is terminated due to timeout, **When** subsequent writes occur, **Then** the device continues processing normally without requiring a restart.
4. **Given** a script times out, **When** the timeout event is logged, **Then** the log includes the script path and elapsed time.

---

### User Story 2 - Mutex Poisoning Recovery (Priority: P2)

When an internal panic poisons a mutex, the device must handle this safely rather than silently continuing with potentially corrupt data. The device should log the event and reset to a known-good state or restart, preventing silent data corruption.

**Why this priority**: Operating on corrupt data after a mutex poisoning can cause unpredictable behavior and data loss. This is a medium-severity issue that affects data integrity.

**Independent Test**: Can be tested by simulating a panic within a mutex-guarded section and verifying the recovery behavior (log output and restart).

**Acceptance Scenarios**:

1. **Given** a mutex becomes poisoned due to a panic, **When** another thread attempts to lock it, **Then** the poisoning event is logged with context about the affected subsystem.
2. **Given** a mutex poisoning is detected, **When** the recovery handler runs, **Then** the device restarts cleanly rather than continuing with potentially corrupt state.
3. **Given** the device restarts after a mutex poisoning, **When** it comes back up, **Then** it loads the last known-good state from persistent storage.

---

### User Story 3 - Structured Logging (Priority: P3)

Device operators need structured, filterable log output to diagnose issues in the field. Logs should include contextual fields (session ID, request path) and support configurable verbosity levels per module to control resource usage on the constrained device.

**Why this priority**: Limited observability makes field debugging difficult. While not a correctness issue, it significantly impacts operational efficiency and time-to-resolution.

**Independent Test**: Can be tested by configuring log levels per module and verifying that log output respects the configuration and includes structured fields.

**Acceptance Scenarios**:

1. **Given** a log level is configured for a specific module, **When** log messages are emitted, **Then** only messages at or above the configured level appear.
2. **Given** a WebSocket session is active, **When** server-side log messages are emitted for that session, **Then** the session ID is included in the log output.
3. **Given** a repeated error condition (e.g., WiFi reconnect loop), **When** the same warning fires repeatedly, **Then** the log rate-limits repeated messages to avoid flooding.

---

### Edge Cases

- What happens when a script timeout occurs during a cascading write (script triggers another script)?
- How does the system behave if a mutex poisoning occurs during NVS persistence?
- How does log rate-limiting behave when errors alternate between two different messages rapidly?

## Requirements *(mandatory)*

### Functional Requirements

#### Script Execution Safety

- **FR-001**: System MUST enforce a maximum execution time for user-supplied scripts.
- **FR-002**: System MUST terminate scripts that exceed the execution time limit and report a timeout error.
- **FR-003**: System MUST remain fully operational after a script timeout, without requiring a restart.
- **FR-004**: System MUST log script timeout events with the script's associated path and elapsed duration.

#### Mutex Poisoning Recovery

- **FR-005**: System MUST log mutex poisoning events with the affected subsystem's identity.
- **FR-006**: System MUST restart the device when a mutex poisoning is detected, rather than continuing with potentially corrupt data.
- **FR-007**: After restart, system MUST load state from the last successful persistent storage save.

#### Structured Logging

- **FR-008**: System MUST support configurable log levels per module at boot time based on build profile.
- **FR-009**: Server-side log messages MUST include session IDs for WebSocket-related events.
- **FR-010**: System MUST rate-limit repeated identical log messages to prevent log flooding.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A script containing an infinite loop is terminated and the device remains responsive, with zero power-cycle restarts needed due to script hangs.
- **SC-002**: Mutex poisoning events result in a controlled device restart within 5 seconds, with no silent data corruption.
- **SC-003**: Operators can filter log output by module and severity level, reducing irrelevant log noise by at least 50% in production builds compared to default.
- **SC-004**: Repeated error conditions produce no more than 1 log message per 10-second window per unique message.

## Assumptions

- The mJS scripting engine supports a timeout or execution-limit mechanism (e.g., `MJS_EXEC_TIMEOUT` flag referenced in the architecture review).
- The device uses `panic = "abort"` in release builds, so mutex poisoning recovery primarily applies to debug/test builds or is handled via a pre-panic hook.
- NVS persistence is reliable enough to serve as the "last known-good state" after a restart.
- Build profiles (debug vs release) are the appropriate mechanism for controlling default log verbosity.
- Rate-limiting log messages by deduplication window (time-based) is acceptable rather than count-based throttling.
