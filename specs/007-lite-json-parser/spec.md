# Feature Specification: Lite JSON Parser

**Feature Branch**: `007-lite-json-parser`
**Created**: 2026-03-08
**Status**: Draft
**Input**: User description: "Simplified JSON parser to replace current serialization. It should only implement a state machine for eomi-lite protocol that we have currently implemented."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Parse Incoming OMI Messages (Priority: P1)

The device receives a JSON-encoded OMI message from a client. The parser extracts the envelope fields (`omi`, `ttl`) and the operation payload (`read`, `write`, `delete`, `cancel`, or `response`) without requiring a general-purpose JSON library.

**Why this priority**: Parsing incoming messages is the most fundamental capability. Every device interaction begins with parsing a request.

**Independent Test**: Can be fully tested by feeding known-valid OMI JSON strings into the parser and verifying the correct operation type and field values are extracted.

**Acceptance Scenarios**:

1. **Given** a valid OMI read message as a JSON byte string, **When** the parser processes it, **Then** it produces the same `OmiMessage` structure as the current serde-based parser would.
2. **Given** a valid OMI write message with batch items, **When** the parser processes it, **Then** all items with their paths, values, and optional timestamps are correctly extracted.
3. **Given** a valid OMI response message with nested result data, **When** the parser processes it, **Then** the status code, optional rid, desc, and result payload are correctly extracted.

---

### User Story 2 - Serialize Outgoing OMI Messages (Priority: P2)

The device constructs an OMI response (or other outgoing message) and serializes it to a JSON byte string for transmission, without a general-purpose JSON library.

**Why this priority**: Responses must be sent back to clients. Serialization is needed after parsing is functional.

**Independent Test**: Can be fully tested by constructing `OmiMessage` / `OmiResponse` values, serializing them, and comparing the JSON output against expected strings (or re-parsing and comparing structures).

**Acceptance Scenarios**:

1. **Given** an `OmiResponse` with status 200 and a single result value, **When** serialized, **Then** the output is valid JSON matching the OMI envelope format.
2. **Given** a batch response with per-item statuses, **When** serialized, **Then** each item appears in the result array with its path, status, and optional description.

---

### User Story 3 - Serialize O-DF Data Structures (Priority: P3)

The device serializes its internal O-DF tree (Objects, InfoItems, Values) to JSON for read responses that return hierarchical data.

**Why this priority**: Read responses returning object trees require O-DF serialization. Builds on the envelope serializer from P2.

**Independent Test**: Can be fully tested by constructing Object/InfoItem trees and verifying the JSON output matches expected structure with correct field names, optional field omission, and newest-first value ordering.

**Acceptance Scenarios**:

1. **Given** an Object with nested child Objects and InfoItems, **When** serialized, **Then** the JSON reflects the hierarchy with correct field names (`id`, `type`, `desc`, `items`, `objects`).
2. **Given** an InfoItem with a RingBuffer containing multiple values, **When** serialized, **Then** values appear in newest-first order with `v` and optional `t` fields.
3. **Given** an InfoItem with null optional fields (no `type`, no `desc`, no `meta`), **When** serialized, **Then** those fields are omitted from the JSON output entirely.

---

### Edge Cases

- What happens when the JSON input is truncated mid-message (e.g., network cut)?
- What happens when string values contain escaped characters (`\"`, `\\`, `\n`, `\uXXXX`)?
- What happens when numeric values are at the boundaries of f64 / i64 range?
- How does the parser handle unknown/extra fields in the envelope (forward compatibility)?
- What happens when the input contains deeply nested objects beyond the protocol's nesting limit?
- What happens when the input is empty, or contains only whitespace?
- How does the parser handle duplicate keys in a JSON object?
- What happens when a required field (`omi`, `ttl`) is missing or has the wrong type?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The parser MUST accept a byte slice as input and produce either a parsed `OmiMessage` or a parse error.
- **FR-002**: The parser MUST recognize the OMI envelope structure: version string (`omi`), TTL integer (`ttl`), and exactly one operation key (`read`, `write`, `delete`, `cancel`, `response`).
- **FR-003**: The parser MUST extract operation-specific fields for all five operation types with the same validation rules as the current implementation (e.g., mutually exclusive fields in read/write, non-empty arrays in cancel).
- **FR-004**: The parser MUST handle JSON value types needed by the protocol: strings, integers, floating-point numbers, booleans, null, arrays, and objects. Response result values are constrained to protocol-defined types (OmiValue, arrays of ItemStatus); a general-purpose arbitrary JSON value type is not required.
- **FR-005**: The parser MUST handle JSON string escape sequences: `\"`, `\\`, `\/`, `\b`, `\f`, `\n`, `\r`, `\t`, and `\uXXXX`.
- **FR-006**: The parser MUST reject malformed JSON with descriptive error information (position or context).
- **FR-007**: The parser MUST silently ignore unknown fields in the envelope and operation objects to allow forward compatibility.
- **FR-008**: The serializer MUST produce valid JSON output for OmiMessage, OmiResponse, and O-DF data structures (Object, InfoItem, Value, OmiValue).
- **FR-009**: The serializer MUST omit optional fields that are None/null rather than emitting `"field": null`.
- **FR-010**: The serializer MUST produce value arrays in newest-first order for InfoItem/RingBuffer serialization.
- **FR-011**: The parser and serializer MUST initially be available as a compile-time alternative to the current serde/serde_json implementation (i.e., selectable via feature flag). The lite parser is intended to fully replace serde/serde_json after a validation period, at which point the serde dependency will be removed.
- **FR-012**: The parser MUST produce parse results that are identical to the current serde-based parser for all valid OMI messages.
- **FR-013**: The parser MUST operate without heap allocation where possible, and minimize allocations overall. String values and collections (arrays, objects with dynamic keys) will require allocation.

### Key Entities

- **Token**: A unit of JSON lexical analysis (string, number, boolean, null, colon, comma, brace/bracket open/close).
- **Parser State**: The current position in the state machine, tracking which part of the OMI message structure is being parsed (envelope level, operation level, field level).
- **OmiMessage**: The existing parsed message structure (version, ttl, operation) - unchanged from current implementation.
- **OmiValue**: The existing protocol value type (Null, Bool, Number, Str) - unchanged.

## Clarifications

### Session 2026-03-08

- Q: How should the parser handle arbitrary JSON values in response results (currently `serde_json::Value`)? → A: Constrain response results to only the types the protocol actually uses (OmiValue, arrays of ItemStatus). No general-purpose JSON value type needed.
- Q: What is the long-term intent for the serde/serde_json dependency? → A: Replacement. The lite parser eventually becomes the sole implementation; serde/serde_json removed after validation.
- Q: Should the parser support `no_std` environments? → A: No. The parser requires `std` (standard library assumed).

## Assumptions

- The parser targets the OMI-Lite v1.0 protocol only; general-purpose JSON parsing (arbitrary nesting, streaming, SAX-style events) is explicitly out of scope.
- The state machine approach is chosen over a generic recursive-descent parser to minimize stack usage and code size on constrained devices.
- The existing `OmiMessage`, `Operation`, `OmiValue`, and O-DF data structures remain unchanged; only the serialization/deserialization layer is replaced.
- UTF-8 encoded input is assumed; the parser does not handle other character encodings.
- The parser requires `std`; `no_std` support is not a goal.
- Maximum message size is bounded by available memory; the parser does not need to support streaming/chunked parsing of partial messages.
- Duplicate JSON keys: the parser uses last-value-wins semantics (consistent with serde_json behavior).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All existing OMI protocol tests pass identically with the new parser (100% behavioral compatibility).
- **SC-002**: The new parser uses less compiled binary size than serde + serde_json combined (measured by comparing builds with the `json` feature flag vs. the new parser feature flag).
- **SC-003**: The new parser uses less peak memory during message parsing than the serde-based parser for the same messages.
- **SC-004**: The new parser correctly round-trips (parse then serialize) all valid OMI message types without data loss.
- **SC-005**: The new parser rejects all malformed inputs that the current parser rejects, with equivalent or better error descriptions.
