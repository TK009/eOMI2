# Feature Specification: Operational Resilience Improvements

**Feature Branch**: `001-operational-resilience`
**Created**: 2026-03-05
**Status**: Draft
**Input**: User description: "Remaining important items from architecture review"

## Clarifications

### Session 2026-03-05

- Q: Should the spec add a wall-clock time limit alongside the existing op-count limit (`MAX_SCRIPT_OPS = 50_000`), or is op-count sufficient? → A: Add wall-clock time limit alongside op-count (belt and suspenders approach).
- Q: Should mutex poisoning recovery (Story 2) target debug/test builds only, change release panic strategy, or be removed entirely? → A: Remove Story 2 entirely; `panic = "abort"` in release already handles the production case.
- Q: Should a timed-out script cause the triggering write to be rolled back, or is "write committed, script effects lost" acceptable? → A: Write committed, script effects lost; report script error to caller.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Script Execution Safety (Priority: P1)

A device operator deploys user-authored mJS scripts to customize device behavior. The system already enforces a bytecode operation count limit to catch simple runaway loops. Additionally, the system must enforce a wall-clock time limit to catch scripts that consume excessive real time (e.g., expensive FFI calls that bypass the op counter). Both mechanisms together ensure the device remains responsive regardless of the failure mode.

**Why this priority**: A single bad script can make the device completely unresponsive. This is the highest-severity issue identified in the architecture review and directly threatens device availability.

**Independent Test**: Can be tested by deploying a script with `while(true){}` and verifying the device remains responsive afterward.

**Acceptance Scenarios**:

1. **Given** a script containing an infinite loop is attached to a node, **When** a write triggers the script, **Then** the script is terminated after the configured timeout and an error is reported.
2. **Given** a script that completes within the timeout, **When** a write triggers the script, **Then** the script executes normally and its effects are applied.
3. **Given** a script is terminated due to timeout, **When** subsequent writes occur, **Then** the device continues processing normally without requiring a restart.
4. **Given** a script times out, **When** the timeout event is logged, **Then** the log includes the script path and elapsed time.
5. **Given** a script times out after the triggering write was committed, **When** the error is reported to the caller, **Then** the write value is preserved and the response indicates partial success (write succeeded, script failed).

---

### User Story 2 - Structured Logging (Priority: P2)

Device operators need structured, filterable log output to diagnose issues in the field. Logs should include contextual fields (session ID, request path) and support configurable verbosity levels per module to control resource usage on the constrained device.

**Why this priority**: Limited observability makes field debugging difficult. While not a correctness issue, it significantly impacts operational efficiency and time-to-resolution.

**Independent Test**: Can be tested by configuring log levels per module and verifying that log output respects the configuration and includes structured fields.

**Acceptance Scenarios**:

1. **Given** a log level is configured for a specific module, **When** log messages are emitted, **Then** only messages at or above the configured level appear.
2. **Given** a WebSocket session is active, **When** server-side log messages are emitted for that session, **Then** the session ID is included in the log output.
3. **Given** a repeated error condition (e.g., WiFi reconnect loop), **When** the same warning fires repeatedly, **Then** the log rate-limits repeated messages to avoid flooding.

---

### Edge Cases

- Cascading script timeout: Each script in a cascade (up to `MAX_SCRIPT_DEPTH = 4`) gets its own independent op-count and wall-clock time limit. A timeout at any depth terminates that script and its pending cascade writes, but writes already committed at earlier depths are preserved.
- How does log rate-limiting behave when errors alternate between two different messages rapidly?

## Requirements *(mandatory)*

### Functional Requirements

#### Script Execution Safety

- **FR-001**: System MUST enforce both a bytecode operation count limit (existing) and a wall-clock time limit for user-supplied scripts.
- **FR-002**: System MUST terminate scripts that exceed either limit and report the specific violation (op-count exceeded or time limit exceeded).
- **FR-003**: System MUST remain fully operational after a script timeout, without requiring a restart.
- **FR-004**: System MUST log script timeout events with the script's associated path and elapsed duration.
- **FR-005**: When a script times out, the triggering write MUST remain committed. The system MUST report a partial-success response indicating the write succeeded but the script failed.

#### Structured Logging

- **FR-006**: System MUST support configurable log levels per module at boot time based on build profile.
- **FR-007**: Server-side log messages MUST include session IDs for WebSocket-related events.
- **FR-008**: System MUST rate-limit repeated identical log messages to prevent log flooding.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A script containing an infinite loop is terminated and the device remains responsive, with zero power-cycle restarts needed due to script hangs.
- **SC-002**: Operators can filter log output by module and severity level, reducing irrelevant log noise by at least 50% in production builds compared to default.
- **SC-003**: Repeated error conditions produce no more than 1 log message per 10-second window per unique message.

## Assumptions

- The existing bytecode operation count limit (`MAX_SCRIPT_OPS`) is functional and tested. The wall-clock time limit is a new addition to catch cases the op counter misses (e.g., FFI calls).
- Release builds use `panic = "abort"`, so mutex poisoning cannot occur in production. The existing `lock_or_recover` logging in debug/test builds is sufficient; no additional mutex recovery work is needed.
- Build profiles (debug vs release) are the appropriate mechanism for controlling default log verbosity.
- Rate-limiting log messages by deduplication window (time-based) is acceptable rather than count-based throttling.
