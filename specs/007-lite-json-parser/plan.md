# Implementation Plan: Lite JSON Parser

**Spec**: [spec.md](./spec.md)
**Branch**: `007-lite-json-parser`
**Created**: 2026-03-08

## Architecture Overview

Replace the serde/serde_json dependency with a domain-specific state-machine
parser and manual serializer, selectable via feature flag. The existing data
structures (`OmiMessage`, `Operation`, `OmiValue`, `Object`, `InfoItem`, etc.)
remain unchanged — only the serialization layer is swapped.

### Module layout

```
src/json/
├── mod.rs          — public API: parse_omi_message(), ToJson trait
├── lexer.rs        — byte-level tokenizer (Token enum, Lexer struct)
├── parser.rs       — state-machine OMI message parser
├── serializer.rs   — JSON writer for OMI/O-DF types
└── error.rs        — LiteParseError with position context
```

### Feature flag strategy

- New feature: `lite-json` (no deps)
- Existing feature: `json` (serde + serde_json) — kept as fallback
- `default` feature switches from `json` to `lite-json` after validation
- Both features expose the same public API surface (`OmiMessage::parse()`,
  `Serialize` or `ToJson` trait)
- `#[cfg(feature = "lite-json")]` gates the new module; `#[cfg(feature = "json")]`
  gates the old one. They are mutually exclusive at compile time.

### Parser design

A two-phase approach:
1. **Lexer**: Scans bytes into tokens (string, number, bool, null, `{`, `}`,
   `[`, `]`, `:`, `,`). Handles escape sequences (FR-005). Tracks byte position
   for error reporting.
2. **Parser**: Consumes tokens via a state machine that mirrors the OMI envelope
   structure. Not a generic JSON parser — it knows the expected schema and
   rejects/ignores fields accordingly (FR-007).

---

## Tasks

### Task 1: Feature flag scaffolding and module structure
**Priority**: P1 | **Estimate**: 30min | **Dependencies**: none

Set up the `lite-json` feature in `Cargo.toml` (no external deps). Create the
`src/json/` module directory with `mod.rs`, `lexer.rs`, `parser.rs`,
`serializer.rs`, `error.rs` as empty stubs. Wire `#[cfg(feature = "lite-json")]`
into `src/lib.rs` (or `main.rs`). Ensure `cargo check --features lite-json
--no-default-features` compiles (empty stubs).

Make `json` and `lite-json` mutually exclusive via a `compile_error!` guard.

**Files**: `Cargo.toml`, `src/lib.rs`, `src/json/mod.rs`, `src/json/lexer.rs`,
`src/json/parser.rs`, `src/json/serializer.rs`, `src/json/error.rs`

**Acceptance**: `cargo check --features lite-json,std` compiles. `cargo check
--features json,lite-json` fails with a clear error.

---

### Task 2: LiteParseError type
**Priority**: P1 | **Estimate**: 30min | **Dependencies**: Task 1

Define `LiteParseError` in `src/json/error.rs` with variants that map to the
existing `ParseError` enum: `InvalidJson { pos: usize, detail: String }`,
`MissingField(&'static str)`, `InvalidField { field: &'static str, detail: String }`,
`InvalidOperationCount(usize)`, `UnsupportedVersion(String)`,
`MutuallyExclusive(&'static str, &'static str)`, `UnexpectedToken { pos: usize, expected: &'static str }`.

Implement `From<LiteParseError> for ParseError` so the existing error type is
returned from `OmiMessage::parse()`.

**Files**: `src/json/error.rs`, `src/omi/error.rs` (add From impl or extend)

**Acceptance**: Unit test that each error variant converts to the corresponding
`ParseError`.

---

### Task 3: JSON lexer / tokenizer
**Priority**: P1 | **Estimate**: 3h | **Dependencies**: Task 2

Implement `Lexer` struct in `src/json/lexer.rs`:
- Input: `&[u8]` (byte slice)
- Output: `Token` enum: `String(String)`, `Number(f64)`, `Integer(i64)`,
  `Bool(bool)`, `Null`, `ObjectStart`, `ObjectEnd`, `ArrayStart`, `ArrayEnd`,
  `Colon`, `Comma`
- Methods: `next_token() -> Result<Option<Token>, LiteParseError>`,
  `peek_token()`, `expect_token()`
- Handles all JSON string escapes (FR-005): `\"`, `\\`, `\/`, `\b`, `\f`, `\n`,
  `\r`, `\t`, `\uXXXX` (including surrogate pairs for astral codepoints)
- Skips whitespace between tokens
- Tracks byte position for error messages (FR-006)
- Distinguishes integers from floats (numbers without `.` or `e` → i64 first,
  fall back to f64)

**Files**: `src/json/lexer.rs`

**Acceptance**: Unit tests for each token type, escape sequences, error positions,
whitespace handling, numeric edge cases (integer overflow fallback to f64,
negative numbers, exponents).

---

### Task 4: OMI envelope parser (state machine core)
**Priority**: P1 | **Estimate**: 4h | **Dependencies**: Task 3

Implement `parse_omi_message(input: &[u8]) -> Result<OmiMessage, ParseError>` in
`src/json/parser.rs`. The state machine:

1. Expect `{`
2. Parse top-level keys: `omi` (string), `ttl` (integer), and exactly one
   operation key (`read`, `write`, `delete`, `cancel`, `response`)
3. Unknown keys → skip value (FR-007) using a `skip_value()` helper that
   handles nested objects/arrays
4. Validate: `omi` == "1.0" (FR-002), `ttl` present, exactly one operation
5. Delegate to operation-specific sub-parsers (Task 5)

Wire into `OmiMessage::parse()` via `#[cfg(feature = "lite-json")]` conditional.

**Files**: `src/json/parser.rs`, `src/json/mod.rs`, `src/omi/mod.rs`

**Acceptance**: Parse a minimal read message. Reject missing `omi`, wrong version,
missing `ttl`, zero operations, multiple operations, invalid JSON. Unknown
envelope fields silently ignored.

---

### Task 5: Operation sub-parsers (read, write, delete, cancel, response)
**Priority**: P1 | **Estimate**: 5h | **Dependencies**: Task 4

Implement parser functions for each operation type in `src/json/parser.rs`:

- **ReadOp**: Parse `path`, `rid`, `newest`, `oldest`, `begin`, `end`, `depth`,
  `interval`, `callback`. Validate mutually exclusive fields (path vs rid).
- **WriteOp**: Detect variant — if `items` key → Batch, if `objects` key → Tree,
  else → Single (`path` + `v` + optional `t`). Parse `WriteItem` arrays for
  batch. Parse `Object` trees recursively.
- **DeleteOp**: Parse `path` (required).
- **CancelOp**: Parse `rid` array (required, non-empty).
- **ResponseBody**: Parse `status` (required), optional `rid`, `desc`, `result`.
  For `result`: if array → `Batch(Vec<ItemStatus>)`, if object → parse as
  protocol-constrained value (OmiValue fields, not arbitrary JSON).

Each sub-parser ignores unknown fields (FR-007) and uses last-value-wins for
duplicate keys.

**Files**: `src/json/parser.rs`

**Acceptance**: All existing `OmiMessage::parse()` tests in `src/omi/mod.rs` pass
when compiled with `--features lite-json`. Round-trip tests may not pass yet
(serializer not done).

---

### Task 6: O-DF structure parsing (Object, InfoItem, Value)
**Priority**: P1 | **Estimate**: 3h | **Dependencies**: Task 5

Implement parsers for O-DF data structures needed by WriteOp::Tree:

- **Object**: Parse `id`, optional `type`, `desc`, `items` (map of InfoItem),
  `objects` (map of Object — recursive).
- **InfoItem**: Parse optional `type`, `desc`, `meta` (map of OmiValue),
  `values` (array of Value).
- **Value**: Parse `v` (OmiValue) and optional `t` (f64 timestamp).
- **OmiValue**: Parse JSON primitives → `Null`, `Bool(bool)`, `Number(f64)`,
  `Str(String)`.

**Files**: `src/json/parser.rs`

**Acceptance**: Parse a WriteOp::Tree message with nested objects and items.
Deserialize an Object JSON and compare with serde-parsed equivalent.

---

### Task 7: OmiMessage serializer
**Priority**: P2 | **Estimate**: 3h | **Dependencies**: Task 1

Implement JSON serialization in `src/json/serializer.rs`:

- `JsonWriter` struct wrapping a `Vec<u8>` or `impl Write`
- Methods: `write_str()`, `write_number()`, `write_bool()`, `write_null()`,
  `write_object_start/end()`, `write_array_start/end()`, `write_key()`
- String escaping for output (reverse of lexer escapes)
- Implement `ToJson` trait (or method) for `OmiMessage`, dispatching to
  operation-specific serializers

Serialize envelope: `{"omi":"1.0","ttl":<n>,"<op_key>":{...}}`

Operation serializers:
- **ReadOp**: Emit `path`/`rid` and optional fields, skip None fields (FR-009)
- **WriteOp**: Emit variant-specific fields
- **DeleteOp**: Emit `path`
- **CancelOp**: Emit `rid` array
- **ResponseBody**: Emit `status`, optional `rid`, `desc`, `result`

**Files**: `src/json/serializer.rs`, `src/json/mod.rs`

**Acceptance**: Serialize each operation type and verify valid JSON output.
`serde_json::from_str()` can parse the output (cross-check).

---

### Task 8: O-DF structure serializer
**Priority**: P3 | **Estimate**: 2h | **Dependencies**: Task 7

Implement serialization for O-DF types:

- **Object**: Emit `id`, optional `type`/`desc`/`items`/`objects`. Support
  depth-limited serialization (`serialize_with_depth` equivalent).
- **InfoItem**: Emit optional `type`/`desc`/`meta`, `values` array.
- **Value**: Emit `v` and optional `t` (FR-009).
- **RingBuffer**: Serialize as array in newest-first order (FR-010).
- **OmiValue**: Direct JSON primitive output.

Replace `#[cfg(feature = "json")]` gated `Serialize` derives on O-DF types with
`#[cfg(feature = "lite-json")]` `ToJson` implementations.

**Files**: `src/json/serializer.rs`, `src/odf/object.rs`, `src/odf/item.rs`,
`src/odf/value.rs`, `src/odf/mod.rs`

**Acceptance**: Serialize an Object tree with nested items and values. Output
matches serde-produced JSON for the same structure. None fields omitted.

---

### Task 9: OmiResponse builder adaptation
**Priority**: P2 | **Estimate**: 1.5h | **Dependencies**: Task 7

Adapt `OmiResponse` helper methods in `src/omi/response.rs` to work without
`serde_json::Value` and `serde_json::json!()` when `lite-json` is active:

- Replace `ResponseResult::Single(serde_json::Value)` with a protocol-specific
  type (e.g., `ResponseResult::Single(OmiResponseValue)` where
  `OmiResponseValue` holds the typed fields the protocol actually uses).
- Adapt `subscription_event()` to build result without `serde_json::json!()`.
- Ensure `OmiResponse::ok()`, `partial_batch()`, etc. compile under both
  feature flags.

**Files**: `src/omi/response.rs`, `src/json/serializer.rs`

**Acceptance**: All `OmiResponse` builder tests pass under `lite-json`. Response
round-trip (build → serialize → parse) produces identical results.

---

### Task 10: Compatibility test suite
**Priority**: P1 | **Estimate**: 3h | **Dependencies**: Tasks 5, 7, 9

Comprehensive `cargo test-host` tests ensuring behavioral parity (FR-012):

- **Parse parity**: Feed every existing test JSON string through both parsers
  (use `#[cfg]` to run both in CI), verify identical `OmiMessage` output.
- **Serialize parity**: Serialize each operation type with both implementations,
  verify JSON-equivalent output (key order may differ, so compare parsed values).
- **Round-trip**: Parse → serialize → parse for all operation types (SC-004).
- **Error parity**: All malformed inputs rejected by serde parser are also
  rejected by lite parser (SC-005).
- **Edge cases**: Empty input, whitespace-only, truncated JSON, escaped strings,
  duplicate keys, unknown fields, numeric boundaries.

**Files**: `tests/json_compat.rs` (new test file)

**Acceptance**: `cargo test-host --features lite-json` passes all tests. All
existing tests in `src/omi/mod.rs` pass with `lite-json`.

---

### Task 11: Binary size and memory validation
**Priority**: P3 | **Estimate**: 1h | **Dependencies**: Task 10

Measure and document:
- Binary size comparison: `cargo build --release --features json` vs
  `cargo build --release --features lite-json` (SC-002)
- Peak memory during parsing: instrument with a test that tracks allocator usage
  (SC-003), or use `/usr/bin/time -v` on a parse benchmark

Create a script `scripts/measure-lite-json.sh` that captures both measurements.

**Files**: `scripts/measure-lite-json.sh`

**Acceptance**: Lite parser produces smaller binary than serde+serde_json. Memory
usage documented.

---

### Task 12: E2E device validation
**Priority**: P3 | **Estimate**: 1.5h | **Dependencies**: Task 10

Run the existing e2e test suite with the device built using `--features
lite-json` instead of `json`:
- Flash device with lite-json build
- Run `./scripts/run-e2e.sh`
- Verify all existing e2e tests pass identically (SC-001)

No new e2e tests needed — the existing suite validates protocol behavior.

**Files**: `scripts/run-e2e.sh` (minor: accept feature flag override)

**Acceptance**: All e2e tests pass with lite-json build.

---

## Dependency Graph

```
Task 1 (scaffolding)
  ├─► Task 2 (error type)
  │     └─► Task 3 (lexer)
  │           └─► Task 4 (envelope parser)
  │                 └─► Task 5 (operation parsers)
  │                       ├─► Task 6 (O-DF parsing)
  │                       └─► Task 10 (compat tests) ◄── also 7, 9
  └─► Task 7 (OmiMessage serializer)
        ├─► Task 8 (O-DF serializer)
        ├─► Task 9 (OmiResponse adaptation)
        └─► Task 10 (compat tests)
              ├─► Task 11 (size/memory validation)
              └─► Task 12 (e2e device validation)
```

## Implementation Order (suggested)

1. Task 1 → Task 2 → Task 3 (foundation: feature flags, errors, lexer)
2. Task 4 → Task 5 → Task 6 (parsing: envelope, operations, O-DF)
3. Task 7 → Task 8 (serialization — can start after Task 1, parallel with parsing)
4. Task 9 (OmiResponse adaptation — after Task 7)
5. Task 10 (compatibility tests — after parsing + serialization converge)
6. Task 11 → Task 12 (validation — last)
