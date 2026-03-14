# Feature Specification: InfoItem OnRead Script Trigger

**Feature Branch**: `006-infoitem-onread-trigger`
**Created**: 2026-03-07
**Status**: Draft
**Input**: User description: "infoitem onread script trigger that can return a modified or different value via script than what is stored currently"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Computed Values on Read (Priority: P1)

A device operator configures an InfoItem with an `onread` script in its metadata. When any client reads that InfoItem, the system executes the script and returns the script's output instead of (or as a modification of) the stored value. This enables computed or derived values — for example, a temperature sensor that stores raw ADC counts but returns calibrated Celsius values to readers.

**Why this priority**: This is the core feature — without it, there is no onread trigger. It directly enables the primary use case of value transformation at read time.

**Independent Test**: Can be fully tested by writing a raw value to an InfoItem with an `onread` script, then reading it and verifying the returned value matches the script's computed output rather than the stored raw value.

**Acceptance Scenarios**:

1. **Given** an InfoItem at `/SensorA/temperature` with metadata `onread: "event.value * 0.01 - 40"` and a stored value of `6500`, **When** a client sends a read request to `/SensorA/temperature`, **Then** the response contains the computed value `25.0` (i.e. 6500 × 0.01 − 40).
2. **Given** an InfoItem at `/SensorA/temperature` with an `onread` script, **When** a client reads the item, **Then** the stored value in the ring buffer remains unchanged (the script does not mutate stored data).
3. **Given** an InfoItem with no `onread` metadata, **When** a client reads the item, **Then** the stored value is returned as-is with no additional overhead.

---

### User Story 2 - OnRead Script with Access to Item Context (Priority: P2)

A device operator writes an `onread` script that reads other InfoItems or uses the item's own metadata to compute a return value. For example, a "status" InfoItem whose script checks multiple sensor readings and returns "ok" or "alarm".

**Why this priority**: Enables richer use cases where onread scripts aggregate or combine multiple data points, but depends on the basic onread mechanism from P1.

**Independent Test**: Can be tested by setting up multiple InfoItems, attaching an `onread` script to one that reads the others via `odf.readItem()`, and verifying the aggregated result.

**Acceptance Scenarios**:

1. **Given** an InfoItem at `/System/status` with an `onread` script that calls `odf.readItem("/SensorA/temperature/value")` and returns "ok" if temperature < 50, "alarm" otherwise, **When** the stored temperature is `30`, **Then** reading `/System/status` returns `"ok"`.
2. **Given** the same setup, **When** the stored temperature is `60`, **Then** reading `/System/status` returns `"alarm"`.

---

### User Story 3 - OnRead in Subscription Delivery (Priority: P3)

When an interval subscription delivers values for an InfoItem that has an `onread` script, the delivered values are transformed by the script, ensuring subscribers receive the same computed values as direct readers. Event-based subscriptions (triggered by writes) deliver the written value as-is without running the `onread` script, since event delivery is a write notification, not a read.

**Why this priority**: Consistency between direct reads and subscription delivery is important but is a secondary concern after the core read path works.

**Independent Test**: Can be tested by creating an interval subscription on an InfoItem with an `onread` script, triggering a tick, and verifying the delivered value is the script-computed value.

**Acceptance Scenarios**:

1. **Given** a subscription on `/SensorA/temperature` which has an `onread` script, **When** an interval tick fires, **Then** the delivered value is the script-computed value, not the raw stored value.

---

### Edge Cases

- What happens when the `onread` script errors (syntax error, runtime exception)? The system returns the stored value as-is and logs a warning — reads never fail due to script errors.
- What happens when the `onread` script exceeds the time or operation limit? The system returns the stored value as-is and logs a warning.
- What happens when the `onread` script returns `undefined` or no value? The system returns the stored value as-is.
- What happens when an `onread` script calls `odf.readItem()` on another item that also has an `onread` script (cascading)? The nested script also executes, subject to the existing depth limit.
- What happens when an `onread` script calls `odf.readItem()` on itself? The stored value is returned directly (no recursive trigger) to prevent infinite loops.
- What happens when the ring buffer is empty and there is an `onread` script? The script still executes with `event.value` set to `null`, allowing it to provide a default or computed value.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST support an `onread` metadata key on InfoItems whose value is a script string (same format as `onwrite`).
- **FR-002**: When an InfoItem with an `onread` script is read (one-time read, interval subscription delivery, or script `odf.readItem()` call), the system MUST execute the script and use its return value as the value delivered to the reader. Event-based subscription delivery MUST NOT trigger the `onread` script — event notifications deliver the written value as-is.
- **FR-003**: The `onread` script MUST receive an `event` object containing `{ value, path, timestamp }` where `value` is the most recent stored value (or `null` if the ring buffer is empty), `path` is the InfoItem path, and `timestamp` is the stored timestamp (or `null`).
- **FR-004**: The `onread` script's return value MUST replace only the delivered value; the stored value in the ring buffer MUST remain unchanged.
- **FR-005**: If the `onread` script errors, times out, or exceeds operation limits, the system MUST fall back to returning the stored value and MUST log a warning. Reads MUST never fail due to script errors.
- **FR-006**: The `onread` script MUST have access to `odf.readItem()` for reading other items, but MUST NOT have access to `odf.writeItem()` — reads are side-effect-free.
- **FR-007**: When an `onread` script calls `odf.readItem()` on another InfoItem that also has an `onread` script, the nested script MUST execute, subject to the existing cascading depth limit.
- **FR-008**: When an `onread` script calls `odf.readItem()` on the same InfoItem (self-read), the system MUST return the stored value directly without re-triggering the `onread` script, preventing infinite recursion.
- **FR-009**: If an InfoItem has both `onwrite` and `onread` scripts, they operate independently — `onwrite` triggers on value writes, `onread` triggers on value reads.
- **FR-010**: The `onread` script MUST be subject to the same resource limits (time limit, operation limit) and rate limiting as `onwrite` scripts.
- **FR-011**: When an `onread` script returns a value, the element structure response (without `/value` suffix) MUST include the script-computed value in the `values` array while preserving the original timestamps.
- **FR-012**: When a read queries multiple historical values (e.g., `newest=5`), the `onread` script MUST execute only once, transforming the most recent value. Older ring buffer entries MUST be returned as-is without script execution.

### Key Entities

- **InfoItem**: Extended with optional `onread` metadata key containing a script string. Existing fields (`type_uri`, `desc`, `meta`, `values`) are unchanged.
- **Event object**: Read-time context passed to the script: `{ value, path, timestamp }`. Same structure as the `onwrite` event but represents the stored value being read rather than the value being written.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: InfoItems with `onread` scripts return the script-computed value on every read operation within the existing script execution time budget.
- **SC-002**: InfoItems without `onread` scripts have no measurable performance regression on read operations.
- **SC-003**: Script errors on read never cause read failures — 100% of reads succeed regardless of script health.
- **SC-004**: Interval subscription-delivered values are consistent with direct read values for the same InfoItem at the same point in time. Event subscription delivery returns the written value without onread transformation.
- **SC-005**: Self-referencing `onread` scripts (reading own path) complete without infinite recursion.

## Clarifications

### Session 2026-03-07

- Q: When a read queries multiple historical values (e.g., newest=5), should the onread script transform all values or only the newest? → A: Script runs once (newest value only); older ring buffer entries returned as-is.
- Q: Should event-based subscription delivery (triggered by writes) also run the onread script? → A: OnRead runs only for interval subscriptions; event subscriptions deliver the written value as-is.
- Q: When reading an Object (subtree) that contains InfoItems with onread scripts, should the serialized values be transformed? → A: No. Onread scripts execute only for direct InfoItem reads, interval subscription delivery, and `odf.readItem()` calls. Object/Root tree reads return raw stored values.

## Assumptions

- The existing mJS script engine, resource limits, and depth-limiting infrastructure are reused for `onread` scripts — no new scripting runtime is needed.
- The `onread` script uses the same syntax and conventions as `onwrite` scripts (inline JS evaluated via mJS).
- The `event` object structure mirrors the `onwrite` event for consistency, reducing the learning curve for device operators.
- `odf.writeItem()` is intentionally excluded from `onread` scripts to maintain the principle that reads are side-effect-free. If write capability is needed during reads, it should be handled via `onwrite` scripts on a separate item.
