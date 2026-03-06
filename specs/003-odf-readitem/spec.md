# Feature Specification: Script API odf.readItem()

**Feature Branch**: `003-odf-readitem`
**Created**: 2026-03-06
**Status**: Draft
**Input**: User description: "Add script API: odf.readItem()"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Read Current Value in Script Logic (Priority: P1)

A script author writes an onwrite script that needs to read the current value of another InfoItem to make decisions. For example, a thermostat script reads the current temperature target when a new temperature reading arrives, and writes a heating command based on the comparison.

**Why this priority**: Reading data is the most fundamental missing capability. Without it, scripts are blind — they can only react to the single incoming write event and produce outputs, but cannot reference any other state in the data tree.

**Independent Test**: Can be tested by creating two InfoItems (a sensor and a target), attaching a script to the sensor that reads the target via `odf.readItem()`, writing a value to the sensor, and verifying the script produced the correct output based on the target value.

**Acceptance Scenarios**:

1. **Given** an InfoItem at path "/target" with value 22.0, **When** a script calls `odf.readItem("/target")`, **Then** it receives the element structure (type, description, values array with timestamps).
2. **Given** an InfoItem at path "/target" with most recent value 22.0, **When** a script calls `odf.readItem("/target/value")`, **Then** it receives the raw value `22.0` directly (a number, not a wrapped object).
3. **Given** an InfoItem at path "/sensor" with an onwrite script that reads "/target/value" and compares, **When** a value is written to "/sensor", **Then** the script reads "/target/value" and writes the correct derived output.
4. **Given** no InfoItem exists at path "/nonexistent", **When** a script calls `odf.readItem("/nonexistent")` or `odf.readItem("/nonexistent/value")`, **Then** it receives `null` (not an error or crash).

---

### User Story 2 - /value Suffix for Direct Value Access (Priority: P1)

A script author uses the `/value` path suffix to get the raw primitive value directly, avoiding the need to navigate a structured element. This convention follows the O-DF data discovery standard as specified in the project's master thesis: paths ending in `/value` return the value directly, while paths ending in the InfoItem name return the full element.

**Why this priority**: The `/value` suffix is the primary ergonomic feature for script authors. Most scripts only need the raw value, and requiring them to unwrap a structure adds unnecessary complexity and code size on a constrained device.

**Independent Test**: Can be tested by reading the same InfoItem with and without the `/value` suffix and verifying the different return formats.

**Acceptance Scenarios**:

1. **Given** an InfoItem at "/Device/Temperature" with most recent value 22.5, **When** `odf.readItem("/Device/Temperature/value")` is called, **Then** it returns `22.5` (raw number).
2. **Given** an InfoItem at "/Device/Temperature" with most recent value 22.5, **When** `odf.readItem("/Device/Temperature")` is called, **Then** it returns the element structure with type, description, and values array.
3. **Given** an InfoItem at "/Device/Status" with string value "OK", **When** `odf.readItem("/Device/Status/value")` is called, **Then** it returns `"OK"` (raw string).
4. **Given** an InfoItem with a boolean value `true`, **When** called with `/value` suffix, **Then** it returns `true` (raw boolean).

---

### User Story 3 - Read Items Marked Non-Readable (Priority: P2)

A script author attempts to read an InfoItem that has been marked as non-readable in its metadata. The system respects the readability flag and returns null, preventing unauthorized data access between scripts.

**Why this priority**: Access control consistency matters for security, but most items are readable by default, so this is a secondary concern.

**Independent Test**: Can be tested by creating a non-readable InfoItem, calling `odf.readItem()` on it from a script, and verifying null is returned.

**Acceptance Scenarios**:

1. **Given** an InfoItem at path "/secret" marked as non-readable, **When** a script calls `odf.readItem("/secret")` or `odf.readItem("/secret/value")`, **Then** it receives `null`.
2. **Given** an InfoItem at path "/public" marked as readable (default), **When** a script calls `odf.readItem("/public")`, **Then** it receives the current element/value.

---

### User Story 4 - Read Within Script Resource Limits (Priority: P2)

A script author calls `odf.readItem()` multiple times within a single script execution. Each call consumes script resources (operations). The system enforces existing resource limits to prevent scripts from performing excessive reads.

**Why this priority**: Resource limits are important for device stability but are already enforced by the existing script execution engine. This story ensures reads integrate correctly with those limits.

**Independent Test**: Can be tested by writing a script that performs reads in a loop and verifying the script terminates within the existing operation/time limits.

**Acceptance Scenarios**:

1. **Given** a script that calls `odf.readItem()` 10 times, **When** the script executes within resource limits, **Then** all reads succeed and the script completes normally.
2. **Given** a script that calls `odf.readItem()` in an unbounded loop, **When** the operation limit is reached, **Then** the script is terminated per existing limits.

---

### Edge Cases

- What happens when `odf.readItem()` is called with a path that points to an Object (not an InfoItem)? It returns `null` since only InfoItems have values.
- What happens when `odf.readItem()` is called with an empty string or invalid path? It returns `null`.
- What happens when `odf.readItem()` is called on an InfoItem that has no values yet (empty ring buffer)? It returns `null`.
- What happens when a script reads a path and then writes to it, triggering another script that reads the same path? The second script sees the updated value (read-after-write consistency within the same processing cycle).
- What happens when `odf.readItem()` is called with no arguments? It returns `null`.
- What happens when an InfoItem is literally named "value" (e.g., "/Device/value")? Standard path resolution takes precedence — "/Device/value" resolves to the InfoItem named "value". To get its raw value, use "/Device/value/value".
- What happens when `/value` is used on a path to root ("/value")? Returns `null` — root has no value.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The scripting environment MUST expose `odf.readItem(path)` as a callable function available to all onwrite scripts.
- **FR-002**: When the path ends with `/value` and the preceding path resolves to an InfoItem, `odf.readItem()` MUST return only the raw primitive value (number, string, boolean, or null) of the most recent entry.
- **FR-003**: When the path does NOT end with `/value` and resolves to an InfoItem, `odf.readItem()` MUST return the element structure including type, description, and values array with timestamps.
- **FR-004**: `odf.readItem(path)` MUST return `null` when the path does not exist, points to an Object instead of an InfoItem, or the InfoItem has no values.
- **FR-005**: `odf.readItem(path)` MUST respect the InfoItem's readability flag — returning `null` for non-readable items.
- **FR-006**: `odf.readItem(path)` MUST operate within the existing script resource limits (operation count, time, memory).
- **FR-007**: `odf.readItem(path)` MUST provide read-after-write consistency — if a value was written earlier in the same processing cycle, the read returns the updated value.
- **FR-008**: `odf.readItem(path)` MUST NOT allow scripts to modify data through the read operation (read-only access).
- **FR-009**: `odf.readItem(path)` MUST accept an absolute path string starting with "/".
- **FR-010**: The `/value` suffix MUST NOT conflict with InfoItems literally named "value" — standard path resolution takes precedence, and `/value` only acts as a value accessor when no InfoItem named "value" exists at that path level.

### Key Entities

- **InfoItem**: A leaf node in the ODF tree containing a ring buffer of timestamped values. The target of `readItem()`.
- **OmiValue**: The primitive value type (null, boolean, number, or string) returned by `readItem()` when using the `/value` suffix.
- **Element Structure**: The full InfoItem representation including type URI, description, and timestamped values array, returned by `readItem()` when NOT using the `/value` suffix.
- **Script Context**: The execution environment for onwrite scripts, which already provides `event` (incoming write data) and `odf.writeItem()`. This feature adds `odf.readItem()` to the same `odf` object.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Scripts can read any readable InfoItem value and use it in decision logic within a single script execution.
- **SC-002**: 100% of existing scripts continue to work unchanged after adding the new API (backward compatibility).
- **SC-003**: Read operations complete within the existing script execution time and operation limits with no measurable increase in per-operation cost.
- **SC-004**: Non-readable items and nonexistent paths consistently return null (zero information leakage).
- **SC-005**: The `/value` suffix provides direct raw value access without any post-processing or unwrapping in script code.

## Assumptions

- The `odf.readItem()` function is added to the existing `odf` global object alongside `writeItem()`, maintaining a consistent API surface.
- The `/value` suffix convention follows the O-DF data discovery standard as described in the project's master thesis.
- Path format follows the existing convention: absolute paths starting with "/" and segments separated by "/".
- The function signature is `odf.readItem(path)` — a single path argument that optionally ends with `/value` to control the return format.
- When using the `/value` suffix, only the most recent single value is returned (not an array). The element structure (without suffix) provides access to the full values array with timestamps.
- InfoItems named "value" are expected to be rare in practice, so the precedence rule (InfoItem name wins over suffix) is acceptable.
- The `/value` suffix is a read-side concern only — it does not affect write operations.
